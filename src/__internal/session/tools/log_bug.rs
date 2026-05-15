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
use crate::__internal::bug_log::{BugCategory, normalize_category};
use crate::Result;

pub struct LogBugTool;

impl LogBugTool {
    /// Build the args-schema `enum` list of canonical category names
    /// straight from `BugCategory::ALL` so the schema and the
    /// normalizer can't drift.
    fn category_enum_values() -> Vec<serde_json::Value> {
        BugCategory::ALL
            .iter()
            .map(|c| serde_json::Value::String(c.as_canonical_str().to_string()))
            .collect()
    }

    fn category_description() -> String {
        format!(
            "Coarse-grained classification for cross-project rollups. \
             Allowed values: {}. \
             Pick the closest match; `other` is the escape hatch but \
             the operator-facing critique flags `other`-heavy logs.",
            BugCategory::ALL
                .iter()
                .map(|c| format!("`{}`", c.as_canonical_str()))
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
}

impl Tool for LogBugTool {
    fn name(&self) -> &'static str {
        "log_bug"
    }
    fn description(&self) -> &'static str {
        "Open a bug entry in the project's bug log \
         (`<project>/.sim-flow/bug-log.jsonl`). Pass a 1-2 sentence \
         `issue` summary and a `category` from the closed taxonomy \
         (see args-schema). Returns the new bug id. Subsequent \
         `declare_hypothesis` / `declare_fix` / `resolve_bug` calls \
         implicitly target this bug. Use ONE bug per distinct failure mode."
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
                    "enum": Self::category_enum_values(),
                    "description": Self::category_description(),
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        let issue = match args.get("issue").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => return Ok(ToolResult::err("log_bug: missing or empty `issue` arg")),
        };
        let raw_category = match args.get("category").and_then(|v| v.as_str()) {
            Some(s) => s.trim(),
            None => {
                return Ok(ToolResult::err(format!(
                    "log_bug: missing `category` arg. Allowed values: {}.",
                    BugCategory::ALL
                        .iter()
                        .map(|c| c.as_canonical_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        };
        let canonical = match normalize_category(raw_category) {
            Some(c) => c,
            None => {
                return Ok(ToolResult::err(format!(
                    "log_bug: unknown category {raw_category:?}. Allowed values: {}.",
                    BugCategory::ALL
                        .iter()
                        .map(|c| c.as_canonical_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        };
        let step = ctx.step_id.unwrap_or("?");
        let milestone = ctx.current_milestone_path;
        match crate::bug_log::open(ctx.project_dir, step, milestone, canonical, &issue) {
            Ok(id) => {
                let milestone_label = milestone
                    .map(|m| format!(", milestone={m}"))
                    .unwrap_or_default();
                Ok(ToolResult::ok(format!(
                    "[log_bug] opened {id} (step={step}{milestone_label}, category={canonical}): {issue}. \
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
    fn log_bug_writes_record_with_canonical_category() {
        let tmp = tempfile::tempdir().unwrap();
        let result = LogBugTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"issue": "stress fails at 0.5/cycle", "category": "framework_misuse"}),
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
        assert_eq!(records[0].category, "framework_misuse");
    }

    #[test]
    fn log_bug_normalizes_legacy_category_to_canonical() {
        // Legacy short name `framework` survives as input but lands on
        // disk as the canonical `framework_misuse`, so cross-project
        // rollups don't see two tags for the same concept.
        let tmp = tempfile::tempdir().unwrap();
        let result = LogBugTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"issue": "foundation surprise", "category": "framework"}),
            )
            .unwrap();
        assert!(result.ok, "{}", result.display);
        let records = crate::bug_log::load_all(tmp.path());
        assert_eq!(records[0].category, "framework_misuse");
    }

    #[test]
    fn log_bug_rejects_unknown_category() {
        let tmp = tempfile::tempdir().unwrap();
        let result = LogBugTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"issue": "something", "category": "not-a-real-category"}),
            )
            .unwrap();
        assert!(!result.ok);
        assert!(
            result.display.contains("unknown category"),
            "expected guidance in error: {}",
            result.display
        );
    }

    #[test]
    fn log_bug_rejects_empty_issue() {
        let tmp = tempfile::tempdir().unwrap();
        let result = LogBugTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"issue": "", "category": "framework_misuse"}),
            )
            .unwrap();
        assert!(!result.ok);
    }
}
