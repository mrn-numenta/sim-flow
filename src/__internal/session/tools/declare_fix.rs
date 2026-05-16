//! `declare_fix(rationale: string)` -- the agent's explicit commit
//! point. No filesystem effect; the tool just emits a structured
//! marker the orchestrator's no-progress classifier uses to score
//! the NEXT `cargo test` as a fix attempt regardless of the file-op
//! heuristic, AND counts against a separate `MAX_DECLARED_FIXES`
//! budget so repeated false starts still bail eventually.
//!
//! Motivation: the file-op heuristic (touched a path in the step's
//! pre-session manifest -> fix attempt) misclassifies two real
//! patterns:
//!   1. Agent emits a fix in a NEW file (e.g. refactored helper
//!      under `tests/testbench/` that didn't exist last session).
//!      File-op says "no touch of existing" -> Investigation,
//!      but the agent meant it as a fix.
//!   2. Agent doesn't edit anything before the test (its prior
//!      reasoning already pointed at the right change; it just runs
//!      cargo test to confirm). Heuristic miss-classifies as
//!      Investigation; the agent never gets credited for its attempt.
//!
//! `declare_fix` lets the agent be explicit. The orchestrator's
//! convention prompt teaches the agent: before each test run that
//! you EXPECT to pass, call `declare_fix({\"rationale\": \"one line:
//! what you changed and why\"})`. This is your commit; failed
//! declared fixes burn from your declared-fix budget (default 8).
//!
//! Composes with the existing classifier: a turn whose file-ops
//! ALREADY indicate a fix attempt (touched_existing_since_last_test)
//! is still a fix attempt; declare_fix doesn't have to be redundant
//! with that. The declared_fixes_count is a parallel counter only.

use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::Result;

pub struct DeclareFixTool;

impl Tool for DeclareFixTool {
    fn name(&self) -> &'static str {
        "declare_fix"
    }
    fn description(&self) -> &'static str {
        "Commit point: declare that this turn (or the next `run_cargo test`) is an intentional fix attempt rather than a diagnostic / investigation. Provide a one-line `rationale` describing what you changed and why. Each call consumes one slot of the session's declared-fix budget (default 8); when the budget is exhausted the auto-driver bails so the operator can intervene. Use it once per real attempt -- not for every probe."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["rationale"],
            "properties": {
                "rationale": {
                    "type": "string",
                    "description": "One-line summary of what you changed and why you expect it to fix the failing tests. Visible to the operator in the run log."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        let rationale = match args.get("rationale").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => {
                return Ok(ToolResult::err(
                    "declare_fix: missing or empty `rationale` arg. Pass `{\"rationale\": \"one-line summary of the fix\"}`.",
                ));
            }
        };
        // Best-effort: if there's an open bug entry, append the
        // fix_attempt to its event trail. Silent no-op when the
        // agent didn't call `log_bug` first -- declare_fix's
        // primary purpose (classifier signal) still works.
        let records = crate::bug_log::load_all(ctx.project_dir);
        let target_id = records
            .iter()
            .rev()
            .find(|r| r.status == "open")
            .map(|r| r.id.clone());
        let display = if let Some(id) = &target_id {
            let event = crate::bug_log::BugEvent {
                ts: now_ts(),
                kind: "fix_attempt".to_string(),
                rationale: Some(rationale.clone()),
                outcome: Some("pending".to_string()),
                message: None,
            };
            let _ = crate::bug_log::append_event(ctx.project_dir, id, event);
            format!("[declare_fix -> {id}] {rationale}")
        } else {
            format!("[declare_fix] {rationale}")
        };
        Ok(ToolResult::ok(display).with_declared_fix())
    }
}

fn now_ts() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(dir: &'a std::path::Path) -> ToolContext<'a> {
        ToolContext::new(dir, None, None, None)
    }

    #[test]
    fn declare_fix_records_rationale_and_sets_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let result = DeclareFixTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"rationale": "raised injection rate to 1/cycle"}),
            )
            .unwrap();
        assert!(result.ok, "{}", result.display);
        assert!(result.declared_fix, "declared_fix flag must be set");
        assert!(result.display.contains("raised injection rate"));
    }

    #[test]
    fn declare_fix_rejects_empty_rationale() {
        let tmp = tempfile::tempdir().unwrap();
        let result = DeclareFixTool
            .invoke(&ctx(tmp.path()), &json!({"rationale": ""}))
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("missing or empty"));
    }

    #[test]
    fn declare_fix_rejects_whitespace_rationale() {
        let tmp = tempfile::tempdir().unwrap();
        let result = DeclareFixTool
            .invoke(&ctx(tmp.path()), &json!({"rationale": "   \n  "}))
            .unwrap();
        assert!(!result.ok);
    }

    #[test]
    fn declare_fix_appends_event_to_open_bug_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".sim-flow")).unwrap();
        let id =
            crate::bug_log::open(tmp.path(), "DM3a", None, "test_failure", "x panics").unwrap();
        let result = DeclareFixTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"rationale": "guard against negative index"}),
            )
            .unwrap();
        assert!(result.ok);
        assert!(result.display.contains(&id));
        // The bug log should now carry a fix_attempt event.
        let records = crate::bug_log::load_all(tmp.path());
        let rec = records.iter().find(|r| r.id == id).expect("bug record");
        assert!(rec.events.iter().any(|e| e.kind == "fix_attempt"));
    }

    #[test]
    fn declare_fix_no_open_bug_omits_bug_id_in_display() {
        let tmp = tempfile::tempdir().unwrap();
        // No open bugs.
        let result = DeclareFixTool
            .invoke(&ctx(tmp.path()), &json!({"rationale": "tried something"}))
            .unwrap();
        assert!(result.ok);
        // Format is `[declare_fix] rationale` without a bug id prefix.
        assert!(result.display.starts_with("[declare_fix] "));
        assert!(!result.display.contains("->"));
    }
}
