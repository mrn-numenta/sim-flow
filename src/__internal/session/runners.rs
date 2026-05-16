//! Orchestrator-only validators. The agent never invokes these
//! directly; the orchestrator runs them between phases of the
//! iteration loop and feeds their output back to the LLM as a
//! system message when something fails.
//!
//! Submodules group the implementation:
//!   - [`spawn`] -- child-process spawn + reader threads + timeout
//!   - [`cargo_test`] -- panic summary extraction
//!   - [`cargo_clippy`] -- diagnostic summary extraction

use std::path::Path;

use serde::Serialize;

use crate::Result;

mod cargo_clippy;
mod cargo_test;
mod spawn;

#[cfg(test)]
mod tests;

pub use cargo_clippy::{ClippyDiagSummary, summarize_clippy_diagnostics};
pub use cargo_test::{CargoTestSummary, summarize_cargo_test_failures};

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
    spawn::spawn(project, "cargo", &["check", "--quiet"])
}

/// Run `cargo test --quiet` (optionally narrowed to a single test).
/// Used at the start of the `test` phase.
pub fn cargo_test(project: &Path, narrow: Option<&str>) -> Result<RunnerOutput> {
    let mut args: Vec<&str> = vec!["test", "--quiet"];
    if let Some(name) = narrow {
        args.push("--test");
        args.push(name);
    }
    spawn::spawn(project, "cargo", &args)
}

/// Run `cargo fmt --all` (or `--all -- --check` when
/// `check_only`). The `fmt` mode rewrites every Rust file in
/// place; `fmt-check` reports a non-zero exit if any file is
/// mis-formatted without modifying it. Both are idempotent
/// from the agent's perspective: `fmt` produces a no-op when
/// already formatted; `fmt-check` is non-destructive.
pub fn cargo_fmt(project: &Path, check_only: bool) -> Result<RunnerOutput> {
    let args: &[&str] = if check_only {
        &["fmt", "--all", "--", "--check"]
    } else {
        &["fmt", "--all"]
    };
    spawn::spawn(project, "cargo", args)
}

/// Run `cargo clippy --all-targets --quiet -- -D warnings`. The
/// `-D warnings` makes any clippy warning a non-zero exit so
/// the orchestrator's gate can fail on lint regressions; the
/// `--quiet` cuts the "checking sim-flow ..." progress lines
/// so the diagnostic body is what dominates the captured tail.
pub fn cargo_clippy(project: &Path) -> Result<RunnerOutput> {
    spawn::spawn(
        project,
        "cargo",
        &["clippy", "--all-targets", "--quiet", "--", "-D", "warnings"],
    )
}

/// Combined report from running the post-Work cargo checks
/// (`cargo fmt --check` + `cargo clippy --all-targets -- -D
/// warnings`). The orchestrator inlines `render_markdown()` into the
/// next Critique session so the agent doesn't have to spend tool
/// turns re-running these and reasoning about the output -- saves
/// 1-3 LLM dispatches per milestone. Clippy implicitly covers
/// `cargo build` (it can't lint code that doesn't compile, so a
/// build failure surfaces as a clippy `error: ...` block).
#[derive(Debug, Clone)]
pub struct PostWorkCargoReport {
    pub fmt_ok: bool,
    pub fmt_command: String,
    pub fmt_tail: String,
    pub clippy_ok: bool,
    pub clippy_command: String,
    pub clippy_summary: Option<ClippyDiagSummary>,
    pub clippy_raw_tail: String,
}

impl PostWorkCargoReport {
    pub fn all_clean(&self) -> bool {
        self.fmt_ok && self.clippy_ok
    }

    /// Compose the markdown the orchestrator inlines into the
    /// Critique session input. Header is the same regardless of
    /// outcome so the Critique can reliably grep for it; the
    /// PASS / FAIL line per check is the actionable signal.
    pub fn render_markdown(&self) -> String {
        let mut out = String::from(
            "## Orchestrator-side cargo checks (post-Work)\n\n\
             These ran AFTER the work session's last response and \
             BEFORE this critique session was launched. Treat them \
             as authoritative -- the agent's earlier reports of \
             cargo state in the work transcript may be stale.\n\n",
        );
        let fmt_status = if self.fmt_ok { "PASS" } else { "FAIL" };
        let clippy_status = if self.clippy_ok { "PASS" } else { "FAIL" };
        out.push_str(&format!("- `{}` -- {}\n", self.fmt_command, fmt_status));
        out.push_str(&format!(
            "- `{}` -- {}\n\n",
            self.clippy_command, clippy_status
        ));
        if !self.fmt_ok {
            out.push_str("### fmt diff (tail)\n\n```\n");
            out.push_str(self.fmt_tail.trim_end());
            out.push_str("\n```\n\n");
        }
        if !self.clippy_ok {
            out.push_str("### clippy diagnostics\n\n");
            if let Some(summary) = &self.clippy_summary {
                out.push_str(&summary.display);
            } else {
                out.push_str("```\n");
                out.push_str(self.clippy_raw_tail.trim_end());
                out.push_str("\n```\n");
            }
        }
        out
    }
}

/// Run `cargo fmt --all -- --check` followed by `cargo clippy
/// --all-targets --quiet -- -D warnings` in `project` and bundle the
/// results. Returns `Ok(None)` when `project` does not contain a
/// `Cargo.toml` (e.g. early DM steps where no Rust code has landed
/// yet); the caller treats that as "no checks to surface".
pub fn run_post_work_cargo(project: &Path) -> Result<Option<PostWorkCargoReport>> {
    if !project.join("Cargo.toml").exists() {
        return Ok(None);
    }
    let fmt = cargo_fmt(project, true)?;
    let clippy = cargo_clippy(project)?;
    let clippy_summary = if clippy.ok() {
        None
    } else {
        summarize_clippy_diagnostics(&clippy.stdout_tail, &clippy.stderr_tail)
    };
    Ok(Some(PostWorkCargoReport {
        fmt_ok: fmt.ok(),
        fmt_command: fmt.command.clone(),
        fmt_tail: if fmt.ok() {
            String::new()
        } else {
            // fmt --check writes its diff to stdout
            fmt.stdout_tail.clone()
        },
        clippy_ok: clippy.ok(),
        clippy_command: clippy.command.clone(),
        clippy_summary,
        clippy_raw_tail: clippy.stderr_tail.clone(),
    }))
}
