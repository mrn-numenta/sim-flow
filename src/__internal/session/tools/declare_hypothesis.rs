//! `declare_hypothesis(rationale)` -- log the agent's current
//! best guess about a bug's root cause into the open bug entry.
//! Pure logging: this does NOT signal the auto-iter classifier
//! (use `declare_fix` for that). It captures the trail so the
//! operator can see what the agent considered and ruled out.
//!
//! Targets the most-recently-opened OPEN bug in
//! `<project>/.sim-flow/bug-log.jsonl`. If no bug is open, the
//! tool returns an error pointing the agent at `log_bug`.

use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::Result;

pub struct DeclareHypothesisTool;

impl Tool for DeclareHypothesisTool {
    fn name(&self) -> &'static str {
        "declare_hypothesis"
    }
    fn description(&self) -> &'static str {
        "Append a hypothesis to the currently-open bug entry. One-line `rationale` describing what you think is wrong and why. Pure logging -- doesn't reset the no-progress counter; use `declare_fix` when you commit to actually attempting the fix. Requires an open bug from a prior `log_bug` call."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["rationale"],
            "properties": {
                "rationale": {
                    "type": "string",
                    "description": "One-line summary of your current best guess about the root cause."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        let rationale = match args.get("rationale").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => {
                return Ok(ToolResult::err(
                    "declare_hypothesis: missing or empty `rationale` arg",
                ));
            }
        };
        // Find the most-recently-opened bug whose status is still
        // "open". The bug log is append-only, so iterating in
        // reverse gives the freshest first.
        let records = crate::bug_log::load_all(ctx.project_dir);
        let Some(target) = records.iter().rev().find(|r| r.status == "open") else {
            return Ok(ToolResult::err(
                "declare_hypothesis: no open bug. Call `log_bug({\"issue\": ..., \"category\": ...})` first to open one.",
            ));
        };
        let ts = now_ts();
        let event = crate::bug_log::BugEvent {
            ts,
            kind: "hypothesis".to_string(),
            rationale: Some(rationale.clone()),
            outcome: None,
            message: None,
        };
        match crate::bug_log::append_event(ctx.project_dir, &target.id, event) {
            Ok(()) => Ok(ToolResult::ok(format!(
                "[declare_hypothesis -> {}] {rationale}",
                target.id
            ))),
            Err(err) => Ok(ToolResult::err(format!(
                "declare_hypothesis: could not append to bug log: {err}"
            ))),
        }
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
        ToolContext::new(dir, None, None, None).with_step_id("DM3c")
    }

    #[test]
    fn declare_hypothesis_appends_to_open_bug() {
        let tmp = tempfile::tempdir().unwrap();
        crate::bug_log::open(tmp.path(), "DM3c", None, "framework", "stress fails").unwrap();
        let result = DeclareHypothesisTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"rationale": "two-phase tick model"}),
            )
            .unwrap();
        assert!(result.ok, "{}", result.display);
        let records = crate::bug_log::load_all(tmp.path());
        assert_eq!(records[0].events.len(), 1);
        assert_eq!(records[0].events[0].kind, "hypothesis");
        assert_eq!(
            records[0].events[0].rationale.as_deref(),
            Some("two-phase tick model")
        );
    }

    #[test]
    fn declare_hypothesis_errors_when_no_open_bug() {
        let tmp = tempfile::tempdir().unwrap();
        let result = DeclareHypothesisTool
            .invoke(&ctx(tmp.path()), &json!({"rationale": "guess"}))
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("no open bug"));
    }

    #[test]
    fn declare_hypothesis_targets_most_recently_opened() {
        let tmp = tempfile::tempdir().unwrap();
        crate::bug_log::open(tmp.path(), "DM3c", None, "framework", "old").unwrap();
        let newer = crate::bug_log::open(tmp.path(), "DM3c", None, "test", "newer").unwrap();
        DeclareHypothesisTool
            .invoke(&ctx(tmp.path()), &json!({"rationale": "h"}))
            .unwrap();
        let records = crate::bug_log::load_all(tmp.path());
        let target = records.iter().find(|r| r.id == newer).unwrap();
        assert_eq!(target.events.len(), 1);
        // older bug untouched
        let older = records.iter().find(|r| r.id == "bug-001").unwrap();
        assert!(older.events.is_empty());
    }
}
