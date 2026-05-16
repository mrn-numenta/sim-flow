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
        "Run a cargo subcommand (`fmt`, `fmt-check`, `build`, `check`, `test`, `clippy`, or `run`) \
         in the project directory and return the truncated stdout / stderr plus the exit code. \
         Use this whenever you need actual compiler / test / lint / runtime output -- do NOT \
         guess at build errors from source files. For `run` you can pass `binary_args` to forward \
         flags to the project binary (e.g. `--run-id baseline-1k-burst`). After a successful `run`, \
         call `record_run` to log the run into `.sim-flow/experiments.db`."
    }
    fn args_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "enum": ["fmt", "fmt-check", "build", "check", "test", "clippy", "run"],
                    "description": "Which cargo subcommand to run. `fmt` formats every Rust file \
                                   in place (idempotent); `fmt-check` reports a non-zero exit if \
                                   any file is mis-formatted without modifying it. `check` is the \
                                   cheapest way to get type errors; `build` produces a binary; \
                                   `test` compiles + runs tests; `clippy` adds lints; `run` \
                                   invokes the project's main binary (use `binary_args` to pass \
                                   --run-id and other flags through `--`)."
                },
                "binary_args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Arguments to forward to the project binary after `--` (only \
                                    used when command = `run`). Example: \
                                    [\"--run-id\", \"baseline-1k-burst\"]."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return Ok(ToolResult::err("run_cargo: missing `command` arg")),
        };
        let binary_args: Vec<String> = args
            .get("binary_args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if command != "run" && !binary_args.is_empty() {
            return Ok(ToolResult::err(format!(
                "run_cargo: `binary_args` only applies to command = `run`; got command = `{command}`"
            )));
        }
        let outcome = match command {
            "fmt" => runners::cargo_fmt(ctx.project_dir, /*check_only=*/ false),
            "fmt-check" => runners::cargo_fmt(ctx.project_dir, /*check_only=*/ true),
            "check" => runners::cargo_check(ctx.project_dir),
            "build" => spawn_one(ctx.project_dir, "build"),
            "test" => runners::cargo_test(ctx.project_dir, None),
            "clippy" => runners::cargo_clippy(ctx.project_dir),
            "run" => spawn_run(ctx.project_dir, &binary_args),
            other => {
                return Ok(ToolResult::err(format!(
                    "run_cargo: unsupported command `{other}`; allowed: fmt, fmt-check, build, check, test, clippy, run"
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

/// Spawn `cargo run --quiet -- <binary_args...>` in the project
/// directory. Captures stdout / stderr tails the same way the other
/// runners do. The binary's `--run-id <id>` (when present) is what
/// downstream `record_run` keys on; this runner does NOT parse it
/// out -- recording is the agent's explicit step (call `record_run`
/// after the run succeeds).
fn spawn_run(project: &std::path::Path, binary_args: &[String]) -> Result<runners::RunnerOutput> {
    use std::process::Command;
    let mut command = Command::new("cargo");
    command.arg("run").arg("--quiet").current_dir(project);
    if !binary_args.is_empty() {
        command.arg("--");
        for arg in binary_args {
            command.arg(arg);
        }
    }
    let output = command.output().map_err(|err| crate::Error::Io {
        path: project.to_path_buf(),
        source: err,
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let exit_code = output.status.code().unwrap_or(-1);
    let suffix = if binary_args.is_empty() {
        String::new()
    } else {
        format!(" -- {}", binary_args.join(" "))
    };
    Ok(runners::RunnerOutput {
        command: format!("cargo run --quiet{suffix}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(dir: &'a std::path::Path) -> ToolContext<'a> {
        ToolContext::new(dir, None, None, None)
    }

    #[test]
    fn missing_command_arg_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let result = RunCargoTool.invoke(&ctx(tmp.path()), &json!({})).unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("missing `command`"));
    }

    #[test]
    fn unsupported_command_lists_the_allowlist() {
        let tmp = tempfile::tempdir().unwrap();
        let result = RunCargoTool
            .invoke(&ctx(tmp.path()), &json!({"command": "publish"}))
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("unsupported command `publish`"));
        // The error must spell out the allowlist so the agent can
        // self-correct on the next turn.
        assert!(result.display.contains("fmt"));
        assert!(result.display.contains("test"));
        assert!(result.display.contains("run"));
    }

    #[test]
    fn binary_args_on_non_run_command_is_rejected() {
        // binary_args is reserved for `run`. Calling `check` with
        // them must fail loud rather than silently dropping them.
        let tmp = tempfile::tempdir().unwrap();
        let result = RunCargoTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"command": "check", "binary_args": ["--run-id", "x"]}),
            )
            .unwrap();
        assert!(!result.ok);
        assert!(result.display.contains("only applies to command = `run`"));
        assert!(result.display.contains("got command = `check`"));
    }

    #[test]
    fn empty_binary_args_array_on_non_run_is_ok_to_reach_spawn() {
        // An empty array shouldn't be flagged -- it's the
        // not-empty case that matters. The spawn will then fail
        // because there's no Cargo.toml in the tempdir, but the
        // validation should let us past the binary_args check.
        let tmp = tempfile::tempdir().unwrap();
        let result = RunCargoTool
            .invoke(
                &ctx(tmp.path()),
                &json!({"command": "check", "binary_args": []}),
            )
            .unwrap();
        // The result will be ok=false because cargo check fails in
        // an empty dir, but the failure message should NOT be the
        // binary_args-misuse one.
        assert!(
            !result.display.contains("only applies to command = `run`"),
            "binary_args check spuriously fired for empty array; got {}",
            result.display
        );
    }
}
