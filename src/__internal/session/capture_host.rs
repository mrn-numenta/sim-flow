//! `Host` decorator that tees every protocol event to a JSONL file
//! while delegating reads + writes to the inner host. Used by the
//! `e2e_auto` / `e2e_manual` study harnesses (Phase 0 of the
//! model-robustness study) so we have a faithful, replayable
//! transcript of every (request, response) pair the orchestrator
//! exchanges with a backend.
//!
//! Per `docs/brainstorming/model-robustness-study.md`, each line is
//! a JSON object of the shape
//!
//! ```jsonl
//! {"ts": <unix_ms>, "dir": "out", "event": {...}}    // orchestrator -> host
//! {"ts": <unix_ms>, "dir": "in",  "event": {...}}    // host -> orchestrator
//! ```
//!
//! `dir = "out"` carries an `Event` (protocol enum the orchestrator
//! writes to the host). `dir = "in"` carries a `HostEvent` (commands
//! / chunks the host writes back). Both share the same outer envelope
//! so a single jq pass can split them.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::Result;
use crate::session::host::Host;
use crate::session::protocol::{Event, HostEvent};

/// JSONL capture sink. Shared so multiple decorators or background
/// threads can write through the same lock without interleaving
/// partial lines.
#[derive(Clone)]
pub struct JsonlCapture {
    inner: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl JsonlCapture {
    /// Bind a capture to a writer (typically a `BufWriter<File>`).
    pub fn new<W: Write + Send + 'static>(writer: W) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(writer))),
        }
    }

    fn write_line(&self, dir: &str, payload: serde_json::Value) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let line = json!({
            "ts": ts,
            "dir": dir,
            "event": payload,
        });
        // Capture is best-effort: a write failure (disk full,
        // closed pipe) must NOT take down the orchestrator. We
        // intentionally swallow errors here -- the protocol path
        // is what the orchestrator actually drives off of.
        if let Ok(mut guard) = self.inner.lock() {
            let _ = serde_json::to_writer(&mut *guard, &line);
            let _ = guard.write_all(b"\n");
            let _ = guard.flush();
        }
    }

    pub fn record_out(&self, event: &Event) {
        let payload = match serde_json::to_value(event) {
            Ok(v) => v,
            Err(err) => json!({"_capture_error": format!("{err}")}),
        };
        self.write_line("out", payload);
    }

    pub fn record_in(&self, event: &HostEvent) {
        let payload = match serde_json::to_value(event) {
            Ok(v) => v,
            Err(err) => json!({"_capture_error": format!("{err}")}),
        };
        self.write_line("in", payload);
    }

    /// Write a study-level marker (run start / end, parameters,
    /// trial index, etc.) so a single capture file can carry the
    /// metadata needed to interpret it standalone.
    pub fn record_meta(&self, meta: serde_json::Value) {
        self.write_line("meta", meta);
    }
}

/// `Host` decorator that tees every event in both directions to a
/// `JsonlCapture`. The inner host still owns the actual protocol
/// behavior; this wrapper is purely observational.
pub struct CaptureHost<H> {
    inner: H,
    capture: JsonlCapture,
}

impl<H: Host> CaptureHost<H> {
    pub fn new(inner: H, capture: JsonlCapture) -> Self {
        Self { inner, capture }
    }
}

impl<H: Host> Host for CaptureHost<H> {
    fn write(&mut self, event: &Event) -> Result<()> {
        self.capture.record_out(event);
        self.inner.write(event)
    }
    fn read(&mut self) -> Result<Option<HostEvent>> {
        let result = self.inner.read()?;
        if let Some(event) = &result {
            self.capture.record_in(event);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::host::TestHost;
    use crate::session::protocol::{Event, HostEvent, HostInfo, PROTOCOL_VERSION};
    use std::io::Cursor;
    use std::sync::Arc;

    /// Thin `Write` that pushes bytes into a shared `Vec<u8>`. Lets
    /// the test inspect the captured lines after `Drop`-ing the
    /// `CaptureHost`.
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn capture_host_records_writes_and_reads_in_order() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let capture = JsonlCapture::new(SharedBuf(buf.clone()));

        let mut inner = TestHost::new();
        inner.enqueue(HostEvent::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host: HostInfo {
                name: "test".into(),
                version: "0".into(),
            },
            capabilities: vec![],
        });
        let mut host = CaptureHost::new(inner, capture);

        // One write, then one read. Lines must land in temporal
        // order in the capture file.
        host.write(&Event::PhaseChanged {
            phase: "chat".into(),
        })
        .unwrap();
        let got = host.read().unwrap().expect("queued hello");
        assert!(matches!(got, HostEvent::Hello { .. }));

        drop(host);
        let body =
            String::from_utf8(Cursor::new(buf.lock().unwrap().clone()).into_inner()).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2, "expected one out + one in line");
        // First line: out, PhaseChanged.
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["dir"], "out");
        assert_eq!(first["event"]["event"], "phase-changed");
        // Second line: in, hello.
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["dir"], "in");
        assert_eq!(second["event"]["event"], "hello");
        // Timestamps monotonic (or equal at ms resolution).
        let t0 = first["ts"].as_u64().unwrap();
        let t1 = second["ts"].as_u64().unwrap();
        assert!(t1 >= t0);
    }

    #[test]
    fn capture_meta_marker_lands_as_dir_meta() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let capture = JsonlCapture::new(SharedBuf(buf.clone()));
        capture.record_meta(serde_json::json!({"kind": "run-start", "model": "qwen3-27b"}));
        drop(capture);
        let body = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(parsed["dir"], "meta");
        assert_eq!(parsed["event"]["model"], "qwen3-27b");
    }
}
