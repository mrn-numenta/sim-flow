//! Orchestrator-only validators. The agent never invokes these
//! directly; the orchestrator runs them between phases of the
//! iteration loop and feeds their output back to the LLM as a
//! system message when something fails.

use std::path::Path;
use std::process::Command;

use serde::Serialize;

use crate::{Error, Result};

/// What the orchestrator wants from a runner: stdout / stderr tail
/// (so the host can render them via `BuildOutput` without us
/// blasting unbounded data over the protocol), and the exit code.
#[derive(Debug, Clone, Serialize)]
pub struct RunnerOutput {
    pub command: String,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub exit_code: i32,
}

impl RunnerOutput {
    pub fn ok(&self) -> bool {
        self.exit_code == 0
    }
}

/// Run `cargo check --quiet` in the project directory. Used at the
/// start of the `build` phase for code-authoring steps.
pub fn cargo_check(project: &Path) -> Result<RunnerOutput> {
    spawn(project, "cargo", &["check", "--quiet"])
}

/// Run `cargo test --quiet` (optionally narrowed to a single test).
/// Used at the start of the `test` phase.
pub fn cargo_test(project: &Path, narrow: Option<&str>) -> Result<RunnerOutput> {
    let mut args: Vec<&str> = vec!["test", "--quiet"];
    if let Some(name) = narrow {
        args.push("--test");
        args.push(name);
    }
    spawn(project, "cargo", &args)
}

fn spawn(project: &Path, cmd: &str, args: &[&str]) -> Result<RunnerOutput> {
    let mut command = Command::new(cmd);
    command.args(args).current_dir(project);
    let output = command.output().map_err(|err| Error::Io {
        path: project.to_path_buf(),
        source: err,
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let exit_code = output.status.code().unwrap_or(-1);
    Ok(RunnerOutput {
        command: format_command(cmd, args),
        stdout_tail: tail(&stdout, 4_000),
        stderr_tail: tail(&stderr, 8_000),
        exit_code,
    })
}

fn format_command(cmd: &str, args: &[&str]) -> String {
    let mut s = cmd.to_string();
    for a in args {
        s.push(' ');
        s.push_str(a);
    }
    s
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

    #[test]
    fn tail_passes_short_strings_through() {
        assert_eq!(tail("hello", 100), "hello");
    }

    #[test]
    fn tail_truncates_long_strings_to_the_end() {
        let s = "a".repeat(2_000);
        let out = tail(&s, 500);
        assert!(out.starts_with("...(truncated"));
        // Last 500 bytes of `s` are appended after the prefix.
        let suffix = &out[out.len() - 500..];
        assert_eq!(suffix, "a".repeat(500));
    }
}
