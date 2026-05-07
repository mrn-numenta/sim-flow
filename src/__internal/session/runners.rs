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
}
