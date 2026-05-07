//! Host abstraction. The orchestrator emits typed `Event`s and reads
//! typed `HostEvent`s; concrete impls translate to/from a transport
//! (JSONL on stdio, in-memory for tests, or rendered terminal in
//! Phase 9 M4's TerminalHost).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

use crate::session::agent::CliAgent;
use crate::session::protocol::{DiagnosticLevel, Event, HostEvent, HostInfo, PROTOCOL_VERSION};
use crate::{Error, Result};

/// Trait every session host implements. Sync because the orchestrator
/// is sync; async hosts wrap a blocking adapter.
pub trait Host {
    /// Send an event to the host. Errors propagate; the orchestrator
    /// stops the session on a failed write.
    fn write(&mut self, event: &Event) -> Result<()>;

    /// Block waiting for the next event from the host. Returns
    /// `Ok(None)` when the host channel closes cleanly (EOF on stdio,
    /// queue drained on TestHost).
    fn read(&mut self) -> Result<Option<HostEvent>>;
}

// ---------------------------------------------------------------------
// JsonlHost - the production transport for IDE / external hosts.
// ---------------------------------------------------------------------

/// Speaks line-delimited JSON over a pair of byte streams. Each line
/// is exactly one JSON object. `JsonlHost::stdio()` wires the host to
/// the process's stdin / stdout; tests can wire to in-memory pipes.
pub struct JsonlHost<R: Read, W: Write> {
    reader: BufReader<R>,
    writer: W,
    /// Shared sink for outgoing events, used by `clone_writer` so the
    /// orchestrator can hand a write-only handle to a worker thread.
    /// Currently unused; reserved for the streaming-LLM-chunk path.
    _sink: Arc<Mutex<()>>,
}

impl JsonlHost<std::io::Stdin, std::io::Stdout> {
    /// Bind to process stdio. Used by `sim-flow session ... --jsonl`.
    pub fn stdio() -> Self {
        Self::new(std::io::stdin(), std::io::stdout())
    }
}

impl<R: Read, W: Write> JsonlHost<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            _sink: Arc::new(Mutex::new(())),
        }
    }
}

impl<R: Read, W: Write> Host for JsonlHost<R, W> {
    fn write(&mut self, event: &Event) -> Result<()> {
        let line = serde_json::to_string(event)
            .map_err(|e| Error::Protocol(format!("session: serialize event: {e}")))?;
        self.writer
            .write_all(line.as_bytes())
            .map_err(|e| Error::Protocol(format!("session: write: {e}")))?;
        self.writer
            .write_all(b"\n")
            .map_err(|e| Error::Protocol(format!("session: write: {e}")))?;
        self.writer
            .flush()
            .map_err(|e| Error::Protocol(format!("session: flush: {e}")))?;
        Ok(())
    }

    fn read(&mut self) -> Result<Option<HostEvent>> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = self
                .reader
                .read_line(&mut line)
                .map_err(|e| Error::Protocol(format!("session: read: {e}")))?;
            if n == 0 {
                return Ok(None);
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue; // skip blank lines (lenient parser)
            }
            let parsed: HostEvent = serde_json::from_str(trimmed)
                .map_err(|e| Error::Protocol(format!("session: parse host event: {e}")))?;
            return Ok(Some(parsed));
        }
    }
}

// ---------------------------------------------------------------------
// TestHost - in-memory recorder/scripter for unit tests.
// ---------------------------------------------------------------------

/// In-memory `Host` impl that records every event the orchestrator
/// writes and replays a scripted queue of `HostEvent`s on read. Tests
/// build a script up front, run the orchestrator against it, then
/// inspect the recorded events to verify behavior.
#[derive(Debug, Default)]
pub struct TestHost {
    /// Scripted host-side responses, drained in FIFO order on each
    /// `read()`. Exhaustion returns `Ok(None)` to signal channel close.
    pub script: std::collections::VecDeque<HostEvent>,
    /// Events emitted by the orchestrator during the session.
    pub written: Vec<Event>,
}

impl TestHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a host event onto the back of the scripted queue.
    pub fn enqueue(&mut self, event: HostEvent) -> &mut Self {
        self.script.push_back(event);
        self
    }

    /// Convenience: enqueue a complete LLM-response sequence
    /// (chunks + end) for the next `RequestLlmResponse`. The
    /// orchestrator's request id binding is verified by tests that
    /// peek at `written` to see the most-recent request id.
    pub fn enqueue_llm_response(
        &mut self,
        request_id: impl Into<String>,
        text: impl Into<String>,
    ) -> &mut Self {
        let request_id = request_id.into();
        self.enqueue(HostEvent::LlmChunk {
            request_id: request_id.clone(),
            text: text.into(),
        });
        self.enqueue(HostEvent::LlmEnd {
            request_id,
            stop_reason: Some("stop".into()),
        });
        self
    }
}

impl Host for TestHost {
    fn write(&mut self, event: &Event) -> Result<()> {
        self.written.push(event.clone());
        Ok(())
    }

    fn read(&mut self) -> Result<Option<HostEvent>> {
        Ok(self.script.pop_front())
    }
}

// ---------------------------------------------------------------------
// TerminalHost - in-process host for `sim-flow session ...` from a
// plain terminal. Renders events to stdout/stderr and dispatches
// `RequestLlmResponse` to a configurable `CliAgent` (Phase 9 M4).
// ---------------------------------------------------------------------

/// Drives a session interactively from a terminal. The host:
/// - synthesizes the `Hello` handshake (no external host is involved),
/// - renders orchestrator events to stdout/stderr in a human-readable
///   form,
/// - reads user replies from stdin,
/// - dispatches `RequestLlmResponse` to the configured `CliAgent` and
///   queues a `LlmChunk` + `LlmEnd` pair so the orchestrator's read
///   loop is unchanged.
///
/// Generic over the agent so tests can inject a `MockAgent`.
pub struct TerminalHost<A: CliAgent, R: BufRead, W: Write, E: Write> {
    agent: A,
    stdin: R,
    stdout: W,
    stderr: E,
    /// FIFO queue of host-bound events synthesized by the host
    /// itself (the initial Hello, LLM chunks generated by the agent).
    /// Drained ahead of stdin reads.
    pending: VecDeque<HostEvent>,
    /// Track whether the orchestrator's most-recent request expected
    /// a user reply (RequestUserInput) so we know to read stdin
    /// rather than spin.
    awaiting_user_input: bool,
    /// Skin used to render `AssistantText` markdown to stdout.
    skin: termimad::MadSkin,
    /// Accumulates assistant chunks until `final_chunk` so the whole
    /// turn is rendered as one markdown document (per-chunk rendering
    /// would split mid-block and corrupt fenced code / lists).
    assistant_buffer: String,
}

impl<A, R, W, E> TerminalHost<A, R, W, E>
where
    A: CliAgent,
    R: BufRead,
    W: Write,
    E: Write,
{
    pub fn new(agent: A, stdin: R, stdout: W, stderr: E) -> Self {
        let mut pending = VecDeque::new();
        // Synthesize the Hello so the orchestrator's handshake fires.
        pending.push_back(HostEvent::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host: HostInfo {
                name: "sim-flow-terminal".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            capabilities: vec![
                "text".into(),
                "markdown".into(),
                "user-input".into(),
                "llm-request".into(),
                "tool-notifications".into(),
            ],
        });
        Self {
            agent,
            stdin,
            stdout,
            stderr,
            pending,
            awaiting_user_input: false,
            skin: termimad::MadSkin::default(),
            assistant_buffer: String::new(),
        }
    }

    fn render_event(&mut self, event: &Event) -> Result<()> {
        match event {
            Event::HelloAck {
                session,
                step_descriptor,
                ..
            } => {
                let kind = match session.kind {
                    crate::session::protocol::SessionKindOut::Work => "work",
                    crate::session::protocol::SessionKindOut::Critique => "critique",
                };
                writeln!(
                    self.stderr,
                    "== {} {} session (backend: {}) ==",
                    session.step,
                    kind,
                    self.agent.name()
                )
                .map_err(write_err)?;
                writeln!(
                    self.stderr,
                    "   instruction: {}",
                    step_descriptor.instruction_path
                )
                .map_err(write_err)?;
            }
            Event::AssistantText { text, final_chunk } => {
                self.assistant_buffer.push_str(text);
                if *final_chunk {
                    let buffer = std::mem::take(&mut self.assistant_buffer);
                    if !buffer.is_empty() {
                        let rendered = format!("{}", self.skin.term_text(&buffer));
                        self.stdout
                            .write_all(rendered.as_bytes())
                            .map_err(write_err)?;
                        if !rendered.ends_with('\n') {
                            writeln!(self.stdout).map_err(write_err)?;
                        }
                        self.stdout.flush().map_err(write_err)?;
                    }
                }
            }
            Event::ArtifactWritten { path, bytes } => {
                writeln!(self.stderr, "  [wrote {path} ({bytes} bytes)]").map_err(write_err)?;
            }
            Event::ToolInvoked {
                name,
                args_summary,
                status,
                duration_ms,
            } => {
                writeln!(
                    self.stderr,
                    "  [tool {name} {args_summary} -> {status} ({duration_ms} ms)]"
                )
                .map_err(write_err)?;
            }
            Event::PhaseChanged { phase } => {
                writeln!(self.stderr, "-- phase: {phase} --").map_err(write_err)?;
            }
            Event::BuildOutput {
                command, exit_code, ..
            } => {
                writeln!(self.stderr, "  [{command} -> exit {exit_code}]").map_err(write_err)?;
            }
            Event::GateResult {
                step,
                clean,
                failures,
            } => {
                if *clean {
                    writeln!(self.stderr, "  [gate {step}: clean]").map_err(write_err)?;
                } else {
                    writeln!(
                        self.stderr,
                        "  [gate {step}: {} failure(s)]",
                        failures.len()
                    )
                    .map_err(write_err)?;
                    for f in failures {
                        writeln!(self.stderr, "    - {}: {}", f.description, f.reason)
                            .map_err(write_err)?;
                    }
                }
            }
            Event::StateAdvanced { from, to } => {
                writeln!(
                    self.stderr,
                    "  [advanced past {from}{}]",
                    to.as_ref()
                        .map(|t| format!("; current step is now {t}"))
                        .unwrap_or_default()
                )
                .map_err(write_err)?;
            }
            Event::Followup { label, action } => {
                writeln!(self.stderr, "  [followup: {label} ({action})]").map_err(write_err)?;
            }
            Event::Diagnostic { level, message } => {
                let tag = match level {
                    DiagnosticLevel::Info => "info",
                    DiagnosticLevel::Warning => "warn",
                    DiagnosticLevel::Error => "error",
                };
                writeln!(self.stderr, "  [{tag}] {message}").map_err(write_err)?;
            }
            Event::SessionEnd { reason, message } => {
                let detail = message
                    .as_ref()
                    .map(|m| format!(": {m}"))
                    .unwrap_or_default();
                writeln!(self.stderr, "== session end ({reason}){detail} ==").map_err(write_err)?;
            }
            Event::RequestUserInput { prompt, .. } => {
                if let Some(p) = prompt {
                    writeln!(self.stderr, "{p}").map_err(write_err)?;
                }
                write!(self.stderr, "> ").map_err(write_err)?;
                self.stderr.flush().map_err(write_err)?;
                self.awaiting_user_input = true;
            }
            Event::RequestLlmResponse {
                request_id,
                messages,
                ..
            } => {
                writeln!(self.stderr, "  [thinking via {}...]", self.agent.name())
                    .map_err(write_err)?;
                self.stderr.flush().map_err(write_err)?;
                match self.agent.dispatch(messages) {
                    Ok((text, metrics)) => {
                        // Per-call metrics: token usage (when the
                        // backend reports it) + wall time. Live
                        // visibility via `RUST_LOG=sim_flow::metrics=info`;
                        // aggregation across a sub-session happens
                        // upstream in the orchestrator / auto driver
                        // by adding up these fields.
                        tracing::info!(
                            target: "sim_flow::metrics",
                            event = "llm_call",
                            request_id = %request_id,
                            agent = %self.agent.name(),
                            tokens_in = ?metrics.tokens_in,
                            tokens_out = ?metrics.tokens_out,
                            wall_ms = metrics.wall_ms,
                            content_bytes = text.len(),
                        );
                        // Synthesize the chunk + end pair the
                        // orchestrator's loop expects.
                        self.pending.push_back(HostEvent::LlmChunk {
                            request_id: request_id.clone(),
                            text,
                        });
                        self.pending.push_back(HostEvent::LlmEnd {
                            request_id: request_id.clone(),
                            stop_reason: Some("stop".into()),
                        });
                    }
                    Err(err) => {
                        self.pending.push_back(HostEvent::LlmError {
                            request_id: request_id.clone(),
                            kind: "agent-failed".into(),
                            message: format!("{err}"),
                        });
                    }
                }
            }
            Event::StepModeChanged { mode } => {
                writeln!(self.stderr, "  [step mode now: {mode:?}]").map_err(write_err)?;
            }
            Event::SubSessionStarted { step, kind } => {
                writeln!(self.stderr, "  [sub-session started: {step}.{kind:?}]")
                    .map_err(write_err)?;
            }
            Event::SubSessionEnded {
                step,
                kind,
                outcome,
            } => {
                writeln!(
                    self.stderr,
                    "  [sub-session ended: {step}.{kind:?} ({outcome})]"
                )
                .map_err(write_err)?;
            }
        }
        Ok(())
    }

    fn read_user_line(&mut self) -> Result<Option<String>> {
        let mut line = String::new();
        let n = self
            .stdin
            .read_line(&mut line)
            .map_err(|err| Error::State(format!("session: read from terminal stdin: {err}")))?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
        Ok(Some(trimmed))
    }
}

fn write_err(err: std::io::Error) -> Error {
    Error::State(format!("session: terminal write: {err}"))
}

impl<A, R, W, E> Host for TerminalHost<A, R, W, E>
where
    A: CliAgent,
    R: BufRead,
    W: Write,
    E: Write,
{
    fn write(&mut self, event: &Event) -> Result<()> {
        self.render_event(event)
    }

    fn read(&mut self) -> Result<Option<HostEvent>> {
        if let Some(e) = self.pending.pop_front() {
            return Ok(Some(e));
        }
        if !self.awaiting_user_input {
            // No queued events and the orchestrator isn't expecting
            // user input right now (we're between turns). The
            // orchestrator should always be in one of these two
            // states; if not, we treat it as channel close.
            return Ok(None);
        }
        self.awaiting_user_input = false;
        match self.read_user_line()? {
            Some(text) if text.is_empty() => Ok(Some(HostEvent::Cancel)),
            Some(text) => Ok(Some(HostEvent::UserMessage { text })),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::{HostInfo, PROTOCOL_VERSION};

    #[test]
    fn jsonl_host_round_trips_a_single_event_pair() {
        let host_msg = HostEvent::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host: HostInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            },
            capabilities: vec!["text".into()],
        };
        let input = serde_json::to_string(&host_msg).unwrap() + "\n";

        let reader = std::io::Cursor::new(input.into_bytes());
        let mut writer: Vec<u8> = Vec::new();
        let mut host = JsonlHost::new(reader, &mut writer);

        // Read the Hello.
        let got = host.read().unwrap().expect("hello on stdin");
        match got {
            HostEvent::Hello {
                protocol_version, ..
            } => assert_eq!(protocol_version, PROTOCOL_VERSION),
            other => panic!("expected Hello, got {:?}", other),
        }

        // Write an AssistantText. Verify it lands on the sink as JSONL.
        host.write(&Event::AssistantText {
            text: "hi".into(),
            final_chunk: true,
        })
        .unwrap();
        drop(host);
        let out = String::from_utf8(writer).unwrap();
        assert!(out.contains("\"event\":\"assistant-text\""));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn jsonl_host_returns_none_on_eof() {
        let reader: &[u8] = b"";
        let mut writer: Vec<u8> = Vec::new();
        let mut host = JsonlHost::new(reader, &mut writer);
        assert!(host.read().unwrap().is_none());
    }

    #[test]
    fn jsonl_host_skips_blank_lines() {
        let payload = format!(
            "\n\n{}\n",
            serde_json::to_string(&HostEvent::Cancel).unwrap()
        );
        let mut writer: Vec<u8> = Vec::new();
        let mut host = JsonlHost::new(std::io::Cursor::new(payload.into_bytes()), &mut writer);
        match host.read().unwrap() {
            Some(HostEvent::Cancel) => {}
            other => panic!("expected Cancel, got {:?}", other),
        }
    }

    #[test]
    fn test_host_records_writes_and_drains_script_in_order() {
        let mut host = TestHost::new();
        host.enqueue(HostEvent::UserMessage {
            text: "first".into(),
        })
        .enqueue(HostEvent::Cancel);

        host.write(&Event::AssistantText {
            text: "hello".into(),
            final_chunk: true,
        })
        .unwrap();
        assert_eq!(host.written.len(), 1);

        match host.read().unwrap() {
            Some(HostEvent::UserMessage { text }) => assert_eq!(text, "first"),
            other => panic!("expected user message, got {:?}", other),
        }
        match host.read().unwrap() {
            Some(HostEvent::Cancel) => {}
            other => panic!("expected cancel, got {:?}", other),
        }
        assert!(host.read().unwrap().is_none());
    }
}
