#!/usr/bin/env bash
# Run `cargo llvm-cov` against the orchestrator with the coverage
# carve-out: bin/* diagnostic binaries, PTY-driven sessions, and
# trait-only files that have no runtime to exercise. The headline
# number from this script is what the >=85% target tracks. Without
# the carve-out the report bakes in ~3.5 KLOC of un-unit-testable
# binaries that hold the number ~20 points below where the
# unit-testable surface actually sits.
#
# Run with:
#   tools/sim-flow/scripts/coverage.sh                 # text summary
#   tools/sim-flow/scripts/coverage.sh --html          # browseable
#   tools/sim-flow/scripts/coverage.sh --json          # machine-readable
#
# Background: https://github.com/taiki-e/cargo-llvm-cov
#
# Note: the regex is matched against the report's source path
# (e.g. `bin/e2e_manual.rs`), not the on-disk path
# (`tools/sim-flow/src/bin/e2e_manual.rs`). cargo-llvm-cov strips
# the `src/` prefix in its output.

set -euo pipefail

# Files excluded from the 85%-coverage target. ANY change here
# must be matched in the coverage CI gate (when it lands) so the
# carve-out is enforced both at developer-laptop time and at
# review time.
#
# - bin/.* — diagnostic + e2e binaries (e2e_manual, e2e_auto,
#   study_analyze, dm_flow_smoke, probe_ingest, pty_inject_probe,
#   session_protocol_schema). Each is its own CLI entry point with
#   no unit-test seam.
# - auto_interactive\.rs — PTY-driven session driver, requires a
#   real terminal to test.
# - presenter\.rs — trait + Default impl, exercised everywhere but
#   has no runtime of its own.
# - test_validation\.rs — orphan helper (deleted in spirit; the
#   feature it serves no longer ships).
# - block_diagram\.rs — exercised by the on-the-side block-diagram
#   CLI; covered by integration tests not the orchestrator suite.
EXCLUDE_REGEX='(/src/bin/.*\.rs$|/auto_interactive\.rs$|/presenter\.rs$|/test_validation\.rs$|/block_diagram\.rs$)'

# All other args (--html, --json, --summary-only, etc.) pass
# through to cargo-llvm-cov. Scope coverage to the main crate surface:
# library code, integration tests, and the shipping `sim-flow` binary.
exec cargo llvm-cov -p sim-flow \
    --profile ci \
    --lib \
    --tests \
    --bin sim-flow \
    --ignore-filename-regex "$EXCLUDE_REGEX" \
    "$@"
