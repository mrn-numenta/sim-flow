//! Tests for the runners module. Reaches across submodules via
//! `super::*` so the test file can exercise both the public API
//! (summarize_*, PostWorkCargoReport) and the pub(super)-scoped
//! helpers (parse_panic_header, tail, format_command).

use super::cargo_test::{extract_panics, is_instrumentation_line, parse_panic_header};
use super::spawn::{format_command, tail};
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
fn summarize_exposes_failing_test_names_deduped() {
    // Three failing tests across two locations -- the summary's
    // `failing_tests` field should list all three (one entry per
    // unique test name) in first-seen order so the orchestrator
    // can intersect / diff with a prior target set.
    let stdout = "\
thread 'alpha' panicked at tests/stress.rs:10:5:
assertion `left == right` failed
  left: 0
 right: 1

thread 'beta' panicked at tests/stress.rs:20:5:
assertion `left == right` failed
  left: 2
 right: 3

thread 'gamma' panicked at tests/stress.rs:10:5:
assertion `left == right` failed
  left: 0
 right: 1

test result: FAILED. 2 passed; 3 failed; 0 ignored
";
    let summary =
        summarize_cargo_test_failures(stdout, "").expect("summary should be produced for failures");
    assert_eq!(summary.failure_count, 3);
    assert_eq!(summary.failing_tests, vec!["alpha", "beta", "gamma"]);
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
    let s =
        summarize_cargo_test_failures(stdout, "").expect("two failures should produce a summary");
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
    let s =
        summarize_clippy_diagnostics("", stderr).expect("should produce a summary for one warning");
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
fn format_command_joins_cmd_and_args_with_spaces() {
    assert_eq!(format_command("cargo", &[]), "cargo");
    assert_eq!(format_command("cargo", &["check"]), "cargo check");
    assert_eq!(
        format_command("cargo", &["test", "--lib"]),
        "cargo test --lib"
    );
}

#[test]
fn is_instrumentation_line_recognizes_framework_noise() {
    for line in [
        "--- timing summary ---",
        "--- host counters ---",
        "elaboration: 12ms",
        "  simulation: 5ms",
        "total: 17ms",
        "cycles: 1000",
        "throughput: 0.88 MHz",
        "elab insn: 4321",
        "sim insn: 99",
        "insn/sim-cycle: 12.3",
    ] {
        assert!(is_instrumentation_line(line), "{line}");
    }
    for line in [
        "running 1 test",
        "test foo::bar ... ok",
        "thread 'main' panicked",
        "",
        "// just a comment",
    ] {
        assert!(!is_instrumentation_line(line), "{line}");
    }
}

#[test]
fn parse_panic_header_extracts_thread_and_location() {
    let line = "thread 'foo::bar' (12345) panicked at tools/sim-flow/src/lib.rs:42:9:";
    let (file, loc) = parse_panic_header(line).expect("parse");
    assert_eq!(loc, "tools/sim-flow/src/lib.rs:42:9");
    // The first element is whatever the parser pulls before the
    // location; just sanity-check it's not empty.
    assert!(!file.is_empty());
}

#[test]
fn parse_panic_header_returns_none_for_non_panic_line() {
    assert!(parse_panic_header("running 1 test").is_none());
    assert!(parse_panic_header("").is_none());
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
