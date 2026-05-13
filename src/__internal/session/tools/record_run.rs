//! `record_run(description, workload?, candidate?, study?, manifest_path?, notes?)`
//! -- record a completed simulation run into the project's
//! experiments index (`<project>/.sim-flow/experiments.db`). The
//! tool wraps `tracking::run_recording::record_run`, allocates a
//! `<sequence>-<description-slug>` run id, and materializes the
//! per-run artifact directory under `<project>/.experiments/<run-id>/`.
//!
//! Used by DM4b (Performance Analysis): after each `run_cargo
//! {command: "run", binary_args: ["--run-id", ...]}` invocation, the
//! agent calls this tool to log the run with its metadata. The
//! critique then verifies that every run-id cited in the perf
//! reports has a matching row in `experiments.db`.
//!
//! Deliberately does NOT run the binary itself -- the agent has
//! already run it via `run_cargo`. This tool is bookkeeping only,
//! so the agent's pattern is `run_cargo({run, ...}) -> record_run({...})`.

use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::Result;

pub struct RecordRunTool;

impl Tool for RecordRunTool {
    fn name(&self) -> &'static str {
        "record_run"
    }
    fn description(&self) -> &'static str {
        "Record a completed simulation run into the project's experiments index \
         (`.sim-flow/experiments.db`). Call AFTER `run_cargo({command: \"run\", \
         binary_args: [\"--run-id\", \"...\"]})` so the run-id you used is tracked. \
         `description` is the short slug that builds the run_id (e.g. \
         \"baseline-1k-burst\"); optional fields (`workload`, `candidate`, \
         `study`, `manifest_path`, `notes`) attach metadata that downstream \
         reports and sweeps can query."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["description"],
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Short description that becomes the run_id suffix. Lowercase, kebab-case (e.g. \"baseline-1k-burst\", \"sweep-fifo-depth-32\")."
                },
                "workload": {
                    "type": "string",
                    "description": "Workload tag (e.g. \"random-1k-burst\"). Optional."
                },
                "candidate": {
                    "type": "string",
                    "description": "Candidate / variant tag (e.g. \"priority-arbiter\", \"baseline\"). Optional."
                },
                "study": {
                    "type": "string",
                    "description": "Study group this run belongs to (e.g. \"latency-bandwidth\"). Optional."
                },
                "manifest_path": {
                    "type": "string",
                    "description": "Project-relative path to the run's observability manifest (the binary's `.experiments/<run-id>/manifest.json`, when written). Optional."
                },
                "notes": {
                    "type": "string",
                    "description": "Free-form notes (e.g. \"checked target throughput; passed\"). Optional."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        let description = match args.get("description").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => {
                return Ok(ToolResult::err(
                    "record_run: missing or empty `description` arg",
                ));
            }
        };
        let options = crate::tracking::run_recording::RecordRunOptions {
            description,
            workload: args
                .get("workload")
                .and_then(|v| v.as_str())
                .map(String::from),
            candidate: args
                .get("candidate")
                .and_then(|v| v.as_str())
                .map(String::from),
            study: args.get("study").and_then(|v| v.as_str()).map(String::from),
            manifest_path: args
                .get("manifest_path")
                .and_then(|v| v.as_str())
                .map(std::path::PathBuf::from),
            notes: args.get("notes").and_then(|v| v.as_str()).map(String::from),
            parent_run_id: None,
            sweep_parameter: None,
            sweep_value: None,
            tags: Vec::new(),
        };
        let dot = ctx.project_dir.join(".sim-flow");
        match crate::tracking::run_recording::record_run(ctx.project_dir, &dot, &options) {
            Ok(recorded) => Ok(ToolResult::ok(format!(
                "[record_run] logged `{}` (sequence {}, artifact dir `{}`).",
                recorded.run_id,
                recorded.sequence,
                recorded
                    .artifact_dir
                    .strip_prefix(ctx.project_dir)
                    .unwrap_or(&recorded.artifact_dir)
                    .display(),
            ))),
            Err(err) => Ok(ToolResult::err(format!(
                "record_run: could not record run: {err}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(dir: &'a std::path::Path) -> ToolContext<'a> {
        ToolContext::new(dir, None, None, None).with_step_id("DM4b")
    }

    #[test]
    fn record_run_creates_experiments_db_and_artifact_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".sim-flow")).unwrap();
        let result = RecordRunTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"description": "baseline-1k-burst", "workload": "random-1k-burst"}),
            )
            .unwrap();
        assert!(result.ok, "{}", result.display);
        // Experiments DB exists.
        assert!(tmp.path().join(".sim-flow/experiments.db").exists());
        // Per-run artifact dir exists.
        let exps_dir = tmp.path().join(".experiments");
        assert!(exps_dir.exists());
        let entries: Vec<_> = std::fs::read_dir(&exps_dir).unwrap().collect();
        assert_eq!(entries.len(), 1, "expected one run dir");
    }

    #[test]
    fn record_run_rejects_empty_description() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".sim-flow")).unwrap();
        let result = RecordRunTool
            .invoke(&ctx(tmp.path()), &json!({"description": ""}))
            .unwrap();
        assert!(!result.ok);
    }

    #[test]
    fn record_run_attaches_optional_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".sim-flow")).unwrap();
        let result = RecordRunTool
            .invoke(
                &ctx(tmp.path()),
                &json!({
                    "description": "sweep-fifo-depth-32",
                    "workload": "random-1k-burst",
                    "candidate": "fifo-depth-32",
                    "study": "queue-pressure",
                    "notes": "target met; stall count = 0"
                }),
            )
            .unwrap();
        assert!(result.ok, "{}", result.display);
    }
}
