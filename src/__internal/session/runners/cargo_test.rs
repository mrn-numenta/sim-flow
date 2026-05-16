//! `cargo test` failure-summary extraction.
//!
//! Walks the cargo-test stdout, pulls out each `thread 'NAME'
//! panicked at FILE:LINE[:COL]:` header + its message body, then
//! coalesces identical (location, message) pairs so the agent sees
//! one block per unique panic across N tests rather than N copies.

/// Coalesced view of a failing `cargo test` run. `failure_count`
/// is the number of distinct `panicked at` headers parsed out of
/// stdout (one per failing test); `display` is the formatted summary
/// the orchestrator threads back into the next User turn.
/// `failing_tests` is the (uniqued, ordered-by-first-seen) list of
/// failing test names so the orchestrator's no-progress detector
/// can track which tests are still failing across iterations --
/// distinguishing "agent fixed test A but introduced regression B"
/// from "agent made no progress at all" (both would have an
/// identical raw count when the swap is 1-for-1).
#[derive(Debug, Clone)]
pub struct CargoTestSummary {
    pub failure_count: usize,
    pub failing_tests: Vec<String>,
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
    // Dedup test names in first-seen order so the orchestrator gets
    // a stable, comparable set. The grouping loop above already
    // deduplicates by (location, message) but a single test name can
    // appear under more than one grouping if the body is non-empty
    // and varies; collapse to unique names here.
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut failing_tests: Vec<String> = Vec::new();
    for p in &panics {
        if seen.insert(p.test.as_str()) {
            failing_tests.push(p.test.clone());
        }
    }
    Some(CargoTestSummary {
        failure_count,
        failing_tests,
        display: out,
    })
}

#[derive(Debug, Clone)]
pub(super) struct ExtractedPanic {
    pub(super) test: String,
    /// `file:line:col` form from the `panicked at` header. Column
    /// retained because some assertion macros report multi-arg
    /// positions; trimming back to file:line happens at display
    /// time when columns aren't useful.
    pub(super) location: String,
    pub(super) message: String,
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
pub(super) fn extract_panics(stdout: &str) -> Vec<ExtractedPanic> {
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
pub(super) fn parse_panic_header(line: &str) -> Option<(String, String)> {
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

pub(super) fn is_instrumentation_line(l: &str) -> bool {
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
