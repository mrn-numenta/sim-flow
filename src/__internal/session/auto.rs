//! Phase 2 auto driver: end-to-end work -> critique -> advance loop.
//!
//! `run_auto` drives a sequence of work and critique sessions for a
//! flow's remaining steps without user prompting. Each inner session
//! reuses the existing `run_session` entry point through an
//! `AutoHost` wrapper that:
//!
//! - synthesizes a `Hello` for every sub-session after the first (so
//!   the wrapped `run_session` thinks each iteration is a fresh
//!   handshake);
//! - swallows `SessionEnd` writes for every sub-session except the
//!   very last (the underlying host should see exactly one SessionEnd
//!   for the whole auto run);
//! - watches for the `max_auto_iters`-exceeded diagnostic so the
//!   driver knows when to stop calling further sub-sessions.
//!
//! Cross-session iteration: after each step's auto critique, the
//! driver re-reads the critique file. If it has any `BLOCKER:`
//! findings (per `parseFindings`-equivalent rules in
//! `extensions/.../state/critiques.ts`) and we haven't exceeded
//! `max_critique_iters`, we re-run the work session. `UNRESOLVED:`
//! findings are informational and do not trigger a re-run -- the
//! model uses that prefix for nits, follow-up notes, and questions
//! it does not consider must-fix. The orchestrator already inlines
//! the critique file via `build_session_inputs`, so the agent sees
//! the findings without further plumbing here.
//!
//! On exceeding the critique-iteration cap or hitting a structural
//! cap, the driver emits a `Diagnostic` and stops. The current chat
//! session continues interactively (the orchestrator has already
//! fallen through to RequestUserInput) until the user types
//! `/end-session`.

use std::path::{Path, PathBuf};

use crate::Result;
use crate::session::host::Host;
use crate::session::orchestrator::{OrchestratorOptions, run_session};
use crate::session::protocol::{
    DiagnosticLevel, Event, HostEvent, HostInfo, PROTOCOL_VERSION, StepMode,
};
use crate::state::State;
use crate::steps::registry_for;

/// Inputs for `run_auto`. The driver picks up the active flow's
/// remaining steps starting from `state.current_step`.
pub struct AutoOptions {
    pub project_dir: PathBuf,
    pub foundation_root: PathBuf,
    pub llm_backend: String,
    pub llm_model: Option<String>,
    /// Per-session structural-gate iteration cap (forwarded to the
    /// orchestrator's auto mode).
    pub max_auto_iters: u32,
    /// Cross-session retry cap. Each retry re-runs the work session
    /// for the same step (the orchestrator inlines the critique file
    /// so the agent sees what to fix).
    pub max_critique_iters: u32,
    /// If set, DM0 work runs in interactive mode (auto=false). The
    /// rest of the flow runs auto. Used when no spec has been
    /// provided -- the user collaborates on DM0, then auto takes
    /// over from DM0 critique onward.
    pub dm0_interactive: bool,
    /// Forwarded to each sub-session's `OrchestratorOptions`.
    /// Backstop runaway-loop guard; default 50.
    pub max_llm_requests: u32,
    /// Forwarded to each sub-session. Stuck-loop detection threshold;
    /// default 3.
    pub max_identical_responses: u32,
    /// Initial step-axis mode. `Auto` (current behavior) walks
    /// `current_step` to end of flow without user input. `Manual`
    /// parks the orchestrator after the hello handshake and
    /// dispatches sub-sessions only in response to host commands.
    /// The mode flag is also live-mutable mid-run via the
    /// `SetStepMode` host event; see
    /// `docs/brainstorming/manual-step-mode.md`. Wired into
    /// `AutoOptions` ahead of the run-loop refactor that consumes
    /// it; today's `run_auto` ignores this field and always behaves
    /// as if it were `Auto`.
    pub step_mode: StepMode,
}

pub fn run_auto<H: Host>(opts: AutoOptions, host: &mut H) -> Result<()> {
    let state = State::load(&opts.project_dir.join(".sim-flow"))?;
    let registry = registry_for(state.flow);
    let order = registry.order_for(state.flow);
    let starting = state.current_step.clone();
    let starting_idx = order.iter().position(|s| *s == starting).ok_or_else(|| {
        crate::Error::State(format!(
            "auto: current step `{starting}` is not in the {} flow",
            state.flow.as_str()
        ))
    })?;
    let remaining: Vec<&'static str> = order[starting_idx..].to_vec();

    let mut auto_host = AutoHost::new(host);

    // Sequence of "sub-session" plans. Materialized lazily because
    // each step may need an extra work iteration if critique reports
    // blockers. We track that with a per-step counter.
    'steps: for (step_pos, step_id) in remaining.iter().enumerate() {
        let is_first_step = step_pos == 0;
        let mut critique_iters: u32 = 0;

        loop {
            // Work session.
            let work_auto = !(opts.dm0_interactive && is_first_step && *step_id == "DM0");
            run_subsession(
                &opts,
                step_id,
                crate::client::SessionKind::Work,
                work_auto,
                &mut auto_host,
                /*is_final=*/ false,
                /*is_first=*/ is_first_step && critique_iters == 0,
            )?;
            if auto_host.cap_exceeded {
                emit_drop_to_interactive(
                    &mut auto_host,
                    step_id,
                    crate::client::SessionKind::Work,
                    opts.max_auto_iters,
                    opts.max_critique_iters,
                )?;
                break 'steps;
            }

            // Critique session.
            run_subsession(
                &opts,
                step_id,
                crate::client::SessionKind::Critique,
                /*auto=*/ true,
                &mut auto_host,
                /*is_final=*/ false,
                /*is_first=*/ false,
            )?;
            if auto_host.cap_exceeded {
                emit_drop_to_interactive(
                    &mut auto_host,
                    step_id,
                    crate::client::SessionKind::Critique,
                    opts.max_auto_iters,
                    opts.max_critique_iters,
                )?;
                break 'steps;
            }

            // Did the critique flag any blockers? If yes and we have
            // budget, loop back to work. Otherwise proceed to advance.
            let blockers = read_blockers(&opts.project_dir, step_id);
            if blockers.is_empty() {
                break;
            }
            critique_iters += 1;
            if critique_iters > opts.max_critique_iters {
                auto_host.write(&Event::Diagnostic {
                    level: DiagnosticLevel::Error,
                    message: format!(
                        "auto: {} critique still has {} blocker(s) after {} retries; pausing the auto run. \
                         To continue: click \"Run / Resume Automated Flow\" again on the dashboard \
                         (the driver picks up at `current_step` -- still {}). Raise \
                         `sim-flow.auto.maxCritiqueIterations` first if you want more retries per resume \
                         (current cap: {}).",
                        step_id,
                        blockers.len(),
                        critique_iters - 1,
                        step_id,
                        opts.max_critique_iters,
                    ),
                })?;
                break 'steps;
            }
            auto_host.write(&Event::Diagnostic {
                level: DiagnosticLevel::Info,
                message: format!(
                    "auto: {} critique reported {} blocker(s); re-running work (retry {}/{})",
                    step_id,
                    blockers.len(),
                    critique_iters,
                    opts.max_critique_iters,
                ),
            })?;
            // Loop body re-runs work; the orchestrator's
            // build_session_inputs will inline the critique file so
            // the agent sees the findings.
        }

        // Try to advance. If gate clean, mark passed + bump
        // current_step. If not clean, emit and stop -- something is
        // wrong (e.g. critique was clean but a structural check
        // regressed).
        let advanced = try_advance(&opts.project_dir, step_id, &mut auto_host)?;
        if !advanced {
            break 'steps;
        }
    }

    // Final SessionEnd. AutoHost forwards this to the underlying host
    // (consume_session_end starts true but is reset to false here so
    // the user sees a clean end-of-auto-run banner).
    auto_host.consume_session_end = false;
    auto_host.write(&Event::SessionEnd {
        reason: "completed".into(),
        message: Some("auto run finished".into()),
    })?;
    Ok(())
}

fn run_subsession<H: Host>(
    opts: &AutoOptions,
    step_id: &str,
    kind: crate::client::SessionKind,
    auto: bool,
    host: &mut AutoHost<H>,
    is_final: bool,
    is_first: bool,
) -> Result<()> {
    if !is_first {
        host.queue_synthetic_hello();
    }
    host.consume_session_end = !is_final;
    host.cap_exceeded = false;
    let session_opts = OrchestratorOptions {
        project_dir: opts.project_dir.clone(),
        foundation_root: opts.foundation_root.clone(),
        step_id: step_id.to_string(),
        kind,
        candidate: None,
        llm_backend: opts.llm_backend.clone(),
        llm_model: opts.llm_model.clone(),
        auto,
        max_auto_iters: opts.max_auto_iters,
        max_llm_requests: opts.max_llm_requests,
        max_identical_responses: opts.max_identical_responses,
        // JSONL host path: the orchestrator extracts fenced
        // ` ```<path>` blocks from each turn and writes them. Use
        // the artifact-write convention.
        agent_has_native_fs_tools: false,
    };
    run_session(session_opts, host)
}

fn try_advance<H: Host>(project_dir: &Path, step_id: &str, host: &mut AutoHost<H>) -> Result<bool> {
    use crate::gate;
    let dot = project_dir.join(".sim-flow");
    let mut state = State::load(&dot)?;
    let registry = registry_for(state.flow);
    let step = registry.get(step_id).ok_or_else(|| {
        crate::Error::InvalidStep(format!("{} is not a {} step", step_id, state.flow.as_str()))
    })?;
    let report = gate::evaluate(project_dir, &step.gate_checks)?;
    if !report.is_clean() {
        host.write(&Event::Diagnostic {
            level: DiagnosticLevel::Error,
            message: format!(
                "auto: {step_id} gate is not clean after critique; cannot advance. {} failure(s).",
                report.failures.len()
            ),
        })?;
        return Ok(false);
    }
    let order = registry.order_for(state.flow);
    let next = order
        .iter()
        .position(|s| *s == step.id)
        .and_then(|idx| order.get(idx + 1).copied());

    // Commit step artifacts before mutating sim-flow state so a
    // committed git history reflects each gate-clean checkpoint.
    let outcome = crate::git_commit::commit_step_advance(project_dir, step.id, next);
    if let Some(msg) = crate::git_commit::outcome_message(&outcome) {
        host.write(&Event::Diagnostic {
            level: DiagnosticLevel::Info,
            message: msg,
        })?;
    }

    state.mark_passed(step.id, current_iso8601());
    if let Some(next_step) = next {
        state.current_step = next_step.to_string();
    }
    state.save(&dot)?;
    host.write(&Event::StateAdvanced {
        from: step.id.into(),
        to: next.map(String::from),
    })?;
    Ok(true)
}

fn emit_drop_to_interactive<H: Host>(
    host: &mut AutoHost<H>,
    step_id: &str,
    kind: crate::client::SessionKind,
    max_auto_iters: u32,
    max_critique_iters: u32,
) -> Result<()> {
    let kind_s = match kind {
        crate::client::SessionKind::Work => "work",
        crate::client::SessionKind::Critique => "critique",
    };
    host.write(&Event::Diagnostic {
        level: DiagnosticLevel::Error,
        message: format!(
            "auto: {step_id} {kind_s} session hit the per-session iteration cap ({max_auto_iters}); pausing the auto run. \
             To continue: click \"Run / Resume Automated Flow\" again on the dashboard (the driver picks up at \
             `current_step` -- still {step_id}). Raise `sim-flow.auto.maxWorkIterations` first if you want more \
             work-side iterations per resume; the critique-side cap is {max_critique_iters}.",
        ),
    })?;
    Ok(())
}

/// Findings that prevent the gate from passing. Only `BLOCKER:`
/// lines block advancement. `UNRESOLVED:` is informational -- the
/// model uses it to flag notes/questions/follow-ups it does not
/// consider must-fix. The auto driver MUST match
/// `Finding::is_blocking` in `tools/sim-flow/src/critique.rs` (which
/// the gate's `CritiqueClean` check uses) or it will loop on issues
/// the gate would happily pass.
fn read_blockers(project_dir: &Path, step_id: &str) -> Vec<String> {
    let path = project_dir
        .join("docs/critiques")
        .join(format!("{step_id}-critique.md"));
    let body = match std::fs::read_to_string(&path) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let mut blockers = Vec::new();
    for line in body.lines() {
        let stripped = strip_finding_prefix(line);
        if let Some(text) = stripped.strip_prefix("BLOCKER:") {
            blockers.push(format!("BLOCKER: {}", text.trim()));
        }
    }
    blockers
}

/// Mirror of the extension's parser: tolerate leading whitespace,
/// list markers (`- `, `* `, `12. `), and bold-wrap (`**...**`).
fn strip_finding_prefix(raw: &str) -> String {
    let mut s = raw.trim_start().to_string();
    if let Some(rest) = strip_list_marker(&s) {
        s = rest;
    }
    if let Some(rest) = s.strip_prefix("**") {
        s = rest.to_string();
        if let Some(close) = s.rfind("**") {
            let mut tmp = String::with_capacity(s.len());
            tmp.push_str(&s[..close]);
            tmp.push_str(&s[close + 2..]);
            s = tmp;
        }
    }
    s
}

fn strip_list_marker(s: &str) -> Option<String> {
    if let Some(rest) = s.strip_prefix("- ") {
        return Some(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("* ") {
        return Some(rest.to_string());
    }
    let mut last_digit_end = None;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() {
            last_digit_end = Some(i + c.len_utf8());
        } else {
            break;
        }
    }
    if let Some(end) = last_digit_end
        && let Some(rest) = s[end..].strip_prefix(". ")
    {
        return Some(rest.to_string());
    }
    None
}

fn current_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Match main.rs's format-free unix epoch placeholder for now.
    format!("{}", dur.as_secs())
}

// ---------------------------------------------------------------------
// AutoHost wrapper
// ---------------------------------------------------------------------

/// `Host` wrapper that lets the auto driver run multiple back-to-back
/// `run_session` calls through one underlying connection.
pub struct AutoHost<'a, H: Host> {
    inner: &'a mut H,
    next_hello: Option<HostEvent>,
    /// Set true before each non-final sub-session; on the next
    /// SessionEnd write we swallow it instead of forwarding.
    pub consume_session_end: bool,
    /// Set when we observe a `max_auto_iters`-exceeded diagnostic so
    /// the driver can stop scheduling further sub-sessions.
    pub cap_exceeded: bool,
}

impl<'a, H: Host> AutoHost<'a, H> {
    pub fn new(inner: &'a mut H) -> Self {
        Self {
            inner,
            next_hello: None,
            consume_session_end: false,
            cap_exceeded: false,
        }
    }

    /// Queue a synthetic `Hello` event so the next sub-session's
    /// orchestrator handshake reads it instead of blocking on the
    /// underlying host.
    pub fn queue_synthetic_hello(&mut self) {
        self.next_hello = Some(HostEvent::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host: HostInfo {
                name: "sim-flow-auto".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            capabilities: vec!["text".into(), "user-input".into(), "llm-request".into()],
        });
    }
}

impl<H: Host> Host for AutoHost<'_, H> {
    fn write(&mut self, event: &Event) -> Result<()> {
        // Watch for the auto-cap diagnostic so the driver can stop.
        if let Event::Diagnostic { level, message } = event
            && matches!(level, DiagnosticLevel::Error)
            && message.contains("max_auto_iters")
        {
            self.cap_exceeded = true;
        }
        if self.consume_session_end && matches!(event, Event::SessionEnd { .. }) {
            self.consume_session_end = false;
            return Ok(());
        }
        self.inner.write(event)
    }

    fn read(&mut self) -> Result<Option<HostEvent>> {
        if let Some(h) = self.next_hello.take() {
            return Ok(Some(h));
        }
        self.inner.read()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_finding_prefix_handles_numbered_bold_markdown() {
        // Mirrors the extension's parser regression test in
        // critiques.test.ts ("handles numbered markdown lists with
        // bold-wrapped headings"). Keeps the two parsers in sync.
        let cases = [
            ("- BLOCKER: foo", "BLOCKER: foo"),
            ("* BLOCKER: foo", "BLOCKER: foo"),
            ("  - UNRESOLVED: bar", "UNRESOLVED: bar"),
            (
                "1. **BLOCKER: testbench file missing entirely.**",
                "BLOCKER: testbench file missing entirely.",
            ),
            ("12. **UNRESOLVED: ...**", "UNRESOLVED: ..."),
            ("BLOCKER: raw", "BLOCKER: raw"),
            ("not a finding", "not a finding"),
        ];
        for (input, expected) in cases {
            assert_eq!(strip_finding_prefix(input), expected, "input={input}");
        }
    }

    #[test]
    fn read_blockers_returns_empty_when_critique_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_blockers(tmp.path(), "DM0").is_empty());
    }

    #[test]
    fn read_blockers_extracts_only_blocker_findings() {
        // Only BLOCKER lines block advancement. UNRESOLVED lines are
        // informational notes the model wants to flag but does not
        // consider must-fix. RESOLVED lines are purely historical.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/critiques");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("DM0-critique.md"),
            "# DM0 Critique\n\n## Issues\n\n\
             1. **BLOCKER: missing clock frequency.**\n   ...\n\n\
             2. **UNRESOLVED: minor wording.**\n\n\
             3. **BLOCKER: bad pinout.**\n\n\
             4. RESOLVED: cleaned up.\n",
        )
        .unwrap();
        let blockers = read_blockers(tmp.path(), "DM0");
        assert_eq!(blockers.len(), 2);
        assert!(blockers[0].starts_with("BLOCKER: missing clock frequency"));
        assert!(blockers[1].starts_with("BLOCKER: bad pinout"));
    }

    #[test]
    fn read_blockers_treats_unresolved_only_critique_as_advanceable() {
        // A critique with only UNRESOLVED items must NOT block
        // advancement -- the driver previously looped back to work
        // for another iteration, burning iteration budget on
        // findings the model itself flagged as informational. With
        // BLOCKER as the only blocking prefix, the gate passes
        // cleanly and the step advances.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("docs/critiques");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("DM0-critique.md"),
            "# DM0 Critique\n\n## Issues\n\n\
             1. UNRESOLVED: minor wording nit.\n\n\
             2. UNRESOLVED: future cleanup note.\n",
        )
        .unwrap();
        let blockers = read_blockers(tmp.path(), "DM0");
        assert!(
            blockers.is_empty(),
            "UNRESOLVED-only critique should not produce blockers, got {blockers:?}",
        );
    }
}
