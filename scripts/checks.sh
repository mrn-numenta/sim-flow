#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-all}"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

run_cmd() {
  local label="$1"
  shift
  echo "[checks] ${label}"
  "$@"
}

run_fmt() {
  run_cmd "cargo fmt --all --check" cargo fmt --all --check
}

run_clippy() {
  run_cmd "cargo clippy --all-targets -- -D warnings" \
    cargo clippy --all-targets -- -D warnings
}

run_test() {
  run_cmd "cargo test" cargo test
}

# Fail if Cargo.toml has the local sim-foundation patch enabled. The patch
# block at the bottom of Cargo.toml is intentionally committed in commented
# form so contributors can uncomment it when iterating on foundation +
# sim-flow code together; landing it uncommented breaks CI (the baked
# /opt/sim-foundation.git mirror in the CI container can't resolve a
# local path).
run_no_local_patch() {
  local file="Cargo.toml"
  # Combined ERE: matches an uncommented `[patch."...sim-foundation..."]`
  # header, OR an uncommented `foundation-framework`/`block-diagram` dep
  # with a `path = ...` field. Lines that start with `#` (with optional
  # leading whitespace) are ignored — `^[[:space:]]*` would not match `#`
  # since the alternation below requires `[` or `f`/`b` as the first
  # non-space char.
  local pattern='^[[:space:]]*(\[patch\.[^]]*sim-foundation|(foundation-framework|block-diagram)[[:space:]]*=[[:space:]]*\{[^}]*path[[:space:]]*=)'
  local matches
  if matches=$(grep -nE "${pattern}" "${file}"); then
    echo "[checks] ERROR: ${file} has the local sim-foundation patch enabled:" >&2
    echo "${matches}" | sed 's/^/  /' >&2
    echo "[checks] Comment these lines out before pushing." >&2
    exit 1
  fi
  echo "[checks] no-local-patch: OK"
}

run_coverage() {
  local output_dir="target/llvm-cov"
  mkdir -p "${output_dir}"

  # Run instrumented tests; defer report generation so we can emit multiple
  # output formats from one set of profraws.
  run_cmd "cargo llvm-cov --no-report" \
    cargo llvm-cov --no-report

  run_cmd "cargo llvm-cov report --json --output-path ${output_dir}/coverage.json" \
    cargo llvm-cov report --json \
      --output-path "${output_dir}/coverage.json"

  # cargo-llvm-cov nests the HTML output under <output-dir>/html/, so point
  # --output-dir at the parent so the report lands at target/llvm-cov/html/.
  run_cmd "cargo llvm-cov report --html --output-dir ${output_dir}" \
    cargo llvm-cov report --html \
      --output-dir "${output_dir}"
  cp "${output_dir}/html/index.html" "${output_dir}/coverage-summary.html"

  if ! command -v jq >/dev/null 2>&1; then
    echo "jq is required to write ${output_dir}/coverage-summary.txt" >&2
    exit 2
  fi

  jq -r '
    def fmt_pct(p): ((p * 100 | floor) / 100 | tostring);
    .data[0] as $d
    | [
        "Coverage Summary",
        "Crate: \(fmt_pct($d.totals.lines.percent))% (\($d.totals.lines.covered)/\($d.totals.lines.count) lines)",
        "",
        "Files Under 80%:",
        (([$d.files[]
          | select(.summary.lines.count > 0)
          | {filename, percent: .summary.lines.percent, covered: .summary.lines.covered, count: .summary.lines.count}]
          | map(select(.percent < 80))
          | sort_by([.percent, .filename])
          | if length == 0 then ["  none"] else map("  \(.filename): \(fmt_pct(.percent))% (\(.covered)/\(.count))") end
        )[]),
        "",
        "All Files:",
        (([$d.files[]
          | select(.summary.lines.count > 0)
          | {filename, percent: .summary.lines.percent, covered: .summary.lines.covered, count: .summary.lines.count}]
          | sort_by(.filename)
          | map("  \(.filename): \(fmt_pct(.percent))% (\(.covered)/\(.count))")
        )[])
      ] | .[]
  ' "${output_dir}/coverage.json" > "${output_dir}/coverage-summary.txt"
}

case "${MODE}" in
  fmt)
    run_fmt
    ;;
  clippy)
    run_clippy
    ;;
  test)
    run_test
    ;;
  coverage)
    run_coverage
    ;;
  no-local-patch)
    run_no_local_patch
    ;;
  pre-commit)
    run_no_local_patch
    run_fmt
    run_clippy
    ;;
  pre-push)
    run_no_local_patch
    run_fmt
    run_clippy
    run_test
    ;;
  all)
    run_no_local_patch
    run_fmt
    run_clippy
    run_test
    ;;
  *)
    echo "Unknown mode: ${MODE}" >&2
    echo "Usage: scripts/checks.sh [fmt|clippy|test|coverage|no-local-patch|pre-commit|pre-push|all]" >&2
    exit 2
    ;;
esac
