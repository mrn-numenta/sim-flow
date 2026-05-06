//! `InteractivePtySession` -- spawn a long-lived child process on a
//! pseudo-terminal, transparently proxy the user's stdin / stdout
//! through to it, AND give the orchestrator a side channel to inject
//! synthesized prompts (step instructions, gate results, advance
//! notifications, etc.).
//!
//! This is the foundation of sim-flow's interactive mode for the
//! `claude` CLI. The user gets a normal interactive `claude` TUI in
//! their terminal; the orchestrator can also write to claude's stdin
//! out of band so dashboard buttons translate into "type this prompt"
//! actions inside claude.
//!
//! Lifecycle:
//!
//!   1. Orchestrator calls `spawn` to start the child on a PTY.
//!   2. `proxy_until_exit` puts the controlling terminal into raw mode,
//!      forks two threads (PTY -> stdout, stdin -> PTY) and blocks until
//!      the child exits. While blocked, other threads can call
//!      `inject(...)` on the same `PtyWriter` (mutex-guarded).
//!   3. When the child exits (e.g. user typed `/exit` in claude), the
//!      raw-mode guard restores termios and `proxy_until_exit` returns
//!      with the exit status.
//!
//! Per-step mode and single-session mode are both built on top of this:
//!
//!   - **Per-step**: orchestrator calls spawn -> inject -> proxy per step.
//!   - **Single-session**: orchestrator keeps one `InteractivePtySession`
//!     alive for the whole flow, lazily re-spawning if the child has
//!     exited the next time `inject` is called. Callers reach for
//!     `is_alive` before assuming `inject` will land in the existing
//!     child instead of a fresh one.

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::{Error, Result};

#[cfg(unix)]
use std::os::unix::io::RawFd;

/// Default size used when we can't query the controlling terminal.
const DEFAULT_ROWS: u16 = 40;
const DEFAULT_COLS: u16 = 120;

/// Outcome of a `proxy_until_exit` call.
#[derive(Debug, Clone)]
pub struct ExitInfo {
    /// `Some(code)` for a normal exit, `None` if the platform reports
    /// "killed by signal."
    pub code: Option<i32>,
    /// True when the exit came from the child finishing on its own
    /// (e.g. user typed `/exit` in claude); false when sim-flow sent
    /// SIGTERM / SIGKILL.
    pub clean: bool,
}

/// A handle to write into the spawned child's stdin. Cheaply cloneable
/// (it's `Arc<Mutex<...>>` under the hood) so the orchestrator can hold
/// one clone for `inject` calls and the proxy thread can hold another
/// for forwarding the user's keystrokes -- writes are serialized so no
/// two writers can interleave mid-line.
#[derive(Clone)]
pub struct PtyWriter {
    inner: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl PtyWriter {
    /// Forward a chunk of bytes to the child's stdin verbatim.
    pub fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| Error::Llm("interactive-pty: writer mutex poisoned".into()))?;
        guard
            .write_all(bytes)
            .map_err(|err| Error::Llm(format!("interactive-pty: pty write failed: {err}")))?;
        guard
            .flush()
            .map_err(|err| Error::Llm(format!("interactive-pty: pty flush failed: {err}")))?;
        Ok(())
    }

    /// Inject a synthetic prompt into the child's stdin and submit
    /// it. The wire shape is `body + human-like pause + \r`. The
    /// body is written in a single burst (matching paste behavior:
    /// a real user pasting from the clipboard delivers all bytes
    /// at once, so per-char throttling would actually look LESS
    /// human than this). The pause before `\r` is randomized to
    /// emulate the review-and-press-Enter delay -- see
    /// `pre_enter_pause_ms` for the calibration.
    ///
    /// The shape itself was chosen against claude's interactive TUI
    /// via the `pty_inject_probe` example, which iterates 10
    /// candidate strategies and reports which actually cause claude
    /// to respond. Plain `body + \r` ranked alongside more elaborate
    /// approaches; the pause is belt-and-braces for TUIs that do
    /// timing-based paste detection and would otherwise bundle our
    /// trailing CR into the burst.
    ///
    /// Notes:
    ///   - LF (`\n`) is intentionally NOT used as the submit byte;
    ///     raw-mode TUIs treat LF as "newline character" and leave
    ///     the input field unsubmitted. CR (`\r`) is what the
    ///     terminal sends when the user hits Enter.
    ///   - Bracketed-paste markers (`ESC[200~`...`ESC[201~`) were
    ///     tested and scored marginally lower than plain body+CR.
    ///     Avoiding them keeps the wire simple and removes a
    ///     potential interaction with prompt content that contains
    ///     ANSI escapes itself.
    ///   - Trailing whitespace / newlines on `text` are stripped so
    ///     the caller doesn't have to think about it.
    pub fn inject(&self, text: &str) -> Result<()> {
        let body = text.trim_end_matches(['\r', '\n']);
        if !body.is_empty() {
            self.write_bytes(body.as_bytes())?;
        }
        let pause_ms = pre_enter_pause_ms(body.len());
        std::thread::sleep(std::time::Duration::from_millis(pause_ms));
        self.write_bytes(b"\r")
    }
}

/// Spawn-able interactive PTY child. Holds the master end of the PTY
/// and a writer handle; owns the child process so dropping the session
/// kills it.
pub struct InteractivePtySession {
    /// The argv of the child (kept so `respawn` can re-launch with the
    /// same command).
    cmd: Vec<String>,
    /// Optional working directory; inherited from the parent if `None`.
    cwd: Option<std::path::PathBuf>,
    /// Optional model argument injected into the child's argv. Mostly
    /// used by `ClaudeAgent` to honor `sim-flow.llm.model`.
    extra_args: Vec<String>,
    /// `None` until `spawn` is called. Cleared again when the child
    /// exits so callers can detect liveness via `is_alive`.
    child: Option<Box<dyn Child + Send + Sync>>,
    /// Mutex-guarded writer; cloneable so the proxy + injectors share.
    writer: Option<PtyWriter>,
    /// Master end of the PTY. We only keep it so resize calls remain
    /// possible (not implemented in this pass; placeholder for later).
    _master: Option<Box<dyn MasterPty + Send>>,
}

impl InteractivePtySession {
    pub fn new(
        cmd: impl Into<Vec<String>>,
        cwd: Option<std::path::PathBuf>,
        extra_args: impl Into<Vec<String>>,
    ) -> Self {
        Self {
            cmd: cmd.into(),
            cwd,
            extra_args: extra_args.into(),
            child: None,
            writer: None,
            _master: None,
        }
    }

    /// True iff a child is currently running. Single-session mode uses
    /// this before deciding whether to inject or re-spawn first.
    pub fn is_alive(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(None) => true, // still running
            Ok(Some(_)) => {
                self.child = None;
                self.writer = None;
                self._master = None;
                false
            }
            Err(_) => false,
        }
    }

    /// Spawn the configured command on a fresh PTY. No-op if a child
    /// is already alive; idempotent so callers can `spawn()` then
    /// `inject(...)` without worrying about state.
    pub fn spawn(&mut self) -> Result<()> {
        if self.is_alive() {
            return Ok(());
        }
        if self.cmd.is_empty() {
            return Err(Error::Llm(
                "interactive-pty: cannot spawn with empty argv".into(),
            ));
        }
        let (rows, cols) = controlling_terminal_size().unwrap_or((DEFAULT_ROWS, DEFAULT_COLS));
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| Error::Llm(format!("interactive-pty: openpty: {err}")))?;
        let mut builder = CommandBuilder::new(&self.cmd[0]);
        for arg in &self.cmd[1..] {
            builder.arg(arg);
        }
        for arg in &self.extra_args {
            builder.arg(arg);
        }
        builder.env("TERM", "xterm-256color");
        if let Some(dir) = &self.cwd {
            builder.cwd(dir);
        } else if let Ok(cwd) = std::env::current_dir() {
            builder.cwd(cwd);
        }
        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|err| Error::Llm(format!("interactive-pty: spawn: {err}")))?;
        let writer_inner = pair
            .master
            .take_writer()
            .map_err(|err| Error::Llm(format!("interactive-pty: take_writer: {err}")))?;
        self.writer = Some(PtyWriter {
            inner: Arc::new(Mutex::new(writer_inner)),
        });
        self.child = Some(child);
        self._master = Some(pair.master);
        // Disable the slave's `ECHO`, `OPOST`, and `ONLCR` flags so:
        //   - our injected prompt isn't echoed back to the master and
        //     doubled on the user's terminal (the child's TUI draws
        //     it in its input box; the line-discipline echo would
        //     duplicate that).
        //   - the slave doesn't translate the child's `\n` writes
        //     into `\r\n` and stack a second translation on top of
        //     whatever the child already emits, which produces visible
        //     blank lines between every output line.
        // ICANON is also cleared so injection bytes flow to the child
        // without waiting for a line terminator. Best-effort: a
        // failure here just leaves cooked mode active.
        #[cfg(unix)]
        if let Some(fd) = self._master.as_ref().and_then(|m| m.as_raw_fd()) {
            let _ = configure_slave_termios_raw(fd);
        }
        Ok(())
    }

    /// Get a cloneable writer handle. Spawns the child first if needed.
    pub fn writer(&mut self) -> Result<PtyWriter> {
        self.spawn()?;
        Ok(self
            .writer
            .as_ref()
            .expect("writer set after spawn")
            .clone())
    }

    /// Convenience: spawn (if needed) and inject `text` followed by a
    /// newline. Used by the dashboard's "Run Step" / "Run Gate" / etc.
    /// button handlers.
    pub fn inject(&mut self, text: &str) -> Result<()> {
        self.writer()?.inject(text)
    }

    /// Forcibly kill the child if it's running. Used on shutdown.
    pub fn kill(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.child = None;
        self.writer = None;
        self._master = None;
    }

    /// Take the reader half of the PTY. Each call returns a fresh
    /// reader cloned from the master; if the platform doesn't support
    /// reader cloning, returns an error. The returned reader is moved
    /// into a thread that pumps PTY -> controlling-terminal stdout.
    pub fn take_reader(&mut self) -> Result<Box<dyn Read + Send>> {
        let master = self
            ._master
            .as_mut()
            .ok_or_else(|| Error::Llm("interactive-pty: spawn before take_reader".into()))?;
        master
            .try_clone_reader()
            .map_err(|err| Error::Llm(format!("interactive-pty: clone_reader: {err}")))
    }

    /// Wait for the spawned child to exit. Returns the exit info, or
    /// `Ok(ExitInfo { code: None, clean: false })` if there's no child.
    pub fn wait(&mut self) -> Result<ExitInfo> {
        let Some(mut child) = self.child.take() else {
            return Ok(ExitInfo {
                code: None,
                clean: false,
            });
        };
        let status = child
            .wait()
            .map_err(|err| Error::Llm(format!("interactive-pty: wait: {err}")))?;
        self.writer = None;
        self._master = None;
        Ok(ExitInfo {
            code: status.exit_code().try_into().ok(),
            clean: status.success() || status.exit_code() == 0,
        })
    }
}

impl Drop for InteractivePtySession {
    fn drop(&mut self) {
        self.kill();
    }
}

/// RAII guard: put the controlling terminal into raw mode on enter,
/// restore on drop. Used by `proxy_until_exit` so the user's
/// keystrokes flow byte-for-byte to the PTY child without local echo
/// or line buffering. Best-effort: failing to enter raw mode is a
/// soft failure (we proceed with cooked mode and the user will see
/// double echo).
pub struct RawModeGuard {
    enabled: bool,
}

impl RawModeGuard {
    pub fn enter() -> Self {
        let enabled = crossterm::terminal::enable_raw_mode().is_ok();
        Self { enabled }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

/// Handle returned by [`start_pty_proxy`]. Holds the running reader
/// thread and the raw-mode guard so callers can drop them after
/// `wait_for_child` returns.
pub struct ProxyHandle {
    /// Reader thread: PTY -> our stdout. Joined in [`finish_proxy`]
    /// after the child exits and the PTY reader sees EOF.
    reader_join: Option<JoinHandle<()>>,
    /// RAII guard restoring cooked mode on drop.
    _raw: RawModeGuard,
}

/// Spawn the reader (PTY -> our stdout) and stdin-forwarding (our
/// stdin -> PTY) threads, put the controlling terminal into raw mode,
/// and return a handle. Returns immediately; the caller can `inject`
/// before draining the resulting handle, so the reader is already
/// pulling claude's startup output by the time the prompt arrives at
/// claude's stdin -- no buffer-fill deadlock.
pub fn start_pty_proxy(session: &mut InteractivePtySession) -> Result<ProxyHandle> {
    let writer = session.writer()?;
    let reader = session.take_reader()?;
    let raw = RawModeGuard::enter();

    let reader_join: JoinHandle<()> = thread::Builder::new()
        .name("sim-flow-pty-reader".into())
        .spawn(move || {
            // Write directly to stdout's fd via libc to bypass Rust's
            // `io::stdout()` LineWriter -- we want every chunk we
            // read from the PTY to appear on the user's terminal
            // immediately, even when the chunk doesn't end on a
            // newline boundary (TUIs stream output character-by-
            // character with no LF between updates). `write(2)` on a
            // tty is unbuffered at the kernel level, so this is the
            // most direct path.
            let mut reader = reader;
            let mut buf = [0u8; 8 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if !raw_write_all_stdout(&buf[..n]) {
                            break;
                        }
                    }
                    Err(err) => {
                        if err.kind() != io::ErrorKind::Interrupted {
                            break;
                        }
                    }
                }
            }
        })
        .map_err(|err| Error::Llm(format!("interactive-pty: reader thread: {err}")))?;

    // Stdin thread: stdin -> PTY. Detached; cooked-mode restore on
    // proxy end usually unblocks its current `read` on the next user
    // keystroke. Not joined (parked on stdin).
    let stdin_writer = writer.clone();
    thread::Builder::new()
        .name("sim-flow-pty-stdin".into())
        .spawn(move || {
            let stdin = io::stdin();
            let mut buf = [0u8; 1024];
            loop {
                let n = match stdin.lock().read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                if stdin_writer.write_bytes(&buf[..n]).is_err() {
                    break;
                }
            }
        })
        .map_err(|err| Error::Llm(format!("interactive-pty: stdin thread: {err}")))?;

    Ok(ProxyHandle {
        reader_join: Some(reader_join),
        _raw: raw,
    })
}

/// Wait for the child to exit, drain the reader thread, and restore
/// cooked mode. Companion to [`start_pty_proxy`].
pub fn finish_proxy(
    session: &mut InteractivePtySession,
    mut handle: ProxyHandle,
) -> Result<ExitInfo> {
    let exit = session.wait()?;
    if let Some(j) = handle.reader_join.take() {
        let _ = j.join();
    }
    // RAII guard inside `handle` restores cooked mode on drop.
    drop(handle);
    // Belt-and-braces: explicit cooked-mode restore in case the guard
    // was bypassed by a panic.
    let _ = crossterm::terminal::disable_raw_mode();
    thread::sleep(Duration::from_millis(20));
    Ok(exit)
}

/// Convenience: combines `start_pty_proxy` + `finish_proxy` for the
/// common per-step case where the caller has nothing to inject between
/// reader-start and child-wait. Single-session mode prefers the split
/// form so it can `inject` after the reader is up but before waiting.
pub fn proxy_until_exit(session: &mut InteractivePtySession) -> Result<ExitInfo> {
    let handle = start_pty_proxy(session)?;
    finish_proxy(session, handle)
}

/// Read the current size of the controlling terminal, if any.
fn controlling_terminal_size() -> Option<(u16, u16)> {
    crossterm::terminal::size()
        .map(|(cols, rows)| (rows, cols))
        .ok()
}

/// Pause to insert between writing the injected body and writing the
/// trailing `\r`, in milliseconds. Two design constraints:
///
/// 1. **Don't exceed the rate of input a human could produce.** A
///    real user pasting then pressing Enter has at minimum a
///    perception-and-decision delay -- the eye has to register that
///    the paste landed, the brain has to confirm it's the right
///    content, and the finger has to hit Enter. Lab studies put
///    that floor around 200-300ms for trivial content; for review
///    of an actual prompt it's longer. We sample 350-900ms as the
///    base "think time" plus 2-5ms per pasted character (a light
///    scan-rate proxy). Capped at 3 seconds because past that, a
///    real human would have either committed or gone back to
///    edit, not stared at the input box longer.
///
/// 2. **Vary turn to turn so we don't look mechanical.** A fixed
///    250ms pause every time is a tell; a uniform random in a
///    realistic band looks like normal cursor latency.
///
/// The body itself is pasted in a single burst (which IS the human-
/// realistic shape for paste -- per-char delivery would look like
/// hand-typing, which a real user wouldn't do for a multi-paragraph
/// prompt). The pause is the ONLY place we slow down.
fn pre_enter_pause_ms(body_len: usize) -> u64 {
    let base = rand_range_u64(350, 900);
    let per_char = rand_range_u64(2, 5);
    (base + per_char * body_len as u64).min(3_000)
}

/// Tiny dependency-free PRNG for inject-pause jitter. Returns a
/// uniformly-sampled `u64` in `[lo, hi)`. Per-thread state is
/// xorshift64 seeded lazily from a clock + thread-id mix; that's
/// nowhere near cryptographic but plenty for "make the pause look
/// non-mechanical" purposes. We avoid pulling in `rand` because this
/// is the only consumer of randomness in the crate and the entire
/// quality bar is "doesn't repeat the same number twice in a row."
fn rand_range_u64(lo: u64, hi_exclusive: u64) -> u64 {
    use std::cell::Cell;
    use std::time::{SystemTime, UNIX_EPOCH};
    debug_assert!(hi_exclusive > lo);
    thread_local! {
        static STATE: Cell<u64> = const { Cell::new(0) };
    }
    STATE.with(|s| {
        let mut x = s.get();
        if x == 0 {
            // Seed from system time + thread id; both are coarse but
            // their combination differs across calls and threads.
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0xa5a5_a5a5_a5a5_a5a5);
            // Mix in the thread id so multiple PTY workers don't
            // synchronize on identical seeds.
            let tid_bits: u64 = {
                // `thread::ThreadId` doesn't expose its inner u64
                // directly; the Debug impl prints `ThreadId(N)`. We
                // hash the address of a thread-local instead -- the
                // address is stable per thread and trivially unique.
                let probe: u8 = 0;
                std::ptr::addr_of!(probe) as usize as u64
            };
            x = nanos.wrapping_mul(0x9e37_79b9_7f4a_7c15)
                ^ tid_bits.wrapping_mul(0xbf58_476d_1ce4_e5b9)
                ^ 0xdead_beef_cafe_babe;
            if x == 0 {
                x = 1; // xorshift collapses to 0 forever from a 0 state
            }
        }
        // xorshift64 (Marsaglia 2003 -- short, fast, good enough).
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        lo + (x % (hi_exclusive - lo))
    })
}

/// Write `bytes` to fd 1 (stdout) via direct `write(2)`, looping over
/// short writes and ignoring `EINTR`. Returns false if the underlying
/// fd reports a fatal error (caller should drop out of its read loop).
///
/// We bypass `io::stdout()` because the standard `Stdout` is a
/// `LineWriter` -- partial-line writes (which TUI streaming produces
/// constantly, since tokens don't align to LFs) sit in the buffer
/// until either an `\n` arrives or `flush` is called. Calling `flush`
/// per chunk works on paper but adds avoidable syscalls and races
/// with `LineWriter`'s internal state machine. Direct `write(2)` is
/// the cleanest path.
#[cfg(unix)]
fn raw_write_all_stdout(bytes: &[u8]) -> bool {
    let mut remaining = bytes;
    while !remaining.is_empty() {
        let written = unsafe {
            libc::write(
                libc::STDOUT_FILENO,
                remaining.as_ptr() as *const libc::c_void,
                remaining.len(),
            )
        };
        if written < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return false;
        }
        remaining = &remaining[(written as usize)..];
    }
    true
}

#[cfg(not(unix))]
fn raw_write_all_stdout(bytes: &[u8]) -> bool {
    use std::io::Write;
    let stdout = io::stdout();
    let mut h = stdout.lock();
    h.write_all(bytes).is_ok() && h.flush().is_ok()
}

/// Configure the slave PTY's termios for clean TUI passthrough by
/// disabling line-discipline echo and output-side newline translation.
///
/// Why this matters:
///
/// 1. **`ECHO` / `ECHONL` / `ECHOE` / `ECHOK`**: with these on, every
///    byte we `inject` into the master is reflected back as part of
///    the master's read stream. The reader thread then writes the
///    reflection to the user's terminal, *and* the child's TUI draws
///    the same content in its input box. The user sees the prompt
///    twice -- once as a stream of echoed characters, once as the
///    rendered TUI input. Disabling ECHO* leaves the TUI as the sole
///    source of truth for what the input box shows.
/// 2. **`OPOST` / `ONLCR` / `OCRNL`**: with these on, the slave maps
///    `\n` → `\r\n` (and sometimes `\r` → `\n`) on the child's output.
///    Combined with the parent terminal already being in raw mode,
///    each child-emitted `\n` arrives as `\r\n`; if the child also
///    emits `\r\n` itself (TUIs commonly do for portability), the
///    slave doubles that to `\r\r\n`, which renders as a blank line
///    between every visual line. Clearing the o-flags makes the
///    slave a pass-through.
/// 3. **`ICANON`**: line-buffered input. We want our injected bytes
///    to flow to the child immediately (the TUI reads them via raw
///    mode anyway once Ink takes over).
///
/// The child (claude / Ink) sets its own raw mode shortly after
/// startup, but by then our prompt has already been injected and the
/// initial banner echoed. Configuring the slave ourselves before
/// inject closes that window.
///
/// `tcsetattr` on the master fd configures the same termios the
/// slave reads (Linux + macOS share the line discipline between
/// master and slave for a given pty pair). Best-effort: a failure
/// here is logged-and-swallowed -- the user gets the old behavior.
#[cfg(unix)]
fn configure_slave_termios_raw(fd: RawFd) -> std::result::Result<(), std::io::Error> {
    use std::mem::MaybeUninit;
    let mut termios = MaybeUninit::<libc::termios>::uninit();
    let rc = unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut termios = unsafe { termios.assume_init() };
    // Output-side: clear all post-processing so the slave doesn't
    // translate the child's newlines.
    termios.c_oflag &= !(libc::OPOST | libc::ONLCR | libc::OCRNL | libc::ONOCR | libc::ONLRET);
    // Local flags: kill echo + line buffering. ISIG stays (so
    // Ctrl-C from the user reaches the child as SIGINT via the
    // line discipline, since we forward 0x03 verbatim from raw
    // parent stdin).
    termios.c_lflag &=
        !(libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ECHONL | libc::ICANON | libc::IEXTEN);
    // Input-side: stop CR↔NL translations on the slave's input.
    // The TUI handles `\r` (Enter) directly once it sets raw mode
    // itself, and we explicitly send `\r` from `inject`.
    termios.c_iflag &= !(libc::ICRNL | libc::INLCR | libc::IGNCR | libc::IXON);
    let rc = unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_alive_returns_false_before_spawn() {
        let mut session =
            InteractivePtySession::new(vec!["/bin/sh".to_string()], None, Vec::<String>::new());
        assert!(!session.is_alive());
    }

    #[test]
    fn spawn_then_kill_makes_is_alive_false() {
        // Use `cat` since it stays alive waiting for stdin -- gives us
        // a deterministic "still running" window for the test.
        let mut session =
            InteractivePtySession::new(vec!["/bin/cat".to_string()], None, Vec::<String>::new());
        session.spawn().expect("spawn cat");
        assert!(session.is_alive(), "child should be alive after spawn");
        session.kill();
        assert!(!session.is_alive(), "child should be dead after kill");
    }

    #[test]
    fn writer_round_trips_via_inject_and_pty_echo() {
        // `cat` echoes stdin to stdout. We can't easily read the PTY
        // master from a unit test (the reader thread normally pumps
        // it to stdout) but we can prove `inject` writes by running
        // `cat` then killing it -- if the write didn't go through,
        // the PtyWriter::write_bytes call would return Err.
        let mut session =
            InteractivePtySession::new(vec!["/bin/cat".to_string()], None, Vec::<String>::new());
        session.spawn().expect("spawn cat");
        session.inject("hello").expect("inject");
        session.kill();
    }

    #[test]
    fn spawn_with_empty_argv_returns_error() {
        let mut session =
            InteractivePtySession::new(Vec::<String>::new(), None, Vec::<String>::new());
        let err = session.spawn().unwrap_err();
        assert!(format!("{err}").contains("empty argv"));
    }

    #[test]
    fn pre_enter_pause_stays_in_human_band() {
        // Floor: even an empty body has a non-zero base think time
        // (350-900ms) so we never blast `\r` immediately after the
        // body lands.
        for _ in 0..32 {
            let p = pre_enter_pause_ms(0);
            assert!(p >= 350, "pause floor violated for empty body: {p}ms");
            assert!(p < 900, "pause base ceiling violated for empty body: {p}ms");
        }
        // Cap: even huge bodies stop at 3s -- a real human reviewing
        // a long paste doesn't sit there for 30s before pressing
        // Enter.
        for _ in 0..32 {
            let p = pre_enter_pause_ms(1_000_000);
            assert!(p <= 3_000, "pause cap violated for huge body: {p}ms");
        }
    }

    #[test]
    fn pre_enter_pause_varies_across_calls() {
        // The whole point of randomizing is that we don't always
        // land on the same number. With 16 samples and a band of
        // ~550ms, the odds of all-equal are vanishingly small unless
        // the PRNG is broken.
        let mut samples = std::collections::HashSet::new();
        for _ in 0..16 {
            samples.insert(pre_enter_pause_ms(50));
        }
        assert!(
            samples.len() > 1,
            "pre_enter_pause_ms returned the same value 16 times in a row -- PRNG seed collapse?",
        );
    }

    #[test]
    fn rand_range_respects_bounds() {
        for _ in 0..256 {
            let v = rand_range_u64(10, 20);
            assert!((10..20).contains(&v), "rand_range_u64 out of band: {v}");
        }
    }
}
