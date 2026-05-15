//! Terminal-driven [`Presenter`] for `sim-flow auto` / `sim-flow session`
//! when there's no IDE attached -- renders events to stderr (with
//! markdown via termimad on stdout for `AssistantText`) and reads
//! user replies from stdin.
//!
//! Pulled out of `TerminalHost` (see [`super::host`]) as part of the
//! presenter / LLM-adapter split: the legacy `TerminalHost` also owned
//! a [`CliAgent`] so it could dispatch `Event::RequestLlmResponse`
//! synchronously. After the rewire the orchestrator dispatches via
//! [`LlmAdapter`] directly, so the terminal surface only needs to
//! present events and collect user input -- exactly what `Presenter`
//! requires.
//!
//! [`CliAgent`]: super::agent::CliAgent
//! [`LlmAdapter`]: super::llm_adapter::LlmAdapter
//! [`Presenter`]: super::presenter::Presenter

use std::collections::VecDeque;
use std::io::{BufRead, Write};

use crate::session::presenter::Presenter;
use crate::session::protocol::{DiagnosticLevel, Event, HostEvent, HostInfo, PROTOCOL_VERSION};
use crate::{Error, Result};

/// Renders orchestrator [`Event`]s to a stderr / stdout pair and
/// reads user replies from stdin. Generic over the byte streams so
/// tests can drive it with in-memory buffers.
///
/// The presenter does NOT own an LLM agent -- LLM dispatch is the
/// orchestrator's responsibility now via [`LlmAdapter`]. Binary entry
/// points construct the agent separately and pass it alongside the
/// presenter.
///
/// [`LlmAdapter`]: super::llm_adapter::LlmAdapter
pub struct StderrPresenter<R: BufRead, W: Write, E: Write> {
    /// Short label for the LLM backend driving this session
    /// (e.g. "openai-compat", "claude"). Shown in the session header
    /// alongside the step / kind so the user sees which backend is
    /// running. Purely informational; can be empty.
    backend_label: String,
    stdin: R,
    stdout: W,
    stderr: E,
    /// FIFO queue of host-bound events synthesized by the presenter
    /// itself (currently just the initial Hello). Drained ahead of
    /// stdin reads.
    pending: VecDeque<HostEvent>,
    /// Set after a `RequestUserInput` event lands so the next `recv`
    /// blocks on stdin rather than returning `None`.
    awaiting_user_input: bool,
    /// Skin used to render `AssistantText` markdown to stdout.
    skin: termimad::MadSkin,
    /// Accumulates assistant chunks until `final_chunk` so the whole
    /// turn is rendered as one markdown document (per-chunk rendering
    /// would split mid-block and corrupt fenced code / lists).
    assistant_buffer: String,
}

impl<R, W, E> StderrPresenter<R, W, E>
where
    R: BufRead,
    W: Write,
    E: Write,
{
    pub fn new(backend_label: impl Into<String>, stdin: R, stdout: W, stderr: E) -> Self {
        let mut pending = VecDeque::new();
        // Synthesize the Hello so the orchestrator's handshake fires
        // without anybody on the wire.
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
                "tool-notifications".into(),
            ],
        });
        Self {
            backend_label: backend_label.into(),
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
                    crate::session::protocol::SessionKindOut::Qa => "qa",
                };
                writeln!(
                    self.stderr,
                    "== {} {} session (backend: {}) ==",
                    session.step, kind, self.backend_label,
                )
                .map_err(write_err)?;
                writeln!(
                    self.stderr,
                    "   instruction: {}",
                    step_descriptor.instruction_path
                )
                .map_err(write_err)?;
            }
            Event::AssistantText {
                text, final_chunk, ..
            } => {
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
            Event::LlmRequest { .. } => {
                // Experimental chat-panel feature; the stderr presenter
                // intentionally stays out of the way -- printing each
                // LLM-bound message would dominate terminal sessions.
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

impl<R, W, E> Presenter for StderrPresenter<R, W, E>
where
    R: BufRead,
    W: Write,
    E: Write,
{
    fn send(&mut self, event: &Event) -> Result<()> {
        self.render_event(event)
    }

    fn recv(&mut self) -> Result<Option<HostEvent>> {
        if let Some(e) = self.pending.pop_front() {
            return Ok(Some(e));
        }
        if !self.awaiting_user_input {
            // No queued events and the orchestrator isn't expecting
            // user input right now (we're between turns). Match
            // `TerminalHost::read`'s behavior: treat as channel close.
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
    use crate::session::protocol::{
        DiagnosticLevel, SessionEndReason, SessionKindOut, SessionTag, StepDescriptorOut,
    };

    fn dummy_step_descriptor() -> StepDescriptorOut {
        StepDescriptorOut {
            step: "DM2a".into(),
            kind: SessionKindOut::Work,
            flow: "ai".into(),
            prerequisite: None,
            instruction_path: "<test>".into(),
            work_artifacts: Vec::new(),
            predecessor_inputs: Vec::new(),
            per_candidate: false,
            phases: Vec::new(),
            tools: Vec::new(),
        }
    }

    fn make_presenter() -> StderrPresenter<&'static [u8], Vec<u8>, Vec<u8>> {
        StderrPresenter::new("mock", &b""[..], Vec::new(), Vec::new())
    }

    #[test]
    fn synthesizes_initial_hello_on_first_recv() {
        let mut p = make_presenter();
        match p.recv().unwrap() {
            Some(HostEvent::Hello {
                protocol_version, ..
            }) => assert_eq!(protocol_version, PROTOCOL_VERSION),
            other => panic!("expected synthesized Hello, got {other:?}"),
        }
        // After Hello: queue drained, not awaiting input -> channel close.
        assert!(p.recv().unwrap().is_none());
    }

    #[test]
    fn renders_session_header_on_hello_ack() {
        let mut p = make_presenter();
        p.send(&Event::HelloAck {
            protocol_version: PROTOCOL_VERSION.into(),
            sim_flow_version: "0.0.0".into(),
            session: SessionTag {
                step: "DM2a".into(),
                kind: SessionKindOut::Work,
                candidate: None,
            },
            step_descriptor: dummy_step_descriptor(),
        })
        .unwrap();
        let out = String::from_utf8(p.stderr.clone()).unwrap();
        assert!(out.contains("DM2a work session"), "got: {out}");
        assert!(out.contains("backend: mock"), "got: {out}");
    }

    #[test]
    fn returns_user_message_after_request_user_input() {
        let stdin: &[u8] = b"yes please\n";
        let mut p = StderrPresenter::new("mock", stdin, Vec::new(), Vec::new());
        // Drain synthesized Hello.
        let _ = p.recv().unwrap();
        p.send(&Event::RequestUserInput {
            prompt: Some("ok?".into()),
            placeholder: None,
        })
        .unwrap();
        match p.recv().unwrap() {
            Some(HostEvent::UserMessage { text }) => assert_eq!(text, "yes please"),
            other => panic!("expected UserMessage, got {other:?}"),
        }
    }

    #[test]
    fn empty_user_input_translates_to_cancel() {
        let stdin: &[u8] = b"\n";
        let mut p = StderrPresenter::new("mock", stdin, Vec::new(), Vec::new());
        let _ = p.recv().unwrap();
        p.send(&Event::RequestUserInput {
            prompt: None,
            placeholder: None,
        })
        .unwrap();
        match p.recv().unwrap() {
            Some(HostEvent::Cancel) => {}
            other => panic!("expected Cancel, got {other:?}"),
        }
    }

    #[test]
    fn renders_diagnostic_with_level_tag() {
        let mut p = make_presenter();
        p.send(&Event::Diagnostic {
            level: DiagnosticLevel::Warning,
            message: "watch out".into(),
        })
        .unwrap();
        let out = String::from_utf8(p.stderr.clone()).unwrap();
        assert!(out.contains("[warn] watch out"), "got: {out}");
    }

    #[test]
    fn renders_session_end_with_reason_and_message() {
        let mut p = make_presenter();
        p.send(&Event::SessionEnd {
            reason: SessionEndReason::Completed,
            message: Some("all clean".into()),
        })
        .unwrap();
        let out = String::from_utf8(p.stderr.clone()).unwrap();
        assert!(
            out.contains("session end (completed): all clean"),
            "got: {out}",
        );
    }
}
