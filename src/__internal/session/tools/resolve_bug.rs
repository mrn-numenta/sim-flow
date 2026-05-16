//! `resolve_bug(resolution)` -- close the currently-open bug entry
//! with a narrative explaining what fixed it. Targets the most-
//! recently-opened OPEN bug (LIFO semantics).
//!
//! Use when a fix attempt succeeded (target failing-test set
//! shrank to empty) AND you're confident the root cause is
//! addressed. The bug is marked `status: resolved` and stays in
//! the log as historical record.

use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::Result;

pub struct ResolveBugTool;

impl Tool for ResolveBugTool {
    fn name(&self) -> &'static str {
        "resolve_bug"
    }
    fn description(&self) -> &'static str {
        "Mark the currently-open bug as resolved with a `resolution` narrative explaining what fixed it. Use after a `declare_fix` whose subsequent `cargo test` actually passed. The bug entry stays in the log; only its `status` flips from `open` to `resolved`."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["resolution"],
            "properties": {
                "resolution": {
                    "type": "string",
                    "description": "1-3 sentence summary of the root cause + what change resolved it."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        let resolution = match args.get("resolution").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => {
                return Ok(ToolResult::err(
                    "resolve_bug: missing or empty `resolution` arg",
                ));
            }
        };
        let records = crate::bug_log::load_all(ctx.project_dir);
        let Some(target) = records.iter().rev().find(|r| r.status == "open") else {
            return Ok(ToolResult::err(
                "resolve_bug: no open bug to resolve. The bug log is empty or every entry is already resolved.",
            ));
        };
        let id = target.id.clone();
        match crate::bug_log::resolve(ctx.project_dir, &id, &resolution, None) {
            Ok(()) => Ok(ToolResult::ok(format!(
                "[resolve_bug -> {id}] {resolution}"
            ))),
            Err(err) => Ok(ToolResult::err(format!(
                "resolve_bug: could not update bug log: {err}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(dir: &'a std::path::Path) -> ToolContext<'a> {
        ToolContext::new(dir, None, None, None).with_step_id("DM3c")
    }

    #[test]
    fn resolve_bug_closes_open_bug() {
        let tmp = tempfile::tempdir().unwrap();
        crate::bug_log::open(tmp.path(), "DM3c", None, "framework", "stress fails").unwrap();
        let result = ResolveBugTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"resolution": "raised injector rate to 1/cycle"}),
            )
            .unwrap();
        assert!(result.ok, "{}", result.display);
        let records = crate::bug_log::load_all(tmp.path());
        assert_eq!(records[0].status, "resolved");
        assert_eq!(
            records[0].resolution.as_deref(),
            Some("raised injector rate to 1/cycle")
        );
        assert!(records[0].closed_at.is_some());
    }

    #[test]
    fn resolve_bug_errors_when_no_open_bug() {
        let tmp = tempfile::tempdir().unwrap();
        let result = ResolveBugTool
            .invoke(&ctx(tmp.path()), &json!({"resolution": "n/a"}))
            .unwrap();
        assert!(!result.ok);
    }

    #[test]
    fn resolve_bug_rejects_missing_or_empty_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        crate::bug_log::open(tmp.path(), "DM3c", None, "framework", "x").unwrap();
        // No resolution arg.
        let result = ResolveBugTool.invoke(&ctx(tmp.path()), &json!({})).unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("missing or empty"));
        // Whitespace-only resolution.
        let result = ResolveBugTool
            .invoke(&ctx(tmp.path()), &json!({"resolution": "   \n"}))
            .unwrap();
        assert!(!result.ok);
    }

    #[test]
    fn resolve_bug_targets_most_recently_opened() {
        let tmp = tempfile::tempdir().unwrap();
        crate::bug_log::open(tmp.path(), "DM3c", None, "framework", "first").unwrap();
        crate::bug_log::open(tmp.path(), "DM3c", None, "test", "second").unwrap();
        ResolveBugTool
            .invoke(&ctx(tmp.path()), &json!({"resolution": "fixed second"}))
            .unwrap();
        let records = crate::bug_log::load_all(tmp.path());
        let first = records.iter().find(|r| r.id == "bug-001").unwrap();
        let second = records.iter().find(|r| r.id == "bug-002").unwrap();
        assert_eq!(first.status, "open"); // older bug untouched
        assert_eq!(second.status, "resolved");
    }
}
