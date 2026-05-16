//! Control socket for the interactive PTY driver. The dashboard
//! connects here to remote-control a long-lived `claude` session
//! (single-session mode). Each line on the socket is one JSON command;
//! the driver dispatches it to either a write-into-claude or a
//! local action whose result is then written into claude.
//!
//! Wire format (JSONL):
//!
//! ```jsonc
//! // Send a literal user message to claude (newline appended).
//! {"command":"inject","text":"explain what you just changed"}
//!
//! // Run the structural gate locally; orchestrator injects the
//! // result as a system-style note into claude's stdin.
//! {"command":"run-gate","step":"DM2c"}
//!
//! // Run gate, mark passed if clean, bump current_step, then
//! // inject the next step's prompt into claude.
//! {"command":"advance","step":"DM2c"}
//!
//! // Reset a step (cascades to downstream gates).
//! {"command":"reset","step":"DM2c"}
//!
//! // Tear down claude and exit the driver.
//! {"command":"shutdown"}
//! ```
//!
//! Responses are JSONL too, one event per line:
//!
//! ```jsonc
//! {"event":"injected"}
//! {"event":"gate-result","step":"DM2c","clean":true,"failures":[]}
//! {"event":"state-advanced","from":"DM2c","to":"DM3"}
//! {"event":"error","message":"...","detail":"..."}
//! {"event":"shutdown"}
//! ```
//!
//! Unix domain socket only for the moment. Windows path can come later
//! via `tokio::net::windows::named_pipe` or a TCP fallback.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Commands the dashboard can issue. Names match the JSON `command`
/// discriminant exactly so we can `serde_json::from_str` straight
/// into this enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum ControlCommand {
    /// Write `text` (with a trailing newline) into claude's stdin.
    Inject { text: String },
    /// Evaluate the structural gate for `step` (or current step if
    /// omitted). The driver runs `gate::evaluate` locally and writes
    /// the formatted result back into claude.
    RunGate { step: Option<String> },
    /// Run the gate; on clean, mark passed and bump `current_step`;
    /// then inject the next step's prompt into claude.
    Advance { step: Option<String> },
    /// Reset a step. Cascades to downstream gates per the existing
    /// `sim-flow reset` semantics.
    Reset { step: String },
    /// Stop the driver. Kills claude if alive and breaks out of the
    /// main loop.
    Shutdown,
}

/// Events the driver writes back to a connected dashboard.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum ControlEvent {
    /// Acknowledgement that an `inject` command landed in claude's
    /// stdin pipe (NOT that claude actually read it).
    Injected,
    /// Result of a `run-gate` invocation. `failures` is an empty
    /// array when `clean` is true.
    GateResult {
        step: String,
        clean: bool,
        failures: Vec<GateFailure>,
    },
    /// Sent after a successful advance.
    StateAdvanced { from: String, to: Option<String> },
    /// Generic error response with a one-line message and optional
    /// long-form detail (e.g. cargo stderr).
    Error {
        message: String,
        detail: Option<String>,
    },
    /// Final event sent before the driver tears down.
    Shutdown,
}

#[derive(Debug, Clone, Serialize)]
pub struct GateFailure {
    pub description: String,
    pub reason: String,
}

/// Default control-socket path for a project.
pub fn default_socket_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".sim-flow").join("control.sock")
}

/// Listener handle. Drop to stop accepting new connections (existing
/// clients keep working until they close).
pub struct ControlListener {
    socket_path: PathBuf,
    /// Channel of commands received from any connected client.
    /// Wrapped in a Mutex so the struct is Sync and an
    /// `Arc<ControlListener>` can be shared across threads (the
    /// dispatch thread + the main proxy loop).
    rx: Mutex<Receiver<ControlCommand>>,
    /// Cloneable broadcast handle: each connected client gets one.
    /// The driver writes events here; the accept thread fans them
    /// out to per-connection writer threads.
    broadcast_tx: Sender<ControlEvent>,
    /// Accept-loop join handle. Dropping the listener triggers
    /// shutdown of this thread.
    _accept_thread: JoinHandle<()>,
    /// Set to true on drop to signal the accept thread to exit.
    shutdown_flag: Arc<Mutex<bool>>,
}

impl ControlListener {
    /// Bind a Unix domain socket at `path`. Removes a stale file at
    /// the same path before binding (the path is a control plane,
    /// not data we want to preserve). Returns a listener whose `rx`
    /// channel drains commands from any connected client.
    pub fn bind(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                Error::State(format!(
                    "control-socket: cannot mkdir `{}`: {err}",
                    parent.display()
                ))
            })?;
        }
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).map_err(|err| {
            Error::State(format!(
                "control-socket: bind `{}` failed: {err}",
                path.display()
            ))
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|err| Error::State(format!("control-socket: set_nonblocking: {err}")))?;

        let (cmd_tx, rx) = channel::<ControlCommand>();
        let (broadcast_tx, broadcast_rx) = channel::<ControlEvent>();
        let shutdown_flag = Arc::new(Mutex::new(false));
        let shutdown_for_thread = shutdown_flag.clone();
        let socket_path = path.clone();
        let accept_thread = thread::Builder::new()
            .name("sim-flow-control-accept".into())
            .spawn(move || {
                accept_loop(listener, cmd_tx, broadcast_rx, shutdown_for_thread);
            })
            .map_err(|err| Error::State(format!("control-socket: accept thread: {err}")))?;

        Ok(Self {
            socket_path,
            rx: Mutex::new(rx),
            broadcast_tx,
            _accept_thread: accept_thread,
            shutdown_flag,
        })
    }

    /// Block until the next command arrives. Returns `None` when all
    /// senders have been dropped (i.e. listener shutting down).
    pub fn recv(&self) -> Option<ControlCommand> {
        self.rx.lock().ok()?.recv().ok()
    }

    /// Non-blocking poll for a command.
    pub fn try_recv(&self) -> Option<ControlCommand> {
        self.rx.lock().ok()?.try_recv().ok()
    }

    /// Send an event to every connected client. Best-effort: a slow
    /// or dead client doesn't block the driver.
    pub fn broadcast(&self, event: ControlEvent) {
        let _ = self.broadcast_tx.send(event);
    }
}

impl Drop for ControlListener {
    fn drop(&mut self) {
        if let Ok(mut g) = self.shutdown_flag.lock() {
            *g = true;
        }
        // Best-effort cleanup of the socket file.
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn accept_loop(
    listener: std::os::unix::net::UnixListener,
    cmd_tx: Sender<ControlCommand>,
    _broadcast_rx: Receiver<ControlEvent>,
    shutdown_flag: Arc<Mutex<bool>>,
) {
    use std::time::Duration;
    // Naive polling accept. Fine for an extension sending a couple of
    // commands per minute; we don't need epoll for this.
    loop {
        if let Ok(g) = shutdown_flag.lock()
            && *g
        {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let stream = match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let cmd_tx = cmd_tx.clone();
                thread::Builder::new()
                    .name("sim-flow-control-conn".into())
                    .spawn(move || handle_connection(stream, cmd_tx))
                    .ok();
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Max bytes the control socket reader will accumulate into one
/// command line before bailing. A misbehaving local client could
/// otherwise stream an unterminated line and OOM the
/// orchestrator -- `reader.lines()` uses `read_line` which grows
/// its String unbounded. 1 MiB is well above any legitimate
/// ControlCommand JSON. See orchestrator audit #11 (2026-05-16).
const CONTROL_SOCKET_MAX_LINE_BYTES: usize = 1024 * 1024;

fn handle_connection(stream: std::os::unix::net::UnixStream, cmd_tx: Sender<ControlCommand>) {
    use std::io::{BufRead, BufReader};
    let mut reader = BufReader::new(stream);
    loop {
        let mut buf = Vec::with_capacity(256);
        let exceeded = loop {
            let chunk = match reader.fill_buf() {
                Ok(c) => c,
                Err(_) => return,
            };
            if chunk.is_empty() {
                return; // EOF
            }
            let newline_pos = chunk.iter().position(|&b| b == b'\n');
            let take = newline_pos.map(|i| i + 1).unwrap_or(chunk.len());
            if buf.len().saturating_add(take) > CONTROL_SOCKET_MAX_LINE_BYTES {
                // Refuse the oversize line and skip the rest of
                // this connection -- the client is misbehaving.
                break true;
            }
            buf.extend_from_slice(&chunk[..take]);
            reader.consume(take);
            if newline_pos.is_some() {
                break false;
            }
        };
        if exceeded {
            return;
        }
        let Ok(text) = std::str::from_utf8(&buf) else {
            continue; // non-UTF8 garbage; skip
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<ControlCommand>(trimmed) {
            Ok(cmd) => {
                if cmd_tx.send(cmd).is_err() {
                    return; // listener has been dropped
                }
            }
            Err(_err) => {
                // Unparseable line. Skip it; surfacing parse errors
                // on the connection-back channel is Pass 2 polish.
                continue;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_inject_command_through_serde() {
        let json = r#"{"command":"inject","text":"hello"}"#;
        let cmd: ControlCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ControlCommand::Inject { text } => assert_eq!(text, "hello"),
            other => panic!("expected Inject, got {other:?}"),
        }
    }

    #[test]
    fn run_gate_with_explicit_step() {
        let json = r#"{"command":"run-gate","step":"DM2c"}"#;
        let cmd: ControlCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ControlCommand::RunGate { step: Some(s) } => assert_eq!(s, "DM2c"),
            other => panic!("expected RunGate, got {other:?}"),
        }
    }

    #[test]
    fn shutdown_command_has_no_fields() {
        let json = r#"{"command":"shutdown"}"#;
        let cmd: ControlCommand = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd, ControlCommand::Shutdown));
    }

    #[test]
    fn unknown_command_fails_to_parse() {
        let json = r#"{"command":"do-the-thing"}"#;
        let result: std::result::Result<ControlCommand, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn default_socket_path_lives_under_dot_sim_flow() {
        let p = default_socket_path(Path::new("/tmp/proj"));
        assert_eq!(p, Path::new("/tmp/proj/.sim-flow/control.sock"));
    }

    #[test]
    fn bind_creates_socket_file_then_drop_removes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let sock = tmp.path().join("ctrl.sock");
        let listener = ControlListener::bind(sock.clone()).unwrap();
        assert!(sock.exists(), "socket file should exist after bind");
        drop(listener);
        // Drop's cleanup happens on the same thread; file is gone.
        assert!(!sock.exists(), "socket file should be removed on drop");
    }
}
