//! Built-in [`Presenter`] transports: stdio JSONL and in-memory test
//! fixture.
//!
//! Originally home to the `Host` trait + `TerminalHost`. After the
//! Presenter / LlmAdapter split the orchestrator dispatches LLM calls
//! in-process via [`LlmAdapter`], so the user-facing surface only
//! needs to render events + collect user input -- exactly [`Presenter`].
//! The terminal-driven UI moved to [`super::stderr_presenter`].
//!
//! [`LlmAdapter`]: super::llm_adapter::LlmAdapter
//! [`Presenter`]: super::presenter::Presenter

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

use crate::session::presenter::Presenter;
use crate::session::protocol::{Event, HostEvent};
use crate::{Error, Result};

// ---------------------------------------------------------------------
// JsonlHost - the production transport for IDE / external hosts.
// ---------------------------------------------------------------------

/// Speaks line-delimited JSON over a pair of byte streams. Each line
/// is exactly one JSON object. `JsonlHost::stdio()` wires the host to
/// the process's stdin / stdout; tests can wire to in-memory pipes.
pub struct JsonlHost<R: Read, W: Write> {
    reader: BufReader<R>,
    writer: W,
    /// Shared sink for outgoing events. Reserved for the future
    /// streaming-LLM-chunk path; currently unused.
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

impl<R: Read, W: Write> Presenter for JsonlHost<R, W> {
    fn send(&mut self, event: &Event) -> Result<()> {
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

    fn recv(&mut self) -> Result<Option<HostEvent>> {
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

/// In-memory `Presenter` impl that records every event the
/// orchestrator writes and replays a scripted queue of `HostEvent`s
/// on `recv`. Tests build a script up front, run the orchestrator
/// against it, then inspect the recorded events to verify behavior.
///
/// Kept as `TestHost` for now since dozens of tests import it under
/// that name; the rename to `TestPresenter` is a follow-up cleanup.
#[derive(Debug, Default)]
pub struct TestHost {
    /// Scripted host-side responses, drained in FIFO order on each
    /// `recv()`. Exhaustion returns `Ok(None)` to signal channel close.
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
}

impl Presenter for TestHost {
    fn send(&mut self, event: &Event) -> Result<()> {
        self.written.push(event.clone());
        Ok(())
    }

    fn recv(&mut self) -> Result<Option<HostEvent>> {
        Ok(self.script.pop_front())
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
        let got = host.recv().unwrap().expect("hello on stdin");
        match got {
            HostEvent::Hello {
                protocol_version, ..
            } => assert_eq!(protocol_version, PROTOCOL_VERSION),
            other => panic!("expected Hello, got {:?}", other),
        }

        // Write an AssistantText. Verify it lands on the sink as JSONL.
        host.send(&Event::AssistantText {
            text: "hi".into(),
            final_chunk: true,
            tool_calls: Vec::new(),
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
        assert!(host.recv().unwrap().is_none());
    }

    #[test]
    fn jsonl_host_skips_blank_lines() {
        let payload = format!(
            "\n\n{}\n",
            serde_json::to_string(&HostEvent::Cancel).unwrap()
        );
        let mut writer: Vec<u8> = Vec::new();
        let mut host = JsonlHost::new(std::io::Cursor::new(payload.into_bytes()), &mut writer);
        match host.recv().unwrap() {
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

        host.send(&Event::AssistantText {
            text: "hello".into(),
            final_chunk: true,
            tool_calls: Vec::new(),
        })
        .unwrap();
        assert_eq!(host.written.len(), 1);

        match host.recv().unwrap() {
            Some(HostEvent::UserMessage { text }) => assert_eq!(text, "first"),
            other => panic!("expected user message, got {:?}", other),
        }
        match host.recv().unwrap() {
            Some(HostEvent::Cancel) => {}
            other => panic!("expected cancel, got {:?}", other),
        }
        assert!(host.recv().unwrap().is_none());
    }
}
