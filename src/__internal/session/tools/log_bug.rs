//! `log_bug(issue, category)` -- open a bug entry in the project's
//! bug log. Subsequent `declare_hypothesis` / `declare_fix` /
//! `resolve_bug` calls in this session implicitly target the most-
//! recently-opened bug (LIFO stack maintained by the orchestrator).
//!
//! Use this when you encounter a distinct failure mode worth
//! tracking across sessions: a failing test cluster you'll
//! investigate, a critique blocker that recurs, a tooling problem
//! the auto-loop tripped on. The log accumulates across the
//! project's history so the operator can mine it for systemic
//! issues. ONE bug per distinct issue -- not one per turn.

use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::Result;

pub struct LogBugTool;

impl Tool for LogBugTool {
    fn name(&self) -> &'static str {
        "log_bug"
    }
    fn description(&self) -> &'static str {
        "Open a bug entry in the project's bug log (`<project>/.sim-flow/bug-log.jsonl`). \
         Pass a 1-2 sentence `issue` summary and a `category` from \
         {framework, test, impl, tooling, other}. Returns the new bug id. \
         Subsequent `declare_hypothesis` / `declare_fix` / `resolve_bug` \
         calls implicitly target this bug. Use ONE bug per distinct failure mode."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["issue", "category"],
            "properties": {
                "issue": {
                    "type": "string",
                    "description": "1-2 sentence summary of what's wrong."
                },
                "category": {
                    "type": "string",
                    "enum": ["framework", "test", "impl", "perf", "tooling", "other"],
                    "description": "Coarse-grained classification for later mining: framework (Foundation behavior), test (test code / expectations wrong), impl (model under test has a bug), perf (DM4* perf-target miss), tooling (cargo / verilator / external), other."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        let issue = match args.get("issue").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => return Ok(ToolResult::err("log_bug: missing or empty `issue` arg")),
        };
        let category = args
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("other")
            .trim()
            .to_string();
        let step = ctx.step_id.unwrap_or("?");
        let milestone = ctx.current_milestone_path;
        match crate::bug_log::open(ctx.project_dir, step, milestone, &category, &issue) {
            Ok(id) => {
                let milestone_label = milestone
                    .map(|m| format!(", milestone={m}"))
                    .unwrap_or_default();
                Ok(ToolResult::ok(format!(
                    "[log_bug] opened {id} (step={step}{milestone_label}, category={category}): {issue}. \
                     Subsequent declare_hypothesis / declare_fix / resolve_bug calls target this bug."
                )))
            }
            Err(err) => Ok(ToolResult::err(format!(
                "log_bug: could not write `.sim-flow/bug-log.jsonl`: {err}"
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
    fn log_bug_writes_record_and_reports_id() {
        let tmp = tempfile::tempdir().unwrap();
        let result = LogBugTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"issue": "stress fails at 0.5/cycle", "category": "framework"}),
            )
            .unwrap();
        assert!(result.ok, "{}", result.display);
        assert!(
            result.display.contains("bug-001"),
            "expected id in display, got: {}",
            result.display,
        );
        let records = crate::bug_log::load_all(tmp.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].step, "DM3c");
        assert_eq!(records[0].category, "framework");
    }

    #[test]
    fn log_bug_rejects_empty_issue() {
        let tmp = tempfile::tempdir().unwrap();
        let result = LogBugTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"issue": "", "category": "framework"}),
            )
            .unwrap();
        assert!(!result.ok);
    }
}
