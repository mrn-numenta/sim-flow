#!/usr/bin/env bash
#
# Publishes coverage data to the orphan `coverage-reports` branch on every
# successful master push. The branch holds:
#   - latest/{coverage-summary.txt,coverage.json,html/}  — most recent report
#   - history.csv                                         — append-only series
#   - TREND.md                                            — Mermaid chart over the last N entries
#   - README.md                                           — explainer
#
# Inputs (from CI env):
#   GITHUB_TOKEN       - Actions token with `contents: write` on this repo
#   GITHUB_REPOSITORY  - owner/repo
#   GITHUB_SHA         - commit being published from
#
# Reads from ${ROOT_DIR}/target/llvm-cov/ — the output of `./scripts/checks.sh coverage`.

set -euo pipefail

TREND_WINDOW="${TREND_WINDOW:-30}"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COVERAGE_DIR="${ROOT_DIR}/target/llvm-cov"

for var in GITHUB_TOKEN GITHUB_REPOSITORY GITHUB_SHA; do
  if [ -z "${!var:-}" ]; then
    echo "::error::Required env var ${var} is not set." >&2
    exit 1
  fi
done

for f in coverage.json coverage-summary.txt; do
  if [ ! -f "${COVERAGE_DIR}/${f}" ]; then
    echo "::error::Missing ${COVERAGE_DIR}/${f}; coverage run did not produce expected output." >&2
    exit 1
  fi
done

PERCENT=$(jq -r '.data[0].totals.lines.percent | (.*100|floor)/100' "${COVERAGE_DIR}/coverage.json")
COVERED=$(jq -r '.data[0].totals.lines.covered' "${COVERAGE_DIR}/coverage.json")
COVERABLE=$(jq -r '.data[0].totals.lines.count' "${COVERAGE_DIR}/coverage.json")
DATE=$(date -u +"%Y-%m-%d")
SHORT_SHA="${GITHUB_SHA:0:8}"

WORK=$(mktemp -d)
trap 'rm -rf "${WORK}"' EXIT

# Clone the reports branch if it exists, otherwise start a fresh orphan.
# The token is embedded in the remote URL only for this clone/push; nothing
# is persisted to disk-level git config beyond the tempdir.
REMOTE_URL="https://x-access-token:${GITHUB_TOKEN}@github.com/${GITHUB_REPOSITORY}.git"
git -C "${WORK}" init -q
git -C "${WORK}" remote add origin "${REMOTE_URL}"
git -C "${WORK}" config user.name "sim-flow-ci[bot]"
git -C "${WORK}" config user.email "ci@numenta.local"

if git -C "${WORK}" fetch --depth=1 origin coverage-reports 2>/dev/null; then
  git -C "${WORK}" checkout -q -B coverage-reports FETCH_HEAD
  echo "[publish-coverage] updating existing coverage-reports branch"
else
  git -C "${WORK}" checkout -q --orphan coverage-reports
  echo "[publish-coverage] creating new coverage-reports branch"
fi

cd "${WORK}"

# Compute delta vs the most recent recorded measurement *before* appending the
# new row to history.
if [ -f history.csv ] && [ "$(wc -l < history.csv)" -gt 1 ]; then
  PREV_LINE=$(tail -1 history.csv)
  PREV_PERCENT=$(echo "${PREV_LINE}" | cut -d, -f3)
  PREV_SHA=$(echo "${PREV_LINE}" | cut -d, -f2)
  DELTA=$(awk -v cur="${PERCENT}" -v prev="${PREV_PERCENT}" 'BEGIN{printf "%+.2f", cur-prev}')
  if awk -v d="${DELTA}" 'BEGIN{exit !(d+0>0)}'; then ARROW="↑"
  elif awk -v d="${DELTA}" 'BEGIN{exit !(d+0<0)}'; then ARROW="↓"
  else ARROW="—"
  fi
  DELTA_LINE="Δ ${DELTA}% ${ARROW} from \`${PREV_SHA}\`"
else
  DELTA_LINE="(first recorded measurement)"
fi

# Replace latest/ wholesale; mv-then-rm pattern would be racier and we don't
# care about half-states because nothing reads this branch live.
rm -rf latest
mkdir -p latest
cp "${COVERAGE_DIR}/coverage-summary.txt" latest/
cp "${COVERAGE_DIR}/coverage.json" latest/
cp -r "${COVERAGE_DIR}/html" latest/

# Append new history row
if [ ! -f history.csv ]; then
  echo "date,sha,percent,covered,coverable" > history.csv
fi
echo "${DATE},${SHORT_SHA},${PERCENT},${COVERED},${COVERABLE}" >> history.csv

# Build the windowed Mermaid chart from the tail of history.csv. Each x-axis
# label is "<date>·<short-sha-prefix>" so consecutive same-day commits stay
# distinguishable.
WINDOW=$(tail -n+2 history.csv | tail -n "${TREND_WINDOW}")
WINDOW_COUNT=$(printf '%s\n' "${WINDOW}" | grep -c '^')
X_AXIS=$(printf '%s\n' "${WINDOW}" | awk -F, '{printf "%s\"%s·%s\"", (NR>1?",":""), $1, substr($2,1,6)}')
Y_VALUES=$(printf '%s\n' "${WINDOW}" | awk -F, '{printf "%s%s", (NR>1?",":""), $3}')

# Auto-scale y so a 2-point swing isn't lost on a 0-100 axis. Pad ±2, clamp 0/100.
Y_MIN=$(printf '%s\n' "${WINDOW}" | awk -F, 'BEGIN{m=100}{if($3+0<m)m=$3+0}END{v=int(m-2);if(v<0)v=0;print v}')
Y_MAX=$(printf '%s\n' "${WINDOW}" | awk -F, 'BEGIN{m=0}{if($3+0>m)m=$3+0}END{v=int(m+2);if(v>100)v=100;print v}')

cat > TREND.md <<EOF
# Coverage Trend — sim-flow

Latest: **${PERCENT}%** (${COVERED} / ${COVERABLE} lines)
Commit: \`${SHORT_SHA}\` · ${DATE}
${DELTA_LINE}

\`\`\`mermaid
xychart-beta
    title "Crate line coverage — last ${WINDOW_COUNT} master commits"
    x-axis [${X_AXIS}]
    y-axis "Coverage %" ${Y_MIN} --> ${Y_MAX}
    line [${Y_VALUES}]
\`\`\`

## Latest summary

\`\`\`
$(cat latest/coverage-summary.txt)
\`\`\`

## Drill-down (per-file HTML report)

\`latest/html/index.html\` in this branch is the full \`cargo-llvm-cov\` HTML
report with source-line coloring. GitHub doesn't render HTML in this repo
(private, Team plan — no Pages), so view it locally:

\`\`\`
git clone --depth=1 --branch coverage-reports git@github.com:${GITHUB_REPOSITORY}.git coverage
cd coverage && python3 -m http.server -d latest/html
\`\`\`

Then open <http://localhost:8000>.

## History

Full series: [history.csv](./history.csv). One row per successful master CI run.
EOF

cat > README.md <<'EOF'
# sim-flow coverage reports

Orphan branch published by sim-flow CI on every successful push to `master`.
It is **not** part of the project source tree — `master` does not see this
branch, and this branch does not see `master`.

- **[TREND.md](./TREND.md)** — Latest coverage % + Mermaid trend chart over the
  last 30 master commits. **Start here.**
- `latest/` — Most recent report:
  - `coverage-summary.txt` — full text summary
  - `coverage.json` — machine-readable totals + per-file numbers
  - `html/` — `cargo-llvm-cov` HTML report (browse locally; see TREND.md)
- `history.csv` — Append-only `date,sha,percent,covered,coverable`.

The CI step that maintains this branch lives in `.github/workflows/ci.yml`
(Coverage Gate job, `Publish coverage history` step) on `master`, and the
script that builds the commit is `scripts/publish-coverage.sh` on `master`.
EOF

git add -A
if git diff --staged --quiet; then
  echo "[publish-coverage] no changes; skipping commit"
  exit 0
fi

git commit -q -m "coverage: ${PERCENT}% from ${SHORT_SHA}"
git push -q origin coverage-reports
echo "[publish-coverage] pushed ${PERCENT}% from ${SHORT_SHA}"
