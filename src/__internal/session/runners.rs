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
    spawn(project, "cargo", args)
}

/// Run `cargo clippy --all-targets --quiet -- -D warnings`. The
/// `-D warnings` makes any clippy warning a non-zero exit so
/// the orchestrator's gate can fail on lint regressions; the
/// `--quiet` cuts the "checking sim-flow ..." progress lines
/// so the diagnostic body is what dominates the captured tail.
pub fn cargo_clippy(project: &Path) -> Result<RunnerOutput> {
    spawn(
        project,
        "cargo",
        &["clippy", "--all-targets", "--quiet", "--", "-D", "warnings"],
    )
}

/// Coalesced view of a failing `cargo test` run. `failure_count`
/// is the number of distinct `panicked at` headers parsed out of
/// stdout (one per failing test); `display` is the formatted summary
/// the orchestrator threads back into the next User turn.
#[derive(Debug, Clone)]
pub struct CargoTestSummary {
    pub failure_count: usize,
    pub display: String,
}

/// Parse `cargo test` stdout into a coalesced failure summary. Each
/// failing test emits a `thread 'NAME' panicked at FILE:LINE:COL:`
/// header followed by 1-3 lines of assertion body, surrounded by
/// per-test instrumentation noise (`--- timing summary ---`,
/// `--- host counters ---` from the sim-foundation framework). The
/// summary keeps the source-location + panic-message pair per
/// failing test, then groups identical panic messages so the agent
/// sees `5 tests failed at tests/smoke.rs:290 (assertion ...)`
/// instead of five copies. Returns `None` when no panic markers are
/// found (the caller falls back to the raw tail-byte output).
pub fn summarize_cargo_test_failures(stdout: &str, stderr: &str) -> Option<CargoTestSummary> {
    let panics = extract_panics(stdout);
    if panics.is_empty() {
        return None;
    }
    let failure_count = panics.len();

    // Group by (file:line, raw message body) so identical assertions
    // collapse. We carry the column off the location (it's
    // typically `col:5` for assertion macros and adds no signal).
    let mut groups: Vec<PanicGroup> = Vec::new();
    for p in &panics {
        let key = (p.location.clone(), p.message.clone());
        if let Some(g) = groups.iter_mut().find(|g| g.key == key) {
            g.tests.push(p.test.clone());
        } else {
            groups.push(PanicGroup {
                key,
                tests: vec![p.test.clone()],
            });
        }
    }

    let unique = groups.len();
    let mut out = String::new();
    out.push_str(&format!(
        "test failures: {failure_count} test(s) panicked across {unique} unique location(s).\n\n",
    ));
    for g in &groups {
        let (loc, msg) = &g.key;
        out.push_str(&format!("- {loc}\n"));
        if !msg.is_empty() {
            for line in msg.lines() {
                out.push_str(&format!("  {line}\n"));
            }
        }
        out.push_str(&format!("  failing test(s): {}\n\n", g.tests.join(", ")));
    }

    // The last `test result:` line of stdout carries the
    // pass/fail counts -- preserve it verbatim. Fall through to
    // empty if missing.
    if let Some(line) = stdout.lines().rev().find(|l| l.starts_with("test result:")) {
        out.push_str(line.trim());
        out.push('\n');
    }
    // Compile / link errors land in stderr (e.g. unused imports
    // promoted to errors via `-D warnings`). Keep a tail so the
    // agent sees them.
    let trimmed_stderr = stderr.trim();
    if !trimmed_stderr.is_empty() {
        out.push_str("\nstderr (tail):\n");
        out.push_str(trimmed_stderr);
        out.push('\n');
    }
    Some(CargoTestSummary {
        failure_count,
        display: out,
    })
}

#[derive(Debug, Clone)]
struct ExtractedPanic {
    test: String,
    /// `file:line:col` form from the `panicked at` header. Column
    /// retained because some assertion macros report multi-arg
    /// positions; trimming back to file:line happens at display
    /// time when columns aren't useful.
    location: String,
    message: String,
}

#[derive(Debug)]
struct PanicGroup {
    key: (String, String),
    tests: Vec<String>,
}

/// Walk the cargo-test stdout extracting each
/// `thread 'NAME' panicked at FILE:LINE[:COL]:` header plus its
/// message body (the lines immediately following until a blank
/// line, the next `thread '...' panicked` header, the `failures:`
/// list, or the `test result:` summary). Lines containing only
/// instrumentation chrome (`--- timing summary ---`, `--- host
/// counters ---`, the indented metric lines under them) are
/// dropped from the captured message.
fn extract_panics(stdout: &str) -> Vec<ExtractedPanic> {
    let mut out: Vec<ExtractedPanic> = Vec::new();
    let lines: Vec<&str> = stdout.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some((test, location)) = parse_panic_header(line) {
            // Collect the message body until a terminator. Skip the
            // sim-foundation instrumentation blocks the framework
            // emits per test.
            let mut msg_lines: Vec<&str> = Vec::new();
            let mut j = i + 1;
            while j < lines.len() {
                let l = lines[j];
                if l.is_empty() {
                    break;
                }
                if l.starts_with("thread '") && l.contains("panicked at") {
                    break;
                }
                if l.starts_with("failures:") || l.starts_with("test result:") {
                    break;
                }
                if l.starts_with("---- ") && l.ends_with(" stdout ----") {
                    break;
                }
                if !is_instrumentation_line(l) {
                    msg_lines.push(l);
                }
                j += 1;
            }
            out.push(ExtractedPanic {
                test,
                location,
                message: msg_lines.join("\n"),
            });
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Match `thread 'NAME' (PID) panicked at FILE:LINE:COL:` (the PID
/// parenthetical is optional — older Rust toolchains omit it).
/// Returns `(test_name, location)` on a hit, where location is the
/// `file:line[:col]` substring without the trailing colon. Anything
/// that doesn't fit the shape is rejected — we don't want to
/// match log lines that quote the word `panicked` in passing.
fn parse_panic_header(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("thread '")?;
    let (test, after_test) = rest.split_once("' ")?;
    // After the test name we accept either `(<pid>) panicked at ` or
    // a bare `panicked at `. Skip the parenthetical when present.
    let after_test = after_test.trim_start();
    let after_pid = if let Some(p) = after_test.strip_prefix('(') {
        let close = p.find(')')?;
        p[close + 1..].trim_start()
    } else {
        after_test
    };
    let after_panicked = after_pid.strip_prefix("panicked at ")?;
    // Strip a trailing `:` from the location -- the line ends with
    // `:` separating the location from the message. The message
    // itself starts on the next line.
    let location = after_panicked.trim_end_matches(':').trim().to_string();
    Some((test.to_string(), location))
}

fn is_instrumentation_line(l: &str) -> bool {
    let t = l.trim_start();
    t.starts_with("--- timing summary ---")
        || t.starts_with("--- host counters ---")
        || t.starts_with("elaboration:")
        || t.starts_with("simulation:")
        || t.starts_with("total:")
        || t.starts_with("cycles:")
        || t.starts_with("throughput:")
        || t.starts_with("elab insn:")
        || t.starts_with("sim insn:")
        || t.starts_with("insn/sim-cycle:")
}

/// Coalesced view of a failing `cargo clippy` run. `diagnostic_count`
/// is the total number of `error:` / `warning:` blocks parsed out
/// of stderr; `display` is the formatted summary the orchestrator
/// threads back into the next User turn.
#[derive(Debug, Clone)]
pub struct ClippyDiagSummary {
    pub diagnostic_count: usize,
    pub display: String,
}

/// Parse `cargo clippy` stderr into a coalesced summary. Each
/// diagnostic emits an `error:` or `warning:` header followed by a
/// `   --> file:line:col` location line and several lines of code-
/// snippet + help. Many diagnostics are the SAME lint repeated at
/// different sites; coalescing groups by header text and lists the
/// locations together so the agent sees `clippy::single_match: 12
/// occurrences across 4 files (sample: src/foo.rs:42)` rather than
/// 12 verbatim blocks.
///
/// Returns `None` when no diagnostic headers are found (caller
/// falls back to the raw stderr tail).
pub fn summarize_clippy_diagnostics(stdout: &str, stderr: &str) -> Option<ClippyDiagSummary> {
    // Clippy emits diagnostics on stderr. stdout typically just
    // shows the `Compiling ...` / `Checking ...` progress lines
    // (cut by `--quiet`) plus a final cargo summary; we keep it
    // trimmed for context but parse the stderr.
    let diagnostics = extract_clippy_diagnostics(stderr);
    if diagnostics.is_empty() {
        return None;
    }
    let diagnostic_count = diagnostics.len();

    // Group by (kind, message header). Locations differ per
    // occurrence; collect them per group.
    let mut groups: Vec<ClippyGroup> = Vec::new();
    for d in &diagnostics {
        let key = (d.kind, d.message.clone());
        if let Some(g) = groups.iter_mut().find(|g| g.key == key) {
            g.locations.push(d.location.clone());
        } else {
            groups.push(ClippyGroup {
                key,
                locations: vec![d.location.clone()],
            });
        }
    }

    let unique = groups.len();
    let mut out = String::new();
    out.push_str(&format!(
        "clippy diagnostics: {diagnostic_count} total, {unique} unique.\n\n",
    ));
    for g in &groups {
        let (kind, msg) = &g.key;
        let count = g.locations.len();
        let kind_str = match kind {
            ClippyKind::Error => "error",
            ClippyKind::Warning => "warning",
        };
        if count == 1 {
            out.push_str(&format!("- {kind_str}: {msg}\n  at {}\n", g.locations[0]));
        } else {
            // Show a sample location + count of additional sites.
            let sample = &g.locations[0];
            out.push_str(&format!(
                "- {kind_str}: {msg}\n  ({count} occurrences; sample: {sample})\n",
            ));
            // For small group sizes (<=6) list every location so
            // the agent can fix them in one pass.
            if count <= 6 {
                for loc in &g.locations[1..] {
                    out.push_str(&format!("    also: {loc}\n"));
                }
            }
        }
        out.push('\n');
    }
    // Preserve the final cargo summary line(s) verbatim if present
    // (e.g. "error: could not compile ..." or
    // "error: aborting due to N previous errors").
    for line in stderr.lines().rev().take(20) {
        if line.starts_with("error: aborting due to") || line.starts_with("error: could not") {
            out.push_str(line.trim());
            out.push('\n');
        }
    }
    let trimmed_stdout = stdout.trim();
    if !trimmed_stdout.is_empty() {
        out.push_str("\nstdout (tail):\n");
        out.push_str(trimmed_stdout);
        out.push('\n');
    }
    Some(ClippyDiagSummary {
        diagnostic_count,
        display: out,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClippyKind {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
struct ExtractedClippyDiag {
    kind: ClippyKind,
    /// First-line text of the diagnostic, with the `error: ` /
    /// `warning: ` prefix stripped and a trailing lint-name
    /// suffix (`#[deny(clippy::single_match)]`) preserved when
    /// present so identical lints group correctly.
    message: String,
    /// `file:line:col` form from the `--> ...` location line.
    location: String,
}

#[derive(Debug)]
struct ClippyGroup {
    key: (ClippyKind, String),
    locations: Vec<String>,
}

/// Walk clippy stderr extracting each diagnostic block. A block
/// opens with `error: ...` or `warning: ...` and contains a
/// `   --> file:line:col` location line. Blocks without a location
/// (terminal "could not compile" wrappers) are dropped here -- the
/// caller surfaces those separately as the trailing summary line.
fn extract_clippy_diagnostics(stderr: &str) -> Vec<ExtractedClippyDiag> {
    let mut out: Vec<ExtractedClippyDiag> = Vec::new();
    let lines: Vec<&str> = stderr.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let kind = if line.starts_with("error: ") {
            Some(ClippyKind::Error)
        } else if line.starts_with("warning: ") {
            Some(ClippyKind::Warning)
        } else {
            None
        };
        let Some(kind) = kind else {
            i += 1;
            continue;
        };
        // The "summary" diagnostic at the tail of clippy output --
        // `error: aborting due to N previous errors; M warnings
        // emitted` or `error: could not compile FOO due to N
        // previous errors` -- doesn't have a location line and is
        // accounted for separately. Detect by header shape so it
        // doesn't inflate the diagnostic count.
        let msg_first_line = line
            .trim_start_matches("error: ")
            .trim_start_matches("warning: ");
        if msg_first_line.starts_with("aborting due to")
            || msg_first_line.starts_with("could not compile")
        {
            i += 1;
            continue;
        }
        // Look for the `--> file:line:col` location line in the
        // next few non-blank lines. Real diagnostics have it
        // within 1-2 lines of the header; if missing entirely,
        // skip the block (treat as non-coalescable).
        let mut location: Option<String> = None;
        let mut j = i + 1;
        let scan_limit = (i + 5).min(lines.len());
        while j < scan_limit {
            let l = lines[j].trim_start();
            if let Some(rest) = l.strip_prefix("--> ") {
                location = Some(rest.trim().to_string());
                break;
            }
            j += 1;
        }
        let Some(location) = location else {
            i += 1;
            continue;
        };
        out.push(ExtractedClippyDiag {
            kind,
            message: msg_first_line.to_string(),
            location,
        });
        // Advance to the next blank line OR next header so we
        // don't double-count when the block has its own embedded
        // `note:` / `help:` lines.
        i = j + 1;
        while i < lines.len() {
            let l = lines[i];
            if l.is_empty() || l.starts_with("error: ") || l.starts_with("warning: ") {
                break;
            }
            i += 1;
        }
    }
    out
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

/// Default per-invocation timeout for cargo / shell runners.
/// Picks 5 minutes because that's safely above the longest
/// observed legitimate `cargo test` (~3 min on the rgb_toy + a
/// margin for lib loads) but tight enough to catch a hung test or
/// a wedged compiler. Override via `SIM_FLOW_CARGO_TIMEOUT_SECS`.
const DEFAULT_RUNNER_TIMEOUT_SECS: u64 = 300;

fn runner_timeout() -> std::time::Duration {
    let secs = std::env::var("SIM_FLOW_CARGO_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_RUNNER_TIMEOUT_SECS);
    std::time::Duration::from_secs(secs)
}

fn spawn(project: &Path, cmd: &str, args: &[&str]) -> Result<RunnerOutput> {
    use std::io::Read;
    let mut command = Command::new(cmd);
    command
        .args(args)
        .current_dir(project)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|err| Error::Io {
        path: project.to_path_buf(),
        source: err,
    })?;

    // Poll for completion with a deadline. We can't use
    // `Command::output()` directly because it blocks until the
    // child exits -- a wedged cargo test would hang the whole
    // orchestrator. Polling at 100ms keeps idle CPU low while
    // still surfacing a hang within a fraction of a second of the
    // timeout.
    let timeout = runner_timeout();
    let started = std::time::Instant::now();
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(RunnerOutput {
                        command: format_command(cmd, args),
                        stdout_tail: String::new(),
                        stderr_tail: format!(
                            "runner timed out after {} seconds (override via SIM_FLOW_CARGO_TIMEOUT_SECS)",
                            timeout.as_secs()
                        ),
                        exit_code: -1,
                    });
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(err) => {
                return Err(Error::Io {
                    path: project.to_path_buf(),
                    source: err,
                });
            }
        }
    };

    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_end(&mut stdout_buf);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_end(&mut stderr_buf);
    }
    let stdout = String::from_utf8_lossy(&stdout_buf).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_buf).into_owned();
    let exit_code = exit_status.code().unwrap_or(-1);
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

    #[test]
    fn parse_panic_header_handles_pid_and_no_pid_forms() {
        let with_pid = "thread 'foo' (38148180) panicked at tests/smoke.rs:230:5:";
        let (test, loc) = parse_panic_header(with_pid).expect("with-pid form");
        assert_eq!(test, "foo");
        assert_eq!(loc, "tests/smoke.rs:230:5");

        let no_pid = "thread 'bar' panicked at src/lib.rs:42:";
        let (test, loc) = parse_panic_header(no_pid).expect("no-pid form");
        assert_eq!(test, "bar");
        assert_eq!(loc, "src/lib.rs:42");

        assert!(parse_panic_header("just some prose mentioning panicked").is_none());
        assert!(parse_panic_header("thread 'foo' did stuff").is_none());
    }

    #[test]
    fn extract_panics_drops_instrumentation_noise() {
        // Real shape from the rgb_toy run: per-test "timing summary"
        // + "host counters" blocks surround the actual panic line.
        // The extractor must keep the assertion body (which the
        // agent needs to fix the test) and drop the framework's
        // instrumentation chrome.
        let stdout = "\
running 3 tests
test foo ... ok
test bar --- FAILED
test baz --- FAILED

failures:

---- bar stdout ----
--- timing summary ---
  elaboration:       1.23 ms (98.5%)
  simulation:        0.02 ms (1.5%)
  cycles:               1
--- host counters ---
  elab insn:        5.56M

thread 'bar' (38148180) panicked at tests/smoke.rs:230:5:
assertion `left == right` failed: expected one output after reset
  left: 0
 right: 1

---- baz stdout ----
--- timing summary ---
  elaboration:       1.14 ms

thread 'baz' panicked at tests/smoke.rs:176:5:
assertion `left == right` failed: expected 10 output pixels, got 0
  left: 0
 right: 10


failures:
    bar
    baz

test result: FAILED. 1 passed; 2 failed; 0 ignored
";
        let panics = extract_panics(stdout);
        assert_eq!(panics.len(), 2, "got {panics:?}");
        assert_eq!(panics[0].test, "bar");
        assert_eq!(panics[0].location, "tests/smoke.rs:230:5");
        assert!(
            panics[0]
                .message
                .contains("assertion `left == right` failed")
        );
        assert!(!panics[0].message.contains("timing summary"));
        assert!(!panics[0].message.contains("host counters"));
        assert!(!panics[0].message.contains("elaboration:"));
        assert_eq!(panics[1].test, "baz");
        assert_eq!(panics[1].location, "tests/smoke.rs:176:5");
        assert!(panics[1].message.contains("expected 10 output pixels"));
    }

    #[test]
    fn summarize_coalesces_identical_assertions() {
        // Five tests fail at the same source line with the same
        // assertion. The summary should show one block listing all
        // five test names rather than five separate entries.
        let mut stdout = String::new();
        for name in ["a", "b", "c", "d", "e"] {
            stdout.push_str(&format!(
                "thread '{name}' panicked at tests/smoke.rs:290:5:\n\
                 assertion `left == right` failed\n  left: 0\n right: 1\n\n"
            ));
        }
        stdout.push_str("test result: FAILED. 0 passed; 5 failed; 0 ignored\n");

        let summary = summarize_cargo_test_failures(&stdout, "")
            .expect("summary should be produced for failures");
        assert_eq!(summary.failure_count, 5);
        assert!(
            summary
                .display
                .contains("5 test(s) panicked across 1 unique")
        );
        assert!(summary.display.contains("tests/smoke.rs:290:5"));
        // All five test names listed under the single group.
        for name in ["a", "b", "c", "d", "e"] {
            assert!(
                summary.display.contains(name),
                "missing test name {name} in summary",
            );
        }
        // The pass/fail summary line is preserved.
        assert!(summary.display.contains("test result: FAILED"));
    }

    #[test]
    fn summarize_returns_none_when_no_failures() {
        let stdout = "running 3 tests\n\ntest result: ok. 3 passed; 0 failed; 0 ignored\n";
        assert!(summarize_cargo_test_failures(stdout, "").is_none());
    }

    #[test]
    fn summarize_keeps_distinct_locations_separate() {
        let stdout = "\
thread 'a' panicked at tests/smoke.rs:42:5:
assertion `left == right` failed
  left: 0
 right: 1

thread 'b' panicked at tests/smoke.rs:88:5:
assertion failed: x.is_some()

test result: FAILED. 0 passed; 2 failed
";
        let s = summarize_cargo_test_failures(stdout, "")
            .expect("two failures should produce a summary");
        assert_eq!(s.failure_count, 2);
        // Two unique locations.
        assert!(s.display.contains("2 test(s) panicked across 2 unique"));
        assert!(s.display.contains("tests/smoke.rs:42:5"));
        assert!(s.display.contains("tests/smoke.rs:88:5"));
    }

    #[test]
    fn clippy_summary_returns_none_on_empty_input() {
        assert!(summarize_clippy_diagnostics("", "").is_none());
        assert!(summarize_clippy_diagnostics("anything", "").is_none());
    }

    #[test]
    fn clippy_summary_extracts_single_warning() {
        let stderr = "\
warning: this `let` binding has unit value
   --> tests/foo.rs:42:5
    |
42  |     let _ = bar();
    |     ^^^^^^^^^^^^^^
    |
    = note: `#[warn(let_unit_value)]` on by default

error: aborting due to 0 previous errors; 1 warning emitted
";
        let s = summarize_clippy_diagnostics("", stderr)
            .expect("should produce a summary for one warning");
        assert_eq!(s.diagnostic_count, 1);
        assert!(s.display.contains("1 total, 1 unique"));
        assert!(s.display.contains("warning:"));
        assert!(s.display.contains("tests/foo.rs:42:5"));
        assert!(s.display.contains("`let` binding has unit value"));
    }

    #[test]
    fn clippy_summary_groups_repeats_across_files() {
        // The same `single_match` lint at three different sites
        // should produce ONE group with three locations, not
        // three separate entries.
        let stderr = "\
warning: you seem to be trying to use `match` for an equality check
   --> src/a.rs:10:5
    |
10  |     match x { 1 => 1, _ => 0, };
    |
warning: you seem to be trying to use `match` for an equality check
   --> src/b.rs:22:9
    |
22  |     match y { 0 => 0, _ => 1, };
    |
warning: you seem to be trying to use `match` for an equality check
   --> src/c.rs:33:13
    |
33  |     match z { 2 => 2, _ => 3, };
    |
error: aborting due to 0 previous errors; 3 warnings emitted
";
        let s = summarize_clippy_diagnostics("", stderr)
            .expect("three identical warnings should still summarize");
        assert_eq!(s.diagnostic_count, 3);
        assert!(s.display.contains("3 total, 1 unique"));
        assert!(s.display.contains("3 occurrences"));
        // All three locations should be listed (count <= 6).
        assert!(s.display.contains("src/a.rs:10:5"));
        assert!(s.display.contains("src/b.rs:22:9"));
        assert!(s.display.contains("src/c.rs:33:13"));
    }

    #[test]
    fn clippy_summary_distinguishes_errors_from_warnings() {
        let stderr = "\
error: unused import: `std::collections::HashMap`
   --> src/foo.rs:1:5
    |
1   | use std::collections::HashMap;
    |     ^^^^^^^^^^^^^^^^^^^^^^^^^

warning: this expression creates a reference which is immediately dereferenced by the compiler
   --> src/foo.rs:5:9
    |
5   |     bar(&x);
    |         ^^

error: aborting due to 1 previous error; 1 warning emitted
";
        let s = summarize_clippy_diagnostics("", stderr).expect("mixed errors + warnings");
        assert_eq!(s.diagnostic_count, 2);
        assert!(s.display.contains("2 total, 2 unique"));
        assert!(s.display.contains("- error: unused import"));
        assert!(s.display.contains("- warning: "));
    }

    #[test]
    fn clippy_summary_drops_summary_line_from_count() {
        // The terminal `error: aborting due to N previous errors`
        // line has no `-->` location and must NOT be counted as
        // a third diagnostic.
        let stderr = "\
error: real failure
   --> src/foo.rs:1:1
    |
1   | bad();
    |

error: also real
   --> src/foo.rs:5:1
    |
5   | bad();
    |

error: aborting due to 2 previous errors

error: could not compile `proj` (lib) due to 2 previous errors
";
        let s = summarize_clippy_diagnostics("", stderr).expect("two real errors");
        assert_eq!(
            s.diagnostic_count, 2,
            "summary should not count the aborting / could-not-compile lines"
        );
        // The could-not-compile line gets preserved verbatim at
        // the end of the display so the agent sees the cargo-
        // level outcome.
        assert!(s.display.contains("could not compile"));
    }

    #[test]
    fn post_work_report_renders_clean_pass_with_just_status_lines() {
        let report = PostWorkCargoReport {
            fmt_ok: true,
            fmt_command: "cargo fmt --all -- --check".into(),
            fmt_tail: String::new(),
            clippy_ok: true,
            clippy_command: "cargo clippy --all-targets --quiet -- -D warnings".into(),
            clippy_summary: None,
            clippy_raw_tail: String::new(),
        };
        let md = report.render_markdown();
        assert!(md.contains("PASS"));
        assert!(!md.contains("FAIL"));
        assert!(!md.contains("### fmt diff"));
        assert!(!md.contains("### clippy diagnostics"));
    }

    #[test]
    fn post_work_report_renders_clippy_failure_with_summary_when_available() {
        let summary = ClippyDiagSummary {
            diagnostic_count: 1,
            display: "clippy diagnostics: 1 total, 1 unique.\n\n- warning: foo\n  at src/x.rs:1\n"
                .into(),
        };
        let report = PostWorkCargoReport {
            fmt_ok: true,
            fmt_command: "cargo fmt --all -- --check".into(),
            fmt_tail: String::new(),
            clippy_ok: false,
            clippy_command: "cargo clippy --all-targets --quiet -- -D warnings".into(),
            clippy_summary: Some(summary),
            clippy_raw_tail: "noise".into(),
        };
        let md = report.render_markdown();
        assert!(md.contains("FAIL"));
        assert!(md.contains("### clippy diagnostics"));
        assert!(md.contains("warning: foo"));
        assert!(!md.contains("noise"));
    }

    #[test]
    fn post_work_report_falls_back_to_raw_tail_when_no_clippy_summary() {
        let report = PostWorkCargoReport {
            fmt_ok: false,
            fmt_command: "cargo fmt --all -- --check".into(),
            fmt_tail: "Diff in src/main.rs at line 1".into(),
            clippy_ok: false,
            clippy_command: "cargo clippy --all-targets --quiet -- -D warnings".into(),
            clippy_summary: None,
            clippy_raw_tail: "linker error: cannot find -lz".into(),
        };
        let md = report.render_markdown();
        assert!(md.contains("### fmt diff"));
        assert!(md.contains("Diff in src/main.rs"));
        assert!(md.contains("### clippy diagnostics"));
        assert!(md.contains("linker error"));
    }

    #[test]
    fn post_work_all_clean_only_true_when_both_pass() {
        let mut r = PostWorkCargoReport {
            fmt_ok: true,
            fmt_command: String::new(),
            fmt_tail: String::new(),
            clippy_ok: true,
            clippy_command: String::new(),
            clippy_summary: None,
            clippy_raw_tail: String::new(),
        };
        assert!(r.all_clean());
        r.fmt_ok = false;
        assert!(!r.all_clean());
        r.fmt_ok = true;
        r.clippy_ok = false;
        assert!(!r.all_clean());
    }
}
