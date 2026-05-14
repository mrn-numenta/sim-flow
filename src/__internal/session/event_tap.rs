//! Read-only event broadcast over a Unix socket.
//!
//! The orchestrator's primary host (`JsonlHost` or `SocketHost`)
//! handles bidirectional command + event traffic with one driver.
//! The `EventTap` is a parallel sink that BROADCASTS every emitted
//! event to any number of attached observers without touching the
//! command channel. Used to let a dashboard, a `tail`-style debug
//! tool, or a second IDE window watch a run that's being driven by
//! something else (e2e_manual, a chat extension, an external script).
//!
//! Each attaching client:
//! - Receives a JSONL replay of every event the orchestrator has
//!   already emitted (so late attachers see the full history -- the
//!   step descriptors, sub-session brackets, and any earlier
//!   diagnostics).
//! - Then receives every new event live as it's emitted.
//! - Sends nothing back. Anything written by an observer is ignored;
//!   commands go to the primary host only.
//!
//! Concurrency model: a dedicated accept thread waits on the
//! listener and pushes new connections onto a shared sender list.
//! `broadcast` is called on the orchestrator thread (synchronously
//! inside `Host::write`) and walks the sender list, dropping any
//! sender that returns a write error. Slow observers can stall the
//! orchestrator briefly during a write -- acceptable for a debug
//! tool but worth knowing.
//!
//! The socket is unlinked on drop. Stale sockets from a previous run
//! are removed at bind time.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::session::presenter::Presenter;
use crate::session::protocol::{Event, HostEvent};
use crate::{Error, Result};

/// Public metadata for one running orchestrator's watch socket.
/// Written to a registry file at bind time and removed on drop, so
/// any observer (the dashboard's "Attach to running session"
/// picker, an external script) can enumerate live runs without
/// needing to know the socket path up front.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchRegistration {
    pub pid: u32,
    pub socket_path: PathBuf,
    pub project_dir: PathBuf,
    pub started_at: String,
    pub llm_backend: String,
    pub llm_model: Option<String>,
}

pub struct EventTap {
    socket_path: PathBuf,
    registry_path: Option<PathBuf>,
    history: Arc<Mutex<Vec<Vec<u8>>>>,
    senders: Arc<Mutex<Vec<UnixStream>>>,
    shutdown: Arc<AtomicBool>,
    _accept_thread: JoinHandle<()>,
}

impl EventTap {
    /// Bind a Unix socket at `path` and start accepting observer
    /// connections. Errors when the path can't be created or bound.
    pub fn bind(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                Error::State(format!(
                    "event-tap: cannot mkdir `{}`: {err}",
                    parent.display()
                ))
            })?;
        }
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).map_err(|err| {
            Error::State(format!(
                "event-tap: bind `{}` failed: {err}",
                path.display()
            ))
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|err| Error::State(format!("event-tap: set_nonblocking: {err}")))?;

        let history = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let senders = Arc::new(Mutex::new(Vec::<UnixStream>::new()));
        let shutdown = Arc::new(AtomicBool::new(false));

        let history_for_thread = history.clone();
        let senders_for_thread = senders.clone();
        let shutdown_for_thread = shutdown.clone();

        let accept_thread = thread::Builder::new()
            .name("sim-flow-event-tap-accept".into())
            .spawn(move || {
                accept_loop(
                    listener,
                    history_for_thread,
                    senders_for_thread,
                    shutdown_for_thread,
                );
            })
            .map_err(|err| Error::State(format!("event-tap: accept thread: {err}")))?;

        info!(socket = %path.display(), "event-tap bound");

        Ok(Self {
            socket_path: path,
            registry_path: None,
            history,
            senders,
            shutdown,
            _accept_thread: accept_thread,
        })
    }

    /// Bind a tap and register it in the well-known discovery
    /// directory so the dashboard's "Attach to running session"
    /// picker (and any external script) can enumerate live runs.
    /// `info.pid` should be the orchestrator's pid; the registry
    /// filename is `<pid>.json` so a single orchestrator is
    /// idempotent across re-binds. The registry file is removed
    /// when the tap is dropped.
    pub fn bind_with_registration(path: PathBuf, info: WatchRegistration) -> Result<Self> {
        let mut tap = Self::bind(path)?;
        let registry_dir = registry_dir()?;
        std::fs::create_dir_all(&registry_dir).map_err(|err| {
            Error::State(format!(
                "event-tap: cannot mkdir registry `{}`: {err}",
                registry_dir.display()
            ))
        })?;
        let registry_path = registry_dir.join(format!("{}.json", info.pid));
        let body = serde_json::to_string_pretty(&info)
            .map_err(|err| Error::State(format!("event-tap: registry serialize: {err}")))?;
        std::fs::write(&registry_path, body).map_err(|err| {
            Error::State(format!(
                "event-tap: write registry `{}`: {err}",
                registry_path.display()
            ))
        })?;
        info!(
            registry = %registry_path.display(),
            "event-tap: registered for discovery"
        );
        tap.registry_path = Some(registry_path);
        Ok(tap)
    }
}

/// Resolve the discovery directory where running orchestrators
/// register their `WatchRegistration` JSON files. Lookup order:
///
/// 1. `SIM_FLOW_WATCHER_DIR` env var (test override).
/// 2. `XDG_RUNTIME_DIR/sim-flow/watchers/` when set (Linux).
/// 3. Platform user-cache dir + `sim-flow/watchers/` via the
///    `directories` crate (macOS / Windows).
/// 4. Last-resort fallback: `<temp>/sim-flow-watchers/`.
pub fn registry_dir() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var("SIM_FLOW_WATCHER_DIR") {
        return Ok(PathBuf::from(custom));
    }
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR")
        && !runtime.is_empty()
    {
        return Ok(PathBuf::from(runtime).join("sim-flow").join("watchers"));
    }
    if let Some(dirs) = directories::ProjectDirs::from("", "", "sim-flow") {
        return Ok(dirs.cache_dir().join("watchers"));
    }
    Ok(std::env::temp_dir().join("sim-flow-watchers"))
}

/// Enumerate every registered watcher in `registry_dir()`. Stale
/// entries (process no longer alive, socket file missing) are
/// silently skipped and their registry files removed so the list
/// reflects what's actually attachable. Returns the surviving
/// entries in arbitrary order; callers sort.
pub fn list_registrations() -> Result<Vec<WatchRegistration>> {
    let dir = registry_dir()?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(err) => {
            return Err(Error::State(format!(
                "event-tap: read registry `{}`: {err}",
                dir.display()
            )));
        }
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".json") {
            continue;
        }
        let body = match std::fs::read_to_string(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let reg: WatchRegistration = match serde_json::from_str(&body) {
            Ok(r) => r,
            Err(_) => {
                // Malformed file -- best-effort cleanup so it
                // doesn't poison future scans.
                let _ = std::fs::remove_file(&path);
                continue;
            }
        };
        if !is_alive(reg.pid) || !reg.socket_path.exists() {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        out.push(reg);
    }
    Ok(out)
}

/// Cheap `kill -0`-style liveness check on a pid. Returns false on
/// platforms / errors we can't probe; the caller treats false as
/// "stale, drop the registration."
fn is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: kill(2) with signal 0 is the standard pid-liveness
        // probe; it doesn't deliver a signal and only checks
        // permissions / existence. Returns 0 on alive, -1 with errno
        // ESRCH for "no such process" or EPERM for "alive but we
        // can't signal it" (which still means alive).
        let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if r == 0 {
            return true;
        }
        let err = std::io::Error::last_os_error();
        err.raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

impl EventTap {
    fn cleanup_registry(&self) {
        if let Some(path) = &self.registry_path {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Serialize `event` and broadcast it to every connected
    /// observer. The serialized bytes are also appended to the
    /// in-process history so future attachers can replay. Errors are
    /// swallowed: a misbehaving observer must not break the
    /// orchestrator's primary command path.
    pub fn broadcast(&self, event: &Event) {
        let mut buf = match serde_json::to_vec(event) {
            Ok(b) => b,
            Err(err) => {
                warn!(error = %err, "event-tap: serialize failed; dropping event");
                return;
            }
        };
        buf.push(b'\n');

        // Append to history first so a connection that races us at
        // the accept thread still sees this event in replay.
        if let Ok(mut history) = self.history.lock() {
            history.push(buf.clone());
        }

        let mut senders = match self.senders.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        senders.retain_mut(|stream| match stream.write_all(&buf) {
            Ok(()) => true,
            Err(err) => {
                debug!(error = %err, "event-tap: dropping closed observer");
                false
            }
        });
    }
}

impl Drop for EventTap {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = std::fs::remove_file(&self.socket_path);
        self.cleanup_registry();
    }
}

/// `Presenter` decorator that broadcasts every emitted event to an
/// `EventTap` while delegating sends + recvs to the inner primary
/// presenter. Used by the `auto` command when `--watch-socket` is set.
pub struct TappedPresenter<P> {
    inner: P,
    tap: EventTap,
}

impl<P: Presenter> TappedPresenter<P> {
    pub fn new(inner: P, tap: EventTap) -> Self {
        Self { inner, tap }
    }
}

impl<P: Presenter> Presenter for TappedPresenter<P> {
    fn send(&mut self, event: &Event) -> Result<()> {
        self.tap.broadcast(event);
        self.inner.send(event)
    }
    fn recv(&mut self) -> Result<Option<HostEvent>> {
        self.inner.recv()
    }
}

fn accept_loop(
    listener: UnixListener,
    history: Arc<Mutex<Vec<Vec<u8>>>>,
    senders: Arc<Mutex<Vec<UnixStream>>>,
    shutdown: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _addr)) => {
                if let Err(err) = onboard_observer(stream, &history, &senders) {
                    warn!(error = %err, "event-tap: failed to onboard observer");
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(err) => {
                warn!(error = %err, "event-tap: accept error; retrying");
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn onboard_observer(
    stream: UnixStream,
    history: &Arc<Mutex<Vec<Vec<u8>>>>,
    senders: &Arc<Mutex<Vec<UnixStream>>>,
) -> Result<()> {
    stream
        .set_nonblocking(false)
        .map_err(|err| Error::State(format!("event-tap: clear nonblocking: {err}")))?;

    // Read the observer's `Hello` line (and discard) before replaying
    // history. This mirrors the SocketHost handshake so the same
    // SocketSessionPump on the dashboard can attach without changes.
    // The handshake also flushes any pre-attach buffer the kernel
    // delivered before we set blocking, so the next write_all is
    // sequenced cleanly. We tolerate a missing Hello (just skip the
    // read) for `nc -U`-style ad-hoc attachers.
    let mut writer = stream
        .try_clone()
        .map_err(|err| Error::State(format!("event-tap: clone observer: {err}")))?;
    let mut reader = BufReader::new(stream);
    if let Err(err) = peek_and_discard_hello(&mut reader) {
        debug!(error = %err, "event-tap: observer sent no Hello (ok)");
    }

    // Replay history under the lock so a concurrent broadcast can't
    // interleave a new event between our last replayed line and the
    // sender registration -- the sender lock guards the broadcast
    // path AND the registration here.
    let history_snapshot = history
        .lock()
        .map_err(|_| Error::State("event-tap: history lock poisoned".into()))?;
    let mut senders_guard = senders
        .lock()
        .map_err(|_| Error::State("event-tap: senders lock poisoned".into()))?;
    for line in history_snapshot.iter() {
        if let Err(err) = writer.write_all(line) {
            debug!(error = %err, "event-tap: replay failed; observer disconnected before live");
            return Ok(());
        }
    }
    senders_guard.push(writer);
    debug!(
        history_len = history_snapshot.len(),
        senders_len = senders_guard.len(),
        "event-tap: observer attached"
    );
    Ok(())
}

fn peek_and_discard_hello(reader: &mut BufReader<UnixStream>) -> Result<()> {
    // Set a short read timeout so non-Hello attachers don't stall
    // the accept thread. We don't actually validate the Hello shape
    // -- this is a read-only tap, so the worst a malformed Hello
    // can do is be silently dropped.
    if let Ok(stream) = reader.get_ref().try_clone() {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
    }
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
    // Restore blocking behavior for any later reads we'd add.
    if let Ok(stream) = reader.get_ref().try_clone() {
        let _ = stream.set_read_timeout(None);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::{DiagnosticLevel, SessionEndReason};
    use std::time::Duration;

    fn temp_socket_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "sim-flow-event-tap-test-{}-{}",
            name,
            std::process::id()
        ));
        p
    }

    fn drain_n_lines(stream: &mut UnixStream, n: usize) -> Vec<String> {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut out = Vec::new();
        for _ in 0..n {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => out.push(line.trim_end_matches('\n').to_string()),
            }
        }
        out
    }

    #[test]
    fn broadcasts_to_two_concurrent_observers() {
        let path = temp_socket_path("two_obs");
        let tap = EventTap::bind(path.clone()).expect("bind");

        // Connect two observers BEFORE any event lands so neither
        // hits the replay path.
        let mut a = UnixStream::connect(&path).expect("a connect");
        let mut b = UnixStream::connect(&path).expect("b connect");
        // No-op Hello (one byte + newline) so the tap's
        // `read_line` returns and we get added to the senders list.
        a.write_all(b"hello\n").unwrap();
        b.write_all(b"hello\n").unwrap();

        // Give the accept thread time to onboard both observers.
        std::thread::sleep(Duration::from_millis(150));

        tap.broadcast(&Event::Diagnostic {
            level: DiagnosticLevel::Info,
            message: "hello tap".into(),
        });
        tap.broadcast(&Event::SessionEnd {
            reason: SessionEndReason::Completed,
            message: None,
        });

        let lines_a = drain_n_lines(&mut a, 2);
        let lines_b = drain_n_lines(&mut b, 2);
        assert_eq!(lines_a.len(), 2, "got: {lines_a:?}");
        assert_eq!(lines_b.len(), 2, "got: {lines_b:?}");
        assert!(lines_a[0].contains("hello tap"));
        assert!(lines_b[0].contains("hello tap"));
        assert!(lines_a[1].contains("session-end"));
        assert!(lines_b[1].contains("session-end"));
    }

    #[test]
    fn replays_history_to_late_attacher() {
        let path = temp_socket_path("replay");
        let tap = EventTap::bind(path.clone()).expect("bind");

        // Emit two events BEFORE any observer connects.
        tap.broadcast(&Event::Diagnostic {
            level: DiagnosticLevel::Warning,
            message: "early-1".into(),
        });
        tap.broadcast(&Event::Diagnostic {
            level: DiagnosticLevel::Warning,
            message: "early-2".into(),
        });

        let mut obs = UnixStream::connect(&path).expect("connect");
        obs.write_all(b"hello\n").unwrap();

        std::thread::sleep(Duration::from_millis(150));

        // Live event AFTER the observer attaches.
        tap.broadcast(&Event::Diagnostic {
            level: DiagnosticLevel::Info,
            message: "live-1".into(),
        });

        let lines = drain_n_lines(&mut obs, 3);
        assert_eq!(lines.len(), 3, "got: {lines:?}");
        assert!(lines[0].contains("early-1"));
        assert!(lines[1].contains("early-2"));
        assert!(lines[2].contains("live-1"));
    }

    #[test]
    fn dropping_observer_does_not_break_broadcast() {
        let path = temp_socket_path("drop_obs");
        let tap = EventTap::bind(path.clone()).expect("bind");

        let mut a = UnixStream::connect(&path).expect("a connect");
        a.write_all(b"hello\n").unwrap();
        std::thread::sleep(Duration::from_millis(100));

        // Slam the connection closed; broadcast must still succeed.
        drop(a);

        // Give the broadcaster a moment to notice the close on its
        // next write attempt.
        tap.broadcast(&Event::Diagnostic {
            level: DiagnosticLevel::Info,
            message: "after-drop".into(),
        });

        // A second observer attaches AFTER the drop and gets the
        // history (which now includes "after-drop").
        let mut b = UnixStream::connect(&path).expect("b connect");
        b.write_all(b"hello\n").unwrap();
        std::thread::sleep(Duration::from_millis(100));

        let lines = drain_n_lines(&mut b, 1);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("after-drop"));
    }
}
