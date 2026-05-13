//! `run_cargo(command: string)` -- run a cargo subcommand in the
//! project directory and return the trimmed output plus exit code.
//!
//! The agent uses this when it needs to see real compiler / test
//! output, not just guess at errors from source. Restricted to a
//! small allowlist of cargo subcommands so this tool can't shell out
//! arbitrarily; arguments beyond the subcommand name are not
//! accepted today (we may add a `--test <name>` narrow form later).

use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::Result;
use crate::session::runners;

pub struct RunCargoTool;

impl Tool for RunCargoTool {
    fn name(&self) -> &'static str {
        "run_cargo"
    }
    fn description(&self) -> &'static str {
        "Run a cargo subcommand (`fmt`, `fmt-check`, `build`, `check`, `test`, or `clippy`) in \
         the project directory and return the truncated stdout / stderr plus the exit code. Use \
         this whenever you need actual compiler / test / lint output -- do NOT guess at build \
         errors from source files."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "enum": ["fmt", "fmt-check", "build", "check", "test", "clippy"],
                    "description": "Which cargo subcommand to run. `fmt` formats every Rust file \
                                   in place (idempotent; safe to run repeatedly); `fmt-check` \
                                   reports a non-zero exit if any file is mis-formatted without \
                                   modifying it. `check` is the cheapest way to get type errors; \
                                   `build` produces a binary; `test` compiles + runs tests; \
                                   `clippy` adds lints."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return Ok(ToolResult::err("run_cargo: missing `command` arg")),
        };
        let outcome = match command {
            "fmt" => runners::cargo_fmt(ctx.project_dir, /*check_only=*/ false),
            "fmt-check" => runners::cargo_fmt(ctx.project_dir, /*check_only=*/ true),
            "check" => runners::cargo_check(ctx.project_dir),
            "build" => spawn_one(ctx.project_dir, "build"),
            "test" => runners::cargo_test(ctx.project_dir, None),
            "clippy" => runners::cargo_clippy(ctx.project_dir),
            other => {
                return Ok(ToolResult::err(format!(
                    "run_cargo: unsupported command `{other}`; allowed: fmt, fmt-check, build, check, test, clippy"
                )));
            }
        };
        let outcome = match outcome {
            Ok(o) => o,
            Err(err) => {
                return Ok(ToolResult::err(format!(
                    "run_cargo: failed to spawn cargo: {err}"
                )));
            }
        };
        // For `cargo test` failures, replace the raw stdout/stderr
        // tail dump with a coalesced panic-block summary -- the
        // sim-foundation framework emits ~7 lines of per-test
        // instrumentation noise around each failing test, which
        // crowds out the actual panic location + assertion body in
        // the 8KB stderr tail. The summarizer extracts each
        // `thread '...' panicked at FILE:LINE:` block, drops the
        // instrumentation, and groups identical assertions so the
        // agent sees `5 tests failed at line 290 (...)` instead of
        // five copies. When the parser finds nothing (e.g. compile
        // error before any test ran), fall back to the raw dump.
        let test_summary = if command == "test" && !outcome.ok() {
            crate::session::runners::summarize_cargo_test_failures(
                &outcome.stdout_tail,
                &outcome.stderr_tail,
            )
        } else {
            None
        };
        // Clippy output coalescing: group warnings / errors by
        // lint name + location-shape so the agent sees "this lint
        // tripped 12 times across N files (sample: src/foo.rs:42)"
        // instead of 12 verbatim diagnostic blocks. Mirrors the
        // test-failure coalescing pattern. Clippy emits its
        // diagnostics on stderr, not stdout.
        let clippy_summary = if command == "clippy" && !outcome.ok() {
            crate::session::runners::summarize_clippy_diagnostics(
                &outcome.stdout_tail,
                &outcome.stderr_tail,
            )
        } else {
            None
        };

        let display = if let Some(s) = &test_summary {
            format!(
                "[run_cargo `{}`] exit {}\n\n{}",
                outcome.command, outcome.exit_code, s.display,
            )
        } else if let Some(s) = &clippy_summary {
            format!(
                "[run_cargo `{}`] exit {}\n\n{}",
                outcome.command, outcome.exit_code, s.display,
            )
        } else {
            format!(
                "[run_cargo `{}`] exit {}\n\n--- stdout (tail) ---\n{}\n\n--- stderr (tail) ---\n{}",
                outcome.command,
                outcome.exit_code,
                if outcome.stdout_tail.is_empty() {
                    "(empty)"
                } else {
                    outcome.stdout_tail.as_str()
                },
                if outcome.stderr_tail.is_empty() {
                    "(empty)"
                } else {
                    outcome.stderr_tail.as_str()
                },
            )
        };

        if outcome.ok() {
            Ok(ToolResult::ok(display))
        } else {
            // Tool didn't fail in any framework sense, but the cargo
            // run reported a non-zero exit. Surface via err() so the
            // chat UI annotates it as a failed tool invocation; the
            // agent still gets the full (or coalesced) output in
            // `display` and can act on it. If the summarizer parsed
            // a failure count, attach it so the orchestrator's
            // auto-iter loop can detect progress.
            let mut result = ToolResult::err(display);
            if let Some(s) = test_summary {
                result = result
                    .with_test_failure_count(s.failure_count)
                    .with_test_failures(s.failing_tests);
            }
            Ok(result)
        }
    }
}

fn spawn_one(project: &std::path::Path, sub: &str) -> Result<runners::RunnerOutput> {
    use std::process::Command;
    let mut command = Command::new("cargo");
    command.arg(sub).arg("--quiet").current_dir(project);
    let output = command.output().map_err(|err| crate::Error::Io {
        path: project.to_path_buf(),
        source: err,
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let exit_code = output.status.code().unwrap_or(-1);
    Ok(runners::RunnerOutput {
        command: format!("cargo {sub} --quiet"),
        stdout_tail: tail(&stdout, 4_000),
        stderr_tail: tail(&stderr, 8_000),
        exit_code,
    })
}

fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let start = s.len() - max;
    let trimmed = s.get(start..).unwrap_or(s);
    format!("...(truncated, last {max} bytes)\n{trimmed}")
}
