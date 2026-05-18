//! Per-turn progress classification + stuck-loop normalization.
//!
//! Splits two related concerns out of the turn loop. `ProgressClass`
//! / `classify_progress` decide whether a turn counts as a fix
//! attempt, real progress, or pure investigation -- it drives the
//! auto-mode no-progress cap. `normalized_response_hash` and
//! `normalize_for_loop_detection` compress an assistant response
//! down to a structural fingerprint so the runaway-loop guard can
//! catch "same error, shifting numbers" repeat-spam without the
//! literal-bytes match it used to require. Both are pure functions
//! so they're directly unit-tested below.

/// Per-turn progress classification used by the auto-iter no-progress
/// cap. Pure function on the inputs the orchestrator loop has at hand:
/// the session's target failing-test set, the latest failing set, and
/// whether the turn touched any path the step has already touched
/// in a prior session / earlier turn (the step's pre-session manifest).
///
/// Decision table:
/// - `target ∩ current` shrank AND no regression -> `Progress`
/// - the turn touched an existing path           -> `FixAttemptNoProgress`
/// - otherwise                                   -> `Investigation`
///
/// "Regression" means the current failing set contains a test that
/// wasn't in `target` -- a new failure introduced by the turn. With a
/// regression we don't claim progress even if some target tests
/// individually started passing; net behavior on the workspace got
/// worse.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ProgressClass {
    /// Target failing set strictly shrank and no new failures
    /// appeared outside the target. The orchestrator resets
    /// `no_progress_iters` and rebases `target_failing_set` to the
    /// new (smaller) set.
    Progress,
    /// Turn modified a path the step already owned, but the target
    /// failing set didn't shrink (or a regression was introduced).
    /// Counts toward `no_progress_iters`.
    FixAttemptNoProgress,
    /// Turn only created new files / only read / didn't touch any
    /// existing path. Counts toward `investigation_only_iters`
    /// (capped separately so the agent can't loop on diagnostics
    /// forever).
    Investigation,
}

pub(super) fn classify_progress(
    target: &std::collections::HashSet<String>,
    current: &std::collections::HashSet<String>,
    touched_existing: bool,
    declared: bool,
) -> ProgressClass {
    let still_failing_target = target.intersection(current).count();
    let target_shrank = still_failing_target < target.len();
    let regressed = current.difference(target).count() > 0;
    if target_shrank && !regressed {
        ProgressClass::Progress
    } else if touched_existing || declared {
        ProgressClass::FixAttemptNoProgress
    } else {
        ProgressClass::Investigation
    }
}

/// Decide whether to surface the one-time test-expectation nudge.
/// Fires when the agent has declared at least `threshold` fixes AND
/// at least one of those fix attempts produced no progress AND the
/// nudge hasn't already been emitted this session. Pure function so
/// it's covered by unit tests without spinning up the auto-mode
/// loop.
pub(super) fn should_emit_expectation_nudge(
    declared_fixes_count: u32,
    no_progress_iters: u32,
    already_emitted: bool,
    threshold: u32,
) -> bool {
    !already_emitted && declared_fixes_count >= threshold && no_progress_iters > 0
}

/// Compute a hash of `text` after normalizing away churn that varies
/// turn-to-turn while the structural content stays the same:
///
/// - Runs of ASCII digits collapse to `<N>` (eats timestamps, byte
///   counts, line numbers, retry indices, durations, exit codes).
/// - Runs of whitespace collapse to a single space (different
///   indentation / line wrapping doesn't defeat the comparison).
///
/// Used by the stuck-loop guard. Two responses that differ only in
/// numbers and whitespace map to the same hash and trip the guard.
pub(super) fn normalized_response_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let normalized = normalize_for_loop_detection(text);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    normalized.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn normalize_for_loop_detection(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_digit_run = false;
    let mut in_ws_run = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            if !in_digit_run {
                out.push_str("<N>");
                in_digit_run = true;
            }
            in_ws_run = false;
            continue;
        }
        in_digit_run = false;
        if ch.is_whitespace() {
            if !in_ws_run {
                out.push(' ');
                in_ws_run = true;
            }
            continue;
        }
        in_ws_run = false;
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod progress_class_tests {
    use super::*;
    use std::collections::HashSet;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn shrunk_target_no_regression_is_progress() {
        let target = set(&["a", "b", "c"]);
        let current = set(&["a", "b"]); // c got fixed
        assert_eq!(
            classify_progress(&target, &current, true, false),
            ProgressClass::Progress
        );
        // touched_existing irrelevant when target shrinks cleanly.
        assert_eq!(
            classify_progress(&target, &current, false, false),
            ProgressClass::Progress
        );
        // declared doesn't override Progress either.
        assert_eq!(
            classify_progress(&target, &current, false, true),
            ProgressClass::Progress
        );
    }

    #[test]
    fn target_shrank_with_regression_is_fix_attempt() {
        // Fixed `c` but introduced `d`. Net not better -> the agent
        // has work to do; counts as fix attempt with no progress.
        let target = set(&["a", "b", "c"]);
        let current = set(&["a", "b", "d"]);
        assert_eq!(
            classify_progress(&target, &current, true, false),
            ProgressClass::FixAttemptNoProgress
        );
    }

    #[test]
    fn unchanged_target_with_touch_is_fix_attempt() {
        let target = set(&["a", "b"]);
        let current = set(&["a", "b"]);
        assert_eq!(
            classify_progress(&target, &current, true, false),
            ProgressClass::FixAttemptNoProgress
        );
    }

    #[test]
    fn unchanged_target_without_touch_is_investigation() {
        // Agent ran cargo test but didn't edit any existing
        // artifact this turn: data collection.
        let target = set(&["a", "b"]);
        let current = set(&["a", "b"]);
        assert_eq!(
            classify_progress(&target, &current, false, false),
            ProgressClass::Investigation
        );
    }

    #[test]
    fn added_diagnostic_failures_without_touch_is_investigation() {
        // Agent added a new test that fails (a probe). target still
        // intact, no fix attempt -> investigation.
        let target = set(&["a", "b"]);
        let current = set(&["a", "b", "diag_probe"]);
        assert_eq!(
            classify_progress(&target, &current, false, false),
            ProgressClass::Investigation
        );
    }

    #[test]
    fn added_diagnostic_failures_with_touch_counts_as_fix_attempt() {
        // Adding diag PLUS editing an existing artifact -> the agent
        // did try a fix; the touch dominates. Conservative: this
        // increments the fix-attempt counter so a model that opens
        // an existing file but adds diag-only failures (net worse)
        // is still bounded.
        let target = set(&["a", "b"]);
        let current = set(&["a", "b", "diag_probe"]);
        assert_eq!(
            classify_progress(&target, &current, true, false),
            ProgressClass::FixAttemptNoProgress
        );
    }

    #[test]
    fn empty_target_means_first_sample_is_a_pass_through() {
        // First test run of the session: target = current. Both
        // sets equal, target didn't shrink. With no touch this is
        // investigation; with touch it'd be a fix attempt -- both
        // are fine, the loop sets target on this very iteration.
        let target = set(&["a"]);
        let current = set(&["a"]);
        assert_eq!(
            classify_progress(&target, &current, false, false),
            ProgressClass::Investigation
        );
    }

    #[test]
    fn declared_fix_promotes_no_touch_turn_to_fix_attempt() {
        // Agent declared but didn't edit existing -- still a fix
        // attempt (agent's commit is the signal).
        let target = set(&["a", "b"]);
        let current = set(&["a", "b"]);
        assert_eq!(
            classify_progress(&target, &current, false, true),
            ProgressClass::FixAttemptNoProgress
        );
    }

    #[test]
    fn declared_fix_and_touch_compose_as_fix_attempt() {
        // Both signals say "fix attempt" -- classification stays
        // FixAttempt (single classification, counters are parallel).
        let target = set(&["a", "b"]);
        let current = set(&["a", "b"]);
        assert_eq!(
            classify_progress(&target, &current, true, true),
            ProgressClass::FixAttemptNoProgress
        );
    }

    #[test]
    fn expectation_nudge_fires_after_threshold_with_no_progress() {
        // 4 declared fixes, 2 no-progress iters, not yet emitted,
        // threshold 4 -> fire.
        assert!(should_emit_expectation_nudge(4, 2, false, 4));
    }

    #[test]
    fn expectation_nudge_skipped_below_threshold() {
        // 3 declared fixes < threshold of 4. Doesn't fire yet -- the
        // agent gets a few free declared fixes before the reframing
        // lands.
        assert!(!should_emit_expectation_nudge(3, 5, false, 4));
    }

    #[test]
    fn expectation_nudge_skipped_when_no_progress_zero() {
        // 4 declared fixes but no_progress_iters == 0 means the
        // agent IS making progress (or has just started); no nudge
        // needed.
        assert!(!should_emit_expectation_nudge(4, 0, false, 4));
    }

    #[test]
    fn expectation_nudge_skipped_once_already_emitted() {
        // Even if all other conditions are met, the nudge fires at
        // most once per session.
        assert!(!should_emit_expectation_nudge(99, 99, true, 4));
    }

    #[test]
    fn normalize_strips_runs_of_digits() {
        // Catches timestamps, byte counts, retry indices, exit codes.
        // Two messages that differ only in numbers should normalize
        // identically.
        let a = "compile failed at 12:34:56 (exit 1, 4096 bytes)";
        let b = "compile failed at 18:02:11 (exit 7, 12 bytes)";
        assert_eq!(
            normalize_for_loop_detection(a),
            normalize_for_loop_detection(b)
        );
        assert!(normalize_for_loop_detection(a).contains("<N>"));
    }

    #[test]
    fn normalize_collapses_whitespace_runs() {
        let a = "error:  cannot find  thing";
        let b = "error: cannot find\n\tthing";
        assert_eq!(
            normalize_for_loop_detection(a),
            normalize_for_loop_detection(b)
        );
    }

    #[test]
    fn normalize_distinguishes_structurally_different_text() {
        let a = "compile error: missing import";
        let b = "compile error: type mismatch";
        assert_ne!(
            normalize_for_loop_detection(a),
            normalize_for_loop_detection(b)
        );
    }

    #[test]
    fn normalized_hash_matches_for_timestamp_only_diffs() {
        // The exact case the user warned about: spewing the same
        // error with shifting timestamps every retry. Hash should
        // match across turns even though no two messages are
        // byte-identical.
        let h1 = normalized_response_hash("Step DM2c failed at 2026-04-28T10:11:51Z (run 1)");
        let h2 = normalized_response_hash("Step DM2c failed at 2026-04-28T10:12:42Z (run 2)");
        let h3 = normalized_response_hash("Step DM2c failed at 2026-04-28T10:13:33Z (run 3)");
        assert_eq!(h1, h2);
        assert_eq!(h2, h3);
    }

    #[test]
    fn normalize_for_loop_detection_collapses_digit_runs() {
        // Different numbers should collapse to the same shape.
        let a = "ran 12345 cycles";
        let b = "ran 99 cycles";
        assert_eq!(
            normalize_for_loop_detection(a),
            normalize_for_loop_detection(b),
        );
        // The marker form is `<N>` (verify shape).
        assert!(normalize_for_loop_detection("count=42").contains("<N>"));
    }

    #[test]
    fn normalize_for_loop_detection_collapses_whitespace_runs() {
        let a = "hello   world";
        let b = "hello world";
        assert_eq!(
            normalize_for_loop_detection(a),
            normalize_for_loop_detection(b),
        );
    }
}
