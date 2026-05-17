//! Reconnectable Unix-socket [`Presenter`] for IDE-driven sessions.
//!
//! Unlike `JsonlHost`, which binds the session lifecycle to one stdio
//! stream pair, `SocketPresenter` keeps the orchestrator alive across
//! host disconnects. Each attaching client sends the normal `Hello`
//! handshake, receives a replay of prior session events, and then
//! continues streaming live events over the same JSONL protocol.
//!
//! After the Presenter / LlmAdapter split, LLM dispatch lives inside
//! the orchestrator; the socket transport only carries user-facing
//! events + user input, exactly what `Presenter` requires.
//!
//! [`Presenter`]: super::presenter::Presenter

use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use tracing::{debug, info, warn};

use crate::session::presenter::Presenter;
use crate::session::protocol::{Event, HostEvent};
use crate::{Error, Result};

pub struct SocketPresenter {
    socket_path: PathBuf,
    /// `<socket_path>.ctl`. Listener for the side-channel cancel
    /// socket; populated only when `bind_with_cancel` was used. Drop
    /// removes this file too. Suffix is `.ctl` (not `.control`) so
    /// the path fits inside macOS's 104-byte `sockaddr_un.sun_path`
    /// when Node's `os.tmpdir()` returns the long
    /// `/var/folders/<2>/<28>/T/` darwin-user tmpdir.
    control_socket_path: Option<PathBuf>,
    accept_rx: Receiver<std::os::unix::net::UnixStream>,
    history: Vec<Event>,
    active_reader: Option<BufReader<std::os::unix::net::UnixStream>>,
    active_writer: Option<std::os::unix::net::UnixStream>,
    pending_hello: Option<HostEvent>,
    shutdown_flag: Arc<Mutex<bool>>,
    _accept_thread: JoinHandle<()>,
    /// Accept thread for the control socket. Kept alive for the
    /// pump's lifetime; exits when `shutdown_flag` flips during
    /// Drop. `None` when no cancel channel was wired (legacy
    /// `bind` callers, tests).
    _control_accept_thread: Option<JoinHandle<()>>,
}

impl SocketPresenter {
    pub fn bind(path: PathBuf) -> Result<Self> {
        Self::bind_inner(path, None)
    }

    /// Bind the main protocol socket AND a side-channel control
    /// socket at `<path>.ctl`. The control socket reads
    /// newline-delimited commands from the dashboard while a
    /// dispatch is in flight; the only command today is `cancel`,
    /// which flips the shared `cancel_flag` so LLM backends can
    /// abort their blocking call (subprocess: SIGTERM the child;
    /// HTTP: abandon the ureq worker thread).
    ///
    /// The legacy `bind()` constructor calls this with `None` so
    /// existing tests / non-cancellable paths keep working without
    /// changes; the flag is permanently false in that case.
    pub fn bind_with_cancel(path: PathBuf, cancel_flag: Arc<AtomicBool>) -> Result<Self> {
        Self::bind_inner(path, Some(cancel_flag))
    }

    fn bind_inner(path: PathBuf, cancel_flag: Option<Arc<AtomicBool>>) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                Error::State(format!(
                    "socket-host: cannot mkdir `{}`: {err}",
                    parent.display()
                ))
            })?;
        }
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).map_err(|err| {
            Error::State(format!(
                "socket-host: bind `{}` failed: {err}",
                path.display()
            ))
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|err| Error::State(format!("socket-host: set_nonblocking: {err}")))?;

        let (accept_tx, accept_rx) = mpsc::channel();
        let shutdown_flag = Arc::new(Mutex::new(false));
        let shutdown_for_thread = shutdown_flag.clone();
        let accept_thread = thread::Builder::new()
            .name("sim-flow-session-socket-accept".into())
            .spawn(move || accept_loop(listener, accept_tx, shutdown_for_thread))
            .map_err(|err| Error::State(format!("socket-host: accept thread: {err}")))?;

        info!(socket = %path.display(), "socket-host bound");

        // Bring up the control socket only when a cancel_flag was
        // supplied. Path is `<main>.ctl` so callers that need
        // to find it can compute it from the main path. Listener
        // is a long-lived thread that accepts connections and
        // drains a tiny line-based protocol on each one; the only
        // command today is `cancel\n` (case-insensitive), which
        // sets the shared flag. Multiple connections over the
        // session's lifetime are fine -- the dashboard reconnects
        // on every Stop click.
        let (control_socket_path, control_accept_thread) = if let Some(flag) = cancel_flag {
            let control_path = control_socket_path_for(&path);
            let _ = std::fs::remove_file(&control_path);
            let control_listener =
                std::os::unix::net::UnixListener::bind(&control_path).map_err(|err| {
                    Error::State(format!(
                        "socket-host: bind control socket `{}` failed: {err}",
                        control_path.display()
                    ))
                })?;
            control_listener.set_nonblocking(true).map_err(|err| {
                Error::State(format!("socket-host: control set_nonblocking: {err}"))
            })?;
            let shutdown_for_control = shutdown_flag.clone();
            let handle = thread::Builder::new()
                .name("sim-flow-session-control-accept".into())
                .spawn(move || control_accept_loop(control_listener, flag, shutdown_for_control))
                .map_err(|err| {
                    Error::State(format!("socket-host: control accept thread: {err}"))
                })?;
            info!(socket = %control_path.display(), "socket-host control socket bound");
            (Some(control_path), Some(handle))
        } else {
            (None, None)
        };

        Ok(Self {
            socket_path: path,
            control_socket_path,
            accept_rx,
            history: Vec::new(),
            active_reader: None,
            active_writer: None,
            pending_hello: None,
            shutdown_flag,
            _accept_thread: accept_thread,
            _control_accept_thread: control_accept_thread,
        })
    }

    fn adopt_pending_connection(&mut self, wait_if_none: bool) -> Result<()> {
        let candidate = if wait_if_none && self.active_reader.is_none() {
            self.accept_rx.recv().ok()
        } else {
            None
        };

        let latest = match candidate {
            Some(stream) => Some(stream),
            None => drain_latest_stream(&self.accept_rx),
        };

        let Some(stream) = latest else {
            return Ok(());
        };
        self.attach_stream(stream)
    }

    fn attach_stream(&mut self, stream: std::os::unix::net::UnixStream) -> Result<()> {
        stream
            .set_nonblocking(false)
            .map_err(|err| Error::State(format!("socket-host: clear nonblocking: {err}")))?;
        let writer = stream
            .try_clone()
            .map_err(|err| Error::State(format!("socket-host: clone writer: {err}")))?;
        let mut reader = BufReader::new(stream);
        let hello = read_attach_hello(&mut reader)?;
        if self.pending_hello.is_none() {
            self.pending_hello = Some(hello);
        }
        self.active_reader = Some(reader);
        self.active_writer = Some(writer);
        debug!(
            history_len = self.history.len(),
            "socket-host adopted connection; replaying history"
        );
        self.replay_history_to_active()
    }

    fn replay_history_to_active(&mut self) -> Result<()> {
        let Some(writer) = self.active_writer.as_mut() else {
            return Ok(());
        };
        for event in &self.history {
            if let Err(err) = write_event_line(writer, event) {
                self.active_reader = None;
                self.active_writer = None;
                return Err(err);
            }
        }
        Ok(())
    }

    fn clear_active_connection(&mut self) {
        if self.active_reader.is_some() || self.active_writer.is_some() {
            debug!("socket-host connection lost; awaiting reattach");
        }
        self.active_reader = None;
        self.active_writer = None;
    }
}

impl Presenter for SocketPresenter {
    fn send(&mut self, event: &Event) -> Result<()> {
        self.adopt_pending_connection(false)?;
        self.history.push(event.clone());
        if let Some(writer) = self.active_writer.as_mut()
            && let Err(_err) = write_event_line(writer, event)
        {
            self.clear_active_connection();
        }
        Ok(())
    }

    fn recv(&mut self) -> Result<Option<HostEvent>> {
        if let Some(hello) = self.pending_hello.take() {
            return Ok(Some(hello));
        }
        loop {
            if self.active_reader.is_none() {
                self.adopt_pending_connection(true)?;
                if let Some(hello) = self.pending_hello.take() {
                    return Ok(Some(hello));
                }
            } else {
                self.adopt_pending_connection(false)?;
                if let Some(hello) = self.pending_hello.take() {
                    return Ok(Some(hello));
                }
            }
            let Some(reader) = self.active_reader.as_mut() else {
                continue;
            };
            match read_host_event(reader) {
                Ok(Some(event)) => return Ok(Some(event)),
                Ok(None) => {
                    self.clear_active_connection();
                }
                Err(_err) => {
                    self.clear_active_connection();
                }
            }
        }
    }
}

impl Drop for SocketPresenter {
    fn drop(&mut self) {
        if let Ok(mut g) = self.shutdown_flag.lock() {
            *g = true;
        }
        let _ = std::fs::remove_file(&self.socket_path);
        if let Some(p) = self.control_socket_path.as_ref() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Compute the control socket path from the main protocol socket
/// path. We append `.ctl` to the file name so a single directory
/// can hold both endpoints side-by-side -- no need to coordinate a
/// separate runtime dir. The dashboard derives the same path on its
/// end (see `socketPump.ts`'s control-socket helper).
///
/// Suffix length matters: macOS caps `sockaddr_un.sun_path` at 104
/// bytes, and Node's `os.tmpdir()` already burns 49 of those on the
/// per-user `/var/folders/<2>/<28>/T/` darwin tmpdir. The data
/// socket `sim-flow-<UUID>.sock` lands at 99 bytes; the older
/// 8-char `.control` suffix pushed the control socket to 107 bytes
/// and bind failed with `path must be shorter than SUN_LEN`,
/// orchestrator exited 1, chat panel never came up. `.ctl` keeps
/// the worst case at 102 bytes.
pub fn control_socket_path_for(main: &std::path::Path) -> PathBuf {
    let mut s: std::ffi::OsString = main.as_os_str().to_owned();
    s.push(".ctl");
    PathBuf::from(s)
}

fn accept_loop(
    listener: std::os::unix::net::UnixListener,
    accept_tx: Sender<std::os::unix::net::UnixStream>,
    shutdown_flag: Arc<Mutex<bool>>,
) {
    use std::io::ErrorKind;
    use std::time::Duration;

    loop {
        if let Ok(g) = shutdown_flag.lock()
            && *g
        {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if accept_tx.send(stream).is_err() {
                    break;
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Control socket accept loop. Listens for short-lived dashboard
/// connections, reads newline-delimited commands (only `cancel`
/// is recognized today), and flips the shared `cancel_flag` on
/// match. Threaded so the main protocol socket's `accept_loop` and
/// the orchestrator's `host.recv()` keep their existing semantics
/// untouched while a side-channel cancel can land mid-LLM-dispatch.
///
/// Connections close after one command; the dashboard reconnects
/// for each Stop click, which keeps the handshake stateless. A
/// caller that sends multiple commands per connection still works
/// (the loop reads until the peer closes), but the dashboard side
/// only sends `cancel\n` then disconnects.
fn control_accept_loop(
    listener: std::os::unix::net::UnixListener,
    cancel_flag: Arc<AtomicBool>,
    shutdown_flag: Arc<Mutex<bool>>,
) {
    use std::io::{BufRead, BufReader};
    use std::time::Duration;

    loop {
        if let Ok(g) = shutdown_flag.lock()
            && *g
        {
            break;
        }
        let stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(err) => {
                warn!(error = %err, "control accept loop: accept failed; retrying");
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        // Switch to blocking so BufReader doesn't spin on WouldBlock;
        // the read returns naturally when the peer disconnects.
        if let Err(err) = stream.set_nonblocking(false) {
            warn!(error = %err, "control accept loop: clear nonblocking failed");
            continue;
        }
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break, // peer closed
                Ok(_) => {
                    let cmd = line.trim().to_ascii_lowercase();
                    match cmd.as_str() {
                        "cancel" => {
                            info!("control socket: cancel received -- setting flag");
                            cancel_flag.store(true, Ordering::Release);
                        }
                        "" => {} // empty line; tolerate
                        other => {
                            warn!(cmd = other, "control socket: unknown command (ignoring)");
                        }
                    }
                }
                Err(err) => {
                    warn!(error = %err, "control socket read failed; dropping connection");
                    break;
                }
            }
        }
    }
}

fn drain_latest_stream(
    accept_rx: &Receiver<std::os::unix::net::UnixStream>,
) -> Option<std::os::unix::net::UnixStream> {
    let mut latest = None;
    loop {
        match accept_rx.try_recv() {
            Ok(stream) => latest = Some(stream),
            Err(TryRecvError::Empty) => return latest,
            Err(TryRecvError::Disconnected) => return latest,
        }
    }
}

/// Maximum bytes the socket reader will accumulate into a single
/// newline-delimited frame. A misbehaving (or malicious) host
/// that connects and sends an unterminated line would otherwise
/// stream data into BufReader's String forever, OOMing the
/// orchestrator. 1 MiB is comfortably above any legitimate
/// protocol message (the largest event is an LLM-request body,
/// which the orchestrator caps separately at ~64 KiB) without
/// being so large that an attacker can still exhaust memory in
/// a practical timeframe. See orchestrator audit #11
/// (2026-05-16).
const SOCKET_MAX_LINE_BYTES: usize = 1024 * 1024;

/// Read one newline-delimited line into `dest`, refusing to grow
/// past `SOCKET_MAX_LINE_BYTES`. Returns the number of bytes
/// appended (0 on EOF before any bytes), like `BufRead::read_line`.
/// Anything larger than the limit produces a Protocol error so a
/// malicious / wedged sender can't OOM us.
fn read_line_bounded<R: std::io::BufRead>(reader: &mut R, dest: &mut String) -> Result<usize> {
    let mut buf = Vec::with_capacity(256);
    loop {
        let chunk = reader
            .fill_buf()
            .map_err(|err| Error::Protocol(format!("socket-host: read: {err}")))?;
        if chunk.is_empty() {
            break; // EOF
        }
        let newline_pos = chunk.iter().position(|&b| b == b'\n');
        let take = newline_pos.map(|i| i + 1).unwrap_or(chunk.len());
        if buf.len().saturating_add(take) > SOCKET_MAX_LINE_BYTES {
            return Err(Error::Protocol(format!(
                "socket-host: refusing line larger than {SOCKET_MAX_LINE_BYTES} bytes (unterminated input or attacker)"
            )));
        }
        buf.extend_from_slice(&chunk[..take]);
        reader.consume(take);
        if newline_pos.is_some() {
            break;
        }
    }
    let appended = buf.len();
    let s = String::from_utf8(buf)
        .map_err(|err| Error::Protocol(format!("socket-host: non-UTF8 line: {err}")))?;
    dest.push_str(&s);
    Ok(appended)
}

fn read_attach_hello(reader: &mut BufReader<std::os::unix::net::UnixStream>) -> Result<HostEvent> {
    let mut line = String::new();
    loop {
        line.clear();
        let n = read_line_bounded(reader, &mut line)?;
        if n == 0 {
            return Err(Error::HostClosed(
                "socket-host: client disconnected before hello".into(),
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event: HostEvent = serde_json::from_str(trimmed)
            .map_err(|err| Error::Protocol(format!("socket-host: parse attach hello: {err}")))?;
        match event {
            hello @ HostEvent::Hello { .. } => return Ok(hello),
            _ => {
                return Err(Error::Protocol(
                    "socket-host: first client event must be hello".into(),
                ));
            }
        }
    }
}

fn read_host_event(
    reader: &mut BufReader<std::os::unix::net::UnixStream>,
) -> Result<Option<HostEvent>> {
    let mut line = String::new();
    loop {
        line.clear();
        let n = read_line_bounded(reader, &mut line)?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event: HostEvent = serde_json::from_str(trimmed)
            .map_err(|err| Error::Protocol(format!("socket-host: parse host event: {err}")))?;
        if matches!(event, HostEvent::Hello { .. }) {
            continue;
        }
        return Ok(Some(event));
    }
}

fn write_event_line(writer: &mut std::os::unix::net::UnixStream, event: &Event) -> Result<()> {
    let line = serde_json::to_string(event)
        .map_err(|err| Error::Protocol(format!("socket-host: serialize event: {err}")))?;
    writer
        .write_all(line.as_bytes())
        .map_err(|err| Error::Protocol(format!("socket-host: write event: {err}")))?;
    writer
        .write_all(b"\n")
        .map_err(|err| Error::Protocol(format!("socket-host: write newline: {err}")))?;
    writer
        .flush()
        .map_err(|err| Error::Protocol(format!("socket-host: flush event: {err}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::BufRead;

    use super::*;
    use crate::session::protocol::{
        Event, HostEvent, HostInfo, PROTOCOL_VERSION, SessionKindOut, SessionTag, StepDescriptorOut,
    };

    fn temp_socket_path(name: &str) -> PathBuf {
        let unique = format!(
            "{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        );
        std::env::temp_dir().join(format!("{name}-{unique}"))
    }

    fn connect_and_hello(path: &std::path::Path) -> std::os::unix::net::UnixStream {
        let mut stream = std::os::unix::net::UnixStream::connect(path).unwrap();
        let hello = HostEvent::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host: HostInfo {
                name: "test".into(),
                version: "1".into(),
            },
            capabilities: vec!["markdown".into()],
        };
        let payload = format!("{}\n", serde_json::to_string(&hello).unwrap());
        stream.write_all(payload.as_bytes()).unwrap();
        stream.flush().unwrap();
        stream
    }

    #[test]
    fn socket_host_replays_history_to_reattached_clients() {
        let socket_path = temp_socket_path("socket-host-replay");
        let mut host = SocketPresenter::bind(socket_path.clone()).unwrap();

        let _first = connect_and_hello(&socket_path);
        match host.recv().unwrap() {
            Some(HostEvent::Hello {
                protocol_version, ..
            }) => {
                assert_eq!(protocol_version, PROTOCOL_VERSION);
            }
            other => panic!("expected hello, got {other:?}"),
        }

        host.send(&Event::HelloAck {
            protocol_version: PROTOCOL_VERSION.into(),
            sim_flow_version: "0.0.0-test".into(),
            session: SessionTag {
                step: "DM0".into(),
                kind: SessionKindOut::Work,
                candidate: None,
            },
            step_descriptor: StepDescriptorOut {
                step: "DM0".into(),
                kind: SessionKindOut::Work,
                flow: "dm".into(),
                prerequisite: None,
                instruction_path: "/tmp/spec.md".into(),
                work_artifacts: Vec::new(),
                predecessor_inputs: Vec::new(),
                per_candidate: false,
                phases: vec!["chat".into()],
                tools: Vec::new(),
            },
        })
        .unwrap();
        host.send(&Event::RequestUserInput {
            prompt: Some("continue".into()),
            placeholder: None,
        })
        .unwrap();

        let second = connect_and_hello(&socket_path);
        std::thread::sleep(std::time::Duration::from_millis(100));
        host.send(&Event::Diagnostic {
            level: crate::session::protocol::DiagnosticLevel::Info,
            message: "replayed".into(),
        })
        .unwrap();
        let mut reader = BufReader::new(second);
        let mut line = String::new();

        line.clear();
        reader.read_line(&mut line).unwrap();
        let hello_ack: Event = serde_json::from_str(line.trim()).unwrap();
        assert!(matches!(hello_ack, Event::HelloAck { .. }));

        line.clear();
        reader.read_line(&mut line).unwrap();
        let request: Event = serde_json::from_str(line.trim()).unwrap();
        assert!(matches!(request, Event::RequestUserInput { .. }));
    }

    #[test]
    fn socket_host_delivers_large_events_without_dropping_connection() {
        // 32 KiB assistant turn body. After the Presenter / LlmAdapter
        // split the orchestrator no longer emits `RequestLlmResponse`,
        // but `AssistantText` is the natural large-payload event a
        // single LLM turn produces -- a long reply easily fits 32 KiB
        // and round-trips through the same JSONL serializer the
        // legacy test exercised.
        let socket_path = temp_socket_path("socket-host-large-event");
        let mut host = SocketPresenter::bind(socket_path.clone()).unwrap();

        let first = connect_and_hello(&socket_path);
        match host.recv().unwrap() {
            Some(HostEvent::Hello {
                protocol_version, ..
            }) => {
                assert_eq!(protocol_version, PROTOCOL_VERSION);
            }
            other => panic!("expected hello, got {other:?}"),
        }

        host.send(&Event::HelloAck {
            protocol_version: PROTOCOL_VERSION.into(),
            sim_flow_version: "0.0.0-test".into(),
            session: SessionTag {
                step: "DM0".into(),
                kind: SessionKindOut::Work,
                candidate: None,
            },
            step_descriptor: StepDescriptorOut {
                step: "DM0".into(),
                kind: SessionKindOut::Work,
                flow: "dm".into(),
                prerequisite: None,
                instruction_path: "/tmp/spec.md".into(),
                work_artifacts: Vec::new(),
                predecessor_inputs: Vec::new(),
                per_candidate: false,
                phases: vec!["chat".into()],
                tools: Vec::new(),
            },
        })
        .unwrap();
        host.send(&Event::PhaseChanged {
            phase: "chat".into(),
        })
        .unwrap();
        let big_body = "x".repeat(32 * 1024);
        let writer_body = big_body.clone();
        let writer = std::thread::spawn(move || {
            host.send(&Event::AssistantText {
                text: writer_body,
                final_chunk: true,
                tool_calls: Vec::new(),
            })
            .unwrap();
        });

        let mut reader = BufReader::new(first);
        let mut line = String::new();

        line.clear();
        reader.read_line(&mut line).unwrap();
        let hello_ack: Event = serde_json::from_str(line.trim()).unwrap();
        assert!(matches!(hello_ack, Event::HelloAck { .. }));

        line.clear();
        reader.read_line(&mut line).unwrap();
        let phase: Event = serde_json::from_str(line.trim()).unwrap();
        assert!(matches!(phase, Event::PhaseChanged { .. }));

        line.clear();
        reader.read_line(&mut line).unwrap();
        let request: Event = serde_json::from_str(line.trim()).unwrap();
        match request {
            Event::AssistantText {
                text, final_chunk, ..
            } => {
                assert_eq!(text.len(), 32 * 1024);
                assert!(final_chunk);
            }
            other => panic!("expected AssistantText, got {other:?}"),
        }

        writer.join().unwrap();
    }

    #[test]
    fn control_socket_flips_cancel_flag_on_cancel_line() {
        // End-to-end: bind a SocketPresenter with a cancel flag, connect
        // to the side-channel control socket as the dashboard would,
        // write `cancel\n`, observe the flag flip. This is the wire
        // contract subprocess + HTTP backends poll during dispatch to
        // abort blocking calls.
        let path = temp_socket_path("control");
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let host = SocketPresenter::bind_with_cancel(path.clone(), flag.clone())
            .expect("bind socket with cancel");

        let control_path = control_socket_path_for(&path);
        // The control accept thread is spawned inside bind; give it a
        // moment to call set_nonblocking + reach its first accept()
        // before the dashboard dials. Without the small sleep the
        // connect can race and fail with ENOENT on a fast machine.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut stream = std::os::unix::net::UnixStream::connect(&control_path)
            .expect("connect to control socket");
        use std::io::Write as _;
        stream.write_all(b"cancel\n").unwrap();
        stream.flush().unwrap();
        // Polling cadence inside the control listener is 50 ms (it's
        // non-blocking accept + sleep). Give the flag time to flip;
        // 500 ms is comfortable headroom on CI.
        let start = std::time::Instant::now();
        while !flag.load(Ordering::Acquire) {
            if start.elapsed() > std::time::Duration::from_millis(500) {
                panic!("control socket: cancel flag did not flip within 500ms");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Drop SocketPresenter -> Drop impl removes both socket files.
        drop(host);
        assert!(
            !control_path.exists(),
            "Drop should remove the control socket file at {}",
            control_path.display()
        );
    }

    #[test]
    fn control_socket_path_appends_dot_ctl_suffix() {
        let main = std::path::PathBuf::from("/tmp/foo/bar.sock");
        let control = control_socket_path_for(&main);
        assert_eq!(control, std::path::PathBuf::from("/tmp/foo/bar.sock.ctl"));
    }

    /// Regression check: the macOS darwin-user tmpdir Node's
    /// `os.tmpdir()` returns can push paths close to the 104-byte
    /// `sockaddr_un.sun_path` cap, and `path must be shorter than
    /// SUN_LEN` is the actual symptom that crashed the chat panel
    /// before the suffix shortened from `.control` to `.ctl`. This
    /// test pins the boundary so a future "rename `.ctl` back to
    /// `.control` because it's more readable" change fails loudly
    /// instead of crashing only on real Macs.
    #[test]
    fn control_socket_path_fits_under_sun_len_on_darwin_tmpdir() {
        // Reproduces the failing path from the chat-log report:
        // /var/folders/<2>/<28>/T/sim-flow-<UUID>.sock + ".ctl".
        let main = std::path::PathBuf::from(
            "/var/folders/wj/hfblrhdj2jj6q6xzh2262d5h0000gq/T/sim-flow-1fd3dc92-e4ca-4fdf-ae62-7c1aad8130e2.sock",
        );
        let control = control_socket_path_for(&main);
        let len = control.as_os_str().as_encoded_bytes().len();
        // macOS sun_path is sized 104 bytes including the trailing
        // NUL, so 103 bytes of path is the practical ceiling for the
        // bind syscall.
        assert!(
            len <= 103,
            "control socket path is {len} bytes; macOS sun_path caps at 104 (including NUL). \
             Path: {}",
            control.display()
        );
    }
}
