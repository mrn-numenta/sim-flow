//! Append-only markdown debug log enabled by `SIM_FOUNDATION_DEBUG`.
//!
//! Categories selected via comma-separated tokens in the env var:
//!
//! - `events` -- parsed events both directions, with full message stack
//!   dumped on each `RequestLlmResponse`.
//! - `raw` -- JSONL form of every event (catches any drift between the
//!   parsed view and what would land on the wire).
//! - `llm` -- LLM dispatch detail; populated by the host-side renderer
//!   (extension), not the orchestrator. Recognized here so the parser
//!   doesn't warn.
//!
//! Shortcuts: `1` / `true` enable `events,llm`; `all` enables every
//! category. Unset / empty disables the log entirely; in that mode the
//! `DebugLog` returned by `open` is a no-op so calls in hot paths cost
//! a single boolean check.

use std::env;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::Result;
use crate::session::host::Host;
use crate::session::protocol::{Event, HostEvent, LlmRole};

#[derive(Debug, Clone, Copy, Default)]
pub struct CategorySet {
    pub events: bool,
    pub raw: bool,
    pub llm: bool,
}

impl CategorySet {
    pub fn any(self) -> bool {
        self.events || self.raw || self.llm
    }
}

pub fn parse_categories(raw: Option<&str>) -> CategorySet {
    let mut out = CategorySet::default();
    let raw = match raw {
        Some(s) => s.trim(),
        None => return out,
    };
    if raw.is_empty() {
        return out;
    }
    for token in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match token {
            "events" => out.events = true,
            "raw" => out.raw = true,
            "llm" => out.llm = true,
            "1" | "true" => {
                out.events = true;
                out.llm = true;
            }
            "all" => {
                out.events = true;
                out.raw = true;
                out.llm = true;
            }
            other => {
                tracing::warn!(token = %other, "ignoring unknown SIM_FOUNDATION_DEBUG token");
            }
        }
    }
    out
}

pub struct DebugLog {
    file: Option<Mutex<File>>,
    cats: CategorySet,
    start: Instant,
}

impl DebugLog {
    /// Open `<project_dir>/.sim-flow/logs/sim-flow-chat.log` if the
    /// env var requests any category. Failure to create the dir / open
    /// the file is reported on stderr and downgrades to a no-op log
    /// (we never want logging to fail the session).
    pub fn open(project_dir: &Path) -> Self {
        let cats = parse_categories(env::var("SIM_FOUNDATION_DEBUG").ok().as_deref());
        if !cats.any() {
            return Self::disabled(cats);
        }
        let dir = project_dir.join(".sim-flow").join("logs");
        if let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::warn!(
                dir = %dir.display(),
                error = %err,
                "cannot create debug log dir; debug logging disabled"
            );
            return Self::disabled(cats);
        }
        let path = dir.join("sim-flow-chat.log");
        let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => f,
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "cannot open debug log file; debug logging disabled"
                );
                return Self::disabled(cats);
            }
        };
        let _ = writeln!(file, "\n## Session started at {}\n", iso_now());
        Self {
            file: Some(Mutex::new(file)),
            cats,
            start: Instant::now(),
        }
    }

    fn disabled(cats: CategorySet) -> Self {
        Self {
            file: None,
            cats,
            start: Instant::now(),
        }
    }

    pub fn log_event_out(&self, event: &Event) {
        if !self.cats.events {
            return;
        }
        let Some(file) = self.file.as_ref() else {
            return;
        };
        let mut buf = String::new();
        if matches!(event, Event::RequestLlmResponse { .. }) {
            self.format_request_llm(event, &mut buf);
        } else {
            format_event_section(&mut buf, &self.elapsed(), "→", event_kind(event), event);
        }
        let _ = file.lock().unwrap().write_all(buf.as_bytes());
    }

    pub fn log_event_in(&self, event: &HostEvent) {
        if !self.cats.events {
            return;
        }
        let Some(file) = self.file.as_ref() else {
            return;
        };
        let mut buf = String::new();
        format_event_section(
            &mut buf,
            &self.elapsed(),
            "←",
            host_event_kind(event),
            event,
        );
        let _ = file.lock().unwrap().write_all(buf.as_bytes());
    }

    pub fn log_raw_out(&self, line: &str) {
        if !self.cats.raw {
            return;
        }
        self.write_raw("→", line);
    }

    pub fn log_raw_in(&self, line: &str) {
        if !self.cats.raw {
            return;
        }
        self.write_raw("←", line);
    }

    fn write_raw(&self, dir: &str, line: &str) {
        let Some(file) = self.file.as_ref() else {
            return;
        };
        let mut f = file.lock().unwrap();
        let _ = writeln!(f, "{} raw{} `{}`", self.elapsed(), dir, line.trim_end());
    }

    fn elapsed(&self) -> String {
        let d = self.start.elapsed();
        format!("[+{:>3}.{:03}s]", d.as_secs(), d.subsec_millis())
    }

    fn format_request_llm(&self, event: &Event, buf: &mut String) {
        use std::fmt::Write;
        let Event::RequestLlmResponse {
            request_id,
            backend,
            model,
            model_family_id,
            runtime_profile_id,
            debug_adaptation,
            messages,
            ..
        } = event
        else {
            return;
        };
        writeln!(
            buf,
            "### {} → RequestLlmResponse #{request_id}",
            self.elapsed()
        )
        .unwrap();
        writeln!(
            buf,
            "backend: `{backend}`  model: {}  family: {}  runtime: {}  debug-adaptation: {}",
            model.as_deref().unwrap_or("(default)"),
            model_family_id.as_deref().unwrap_or("(infer)"),
            runtime_profile_id.as_deref().unwrap_or("(default)"),
            if *debug_adaptation { "on" } else { "off" }
        )
        .unwrap();
        writeln!(buf, "{} message(s):\n", messages.len()).unwrap();
        for (i, m) in messages.iter().enumerate() {
            let role = match m.role {
                LlmRole::System => "system",
                LlmRole::User => "user",
                LlmRole::Assistant => "assistant",
                LlmRole::Tool => "tool",
            };
            writeln!(buf, "#### [{i}] {role}").unwrap();
            writeln!(buf, "```").unwrap();
            writeln!(buf, "{}", m.content).unwrap();
            writeln!(buf, "```\n").unwrap();
        }
    }
}

fn format_event_section<E: serde::Serialize>(
    buf: &mut String,
    elapsed: &str,
    dir: &str,
    kind: &str,
    event: &E,
) {
    use std::fmt::Write;
    writeln!(buf, "### {elapsed} {dir} {kind}").unwrap();
    writeln!(buf, "```json").unwrap();
    writeln!(
        buf,
        "{}",
        serde_json::to_string_pretty(event).unwrap_or_default()
    )
    .unwrap();
    writeln!(buf, "```\n").unwrap();
}

fn event_kind(event: &Event) -> &'static str {
    match event {
        Event::HelloAck { .. } => "HelloAck",
        Event::AssistantText { .. } => "AssistantText",
        Event::ArtifactWritten { .. } => "ArtifactWritten",
        Event::ToolInvoked { .. } => "ToolInvoked",
        Event::PhaseChanged { .. } => "PhaseChanged",
        Event::BuildOutput { .. } => "BuildOutput",
        Event::GateResult { .. } => "GateResult",
        Event::StateAdvanced { .. } => "StateAdvanced",
        Event::Followup { .. } => "Followup",
        Event::Diagnostic { .. } => "Diagnostic",
        Event::SessionEnd { .. } => "SessionEnd",
        Event::RequestUserInput { .. } => "RequestUserInput",
        Event::RequestLlmResponse { .. } => "RequestLlmResponse",
        Event::StepModeChanged { .. } => "StepModeChanged",
        Event::SubSessionStarted { .. } => "SubSessionStarted",
        Event::SubSessionEnded { .. } => "SubSessionEnded",
    }
}

fn host_event_kind(event: &HostEvent) -> &'static str {
    match event {
        HostEvent::Hello { .. } => "Hello",
        HostEvent::UserMessage { .. } => "UserMessage",
        HostEvent::Cancel => "Cancel",
        HostEvent::LlmChunk { .. } => "LlmChunk",
        HostEvent::LlmEnd { .. } => "LlmEnd",
        HostEvent::LlmError { .. } => "LlmError",
        HostEvent::FollowupSelected { .. } => "FollowupSelected",
        HostEvent::RunStep { .. } => "RunStep",
        HostEvent::RunCritique { .. } => "RunCritique",
        HostEvent::RunGate { .. } => "RunGate",
        HostEvent::Advance { .. } => "Advance",
        HostEvent::Reset { .. } => "Reset",
        HostEvent::SetStepMode { .. } => "SetStepMode",
        HostEvent::Shutdown => "Shutdown",
    }
}

/// Transparent `Host` wrapper that fans every event through a
/// `DebugLog` before forwarding to the inner host. When no debug
/// categories are enabled the log calls are early-exit no-ops, so
/// wrapping is essentially free.
pub struct LoggingHost<'a, H: Host> {
    inner: &'a mut H,
    log: &'a DebugLog,
}

impl<'a, H: Host> LoggingHost<'a, H> {
    pub fn new(inner: &'a mut H, log: &'a DebugLog) -> Self {
        Self { inner, log }
    }
}

impl<H: Host> Host for LoggingHost<'_, H> {
    fn write(&mut self, event: &Event) -> Result<()> {
        self.log.log_event_out(event);
        if let Ok(line) = serde_json::to_string(event) {
            self.log.log_raw_out(&line);
        }
        self.inner.write(event)
    }

    fn read(&mut self) -> Result<Option<HostEvent>> {
        let r = self.inner.read()?;
        if let Some(ref e) = r {
            self.log.log_event_in(e);
            if let Ok(line) = serde_json::to_string(e) {
                self.log.log_raw_in(&line);
            }
        }
        Ok(r)
    }
}

fn iso_now() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}Z (unix-epoch)", dur.as_secs(), dur.subsec_millis())
}

/// Append a session-exit marker to `<project>/.sim-flow/logs/sim-flow-chat.log`.
/// Called from `main.rs`'s error handler and panic hook so the log shows
/// *why* the process is leaving, even when no `DebugLog` instance is in
/// scope (`run_session` already returned with an error). No-op if the
/// log file doesn't exist or can't be opened — this path runs during
/// teardown and must never itself panic.
pub fn append_session_exit_marker(project_dir: &Path, reason: &str) {
    let path = project_dir
        .join(".sim-flow")
        .join("logs")
        .join("sim-flow-chat.log");
    let mut file = match OpenOptions::new().append(true).open(&path) {
        Ok(f) => f,
        Err(_) => return,
    };
    let _ = writeln!(file, "\n### {} session exited", iso_now());
    let _ = writeln!(file, "```");
    let _ = writeln!(file, "{reason}");
    let _ = writeln!(file, "```\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_categories_handles_known_tokens_and_shortcuts() {
        let none = parse_categories(None);
        assert!(!none.any());

        let empty = parse_categories(Some(""));
        assert!(!empty.any());

        let one = parse_categories(Some("events"));
        assert!(one.events && !one.raw && !one.llm);

        let combo = parse_categories(Some("events,raw,llm"));
        assert!(combo.events && combo.raw && combo.llm);

        let shortcut_one = parse_categories(Some("1"));
        assert!(shortcut_one.events && !shortcut_one.raw && shortcut_one.llm);

        let all = parse_categories(Some("all"));
        assert!(all.events && all.raw && all.llm);
    }

    #[test]
    fn unknown_token_does_not_panic() {
        let cats = parse_categories(Some("events,bogus"));
        assert!(cats.events);
    }
}
