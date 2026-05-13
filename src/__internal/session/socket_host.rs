//! Reconnectable Unix-socket host for IDE-driven sessions.
//!
//! Unlike `JsonlHost`, which binds the session lifecycle to one stdio
//! stream pair, `SocketHost` keeps the orchestrator alive across host
//! disconnects. Each attaching client sends the normal `Hello`
//! handshake, receives a replay of prior session events, and then
//! continues streaming live events over the same JSONL protocol.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use tracing::{debug, info};

use crate::session::host::Host;
use crate::session::protocol::{Event, HostEvent};
use crate::{Error, Result};

pub struct SocketHost {
    socket_path: PathBuf,
    accept_rx: Receiver<std::os::unix::net::UnixStream>,
    history: Vec<Event>,
    active_reader: Option<BufReader<std::os::unix::net::UnixStream>>,
    active_writer: Option<std::os::unix::net::UnixStream>,
    pending_hello: Option<HostEvent>,
    shutdown_flag: Arc<Mutex<bool>>,
    _accept_thread: JoinHandle<()>,
}

impl SocketHost {
    pub fn bind(path: PathBuf) -> Result<Self> {
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
        Ok(Self {
            socket_path: path,
            accept_rx,
            history: Vec::new(),
            active_reader: None,
            active_writer: None,
            pending_hello: None,
            shutdown_flag,
            _accept_thread: accept_thread,
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

impl Host for SocketHost {
    fn write(&mut self, event: &Event) -> Result<()> {
        self.adopt_pending_connection(false)?;
        self.history.push(event.clone());
        if let Some(writer) = self.active_writer.as_mut()
            && let Err(_err) = write_event_line(writer, event)
        {
            self.clear_active_connection();
        }
        Ok(())
    }

    fn read(&mut self) -> Result<Option<HostEvent>> {
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

impl Drop for SocketHost {
    fn drop(&mut self) {
        if let Ok(mut g) = self.shutdown_flag.lock() {
            *g = true;
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
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

fn read_attach_hello(reader: &mut BufReader<std::os::unix::net::UnixStream>) -> Result<HostEvent> {
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|err| Error::Protocol(format!("socket-host: read attach hello: {err}")))?;
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
        let n = reader
            .read_line(&mut line)
            .map_err(|err| Error::Protocol(format!("socket-host: read host event: {err}")))?;
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
    use super::*;
    use crate::session::protocol::{
        Event, HostEvent, HostInfo, LlmMessage, LlmRole, PROTOCOL_VERSION, SessionKindOut,
        SessionTag, StepDescriptorOut,
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
        let mut host = SocketHost::bind(socket_path.clone()).unwrap();

        let _first = connect_and_hello(&socket_path);
        match host.read().unwrap() {
            Some(HostEvent::Hello {
                protocol_version, ..
            }) => {
                assert_eq!(protocol_version, PROTOCOL_VERSION);
            }
            other => panic!("expected hello, got {other:?}"),
        }

        host.write(&Event::HelloAck {
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
        host.write(&Event::RequestUserInput {
            prompt: Some("continue".into()),
            placeholder: None,
        })
        .unwrap();

        let second = connect_and_hello(&socket_path);
        std::thread::sleep(std::time::Duration::from_millis(100));
        host.write(&Event::Diagnostic {
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
    fn socket_host_delivers_large_request_events_without_dropping_connection() {
        let socket_path = temp_socket_path("socket-host-large-request");
        let mut host = SocketHost::bind(socket_path.clone()).unwrap();

        let first = connect_and_hello(&socket_path);
        match host.read().unwrap() {
            Some(HostEvent::Hello {
                protocol_version, ..
            }) => {
                assert_eq!(protocol_version, PROTOCOL_VERSION);
            }
            other => panic!("expected hello, got {other:?}"),
        }

        host.write(&Event::HelloAck {
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
        host.write(&Event::PhaseChanged {
            phase: "chat".into(),
        })
        .unwrap();
        let writer = std::thread::spawn(move || {
            host.write(&Event::RequestLlmResponse {
                request_id: "lr-1".into(),
                backend: "openai-compat".into(),
                model: Some("qwen/qwen3-coder-next".into()),
                model_family_id: Some("qwen3_6".into()),
                runtime_profile_id: Some("openai_compat_generic".into()),
                debug_adaptation: true,
                kind: crate::session::protocol::SessionKindOut::Work,
                messages: vec![LlmMessage {
                    role: LlmRole::System,
                    content: "x".repeat(32 * 1024),
                    attachments: Vec::new(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                }],
                tools: Vec::new(),
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
            Event::RequestLlmResponse {
                request_id,
                backend,
                model,
                messages,
                ..
            } => {
                assert_eq!(request_id, "lr-1");
                assert_eq!(backend, "openai-compat");
                assert_eq!(model.as_deref(), Some("qwen/qwen3-coder-next"));
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0].content.len(), 32 * 1024);
            }
            other => panic!("expected RequestLlmResponse, got {other:?}"),
        }

        writer.join().unwrap();
    }
}
