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
        "Run a cargo subcommand (`build`, `check`, `test`, or `clippy`) in the project directory \
         and return the truncated stdout / stderr plus the exit code. Use this whenever you need \
         actual compiler / test output -- do NOT guess at build errors from source files."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "enum": ["build", "check", "test", "clippy"],
                    "description": "Which cargo subcommand to run. `check` is the cheapest \
                                   way to get type errors; `build` produces a binary; `test` \
                                   compiles + runs tests; `clippy` adds lints."
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
            "check" => runners::cargo_check(ctx.project_dir),
            "build" => spawn_one(ctx.project_dir, "build"),
            "test" => runners::cargo_test(ctx.project_dir, None),
            "clippy" => spawn_one(ctx.project_dir, "clippy"),
            other => {
                return Ok(ToolResult::err(format!(
                    "run_cargo: unsupported command `{other}`; allowed: build, check, test, clippy"
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
        let display = format!(
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
        );
        if outcome.ok() {
            Ok(ToolResult::ok(display))
        } else {
            // Tool didn't fail in any framework sense, but the cargo
            // run reported a non-zero exit. We surface that via the
            // err variant so the chat UI annotates it as a failed
            // tool invocation; the agent still gets the full output
            // in `display` and can act on it.
            Ok(ToolResult::err(display))
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
