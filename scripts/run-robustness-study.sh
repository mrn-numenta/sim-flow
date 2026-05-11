#!/usr/bin/env bash
# Phase 0 driver for the model-robustness study.
# See `docs/brainstorming/model-robustness-study.md`.
#
# For each (model, trial) pair, this script:
#   1. Creates a fresh scratch project via `sim-flow new model`.
#   2. Stages the smoke fixture spec.
#   3. Runs `e2e_auto --capture-jsonl <study-root>/<model>/trial-<N>/protocol.jsonl`.
#   4. Captures stderr to `<study-root>/<model>/trial-<N>/stderr.log` so the
#      `sim_flow::metrics` tracing rollups land alongside the protocol JSONL.
#
# Usage:
#   ./run-robustness-study.sh <study-root> <backend> <base-url> <model> <K>
#
# Example (Phase 0 -- vLLM qwen3.6 at localhost:8012, K=3):
#   ./run-robustness-study.sh \
#       /tmp/robustness-2026-05-11 \
#       openai-compat \
#       http://localhost:8012/v1 \
#       qwen3.6 \
#       3
#
# Exit codes:
#   0  every trial completed cleanly (run_auto returned Ok)
#   1  at least one trial failed; check per-trial stderr.log
#   2  bad arguments / setup

set -u  # not -e: a single failed trial must not abort the rest of the run

STUDY_ROOT="${1:-}"
BACKEND="${2:-}"
BASE_URL="${3:-}"
MODEL="${4:-}"
K="${5:-3}"

if [[ -z "$STUDY_ROOT" || -z "$BACKEND" || -z "$BASE_URL" || -z "$MODEL" ]]; then
    echo "usage: $0 <study-root> <backend> <base-url> <model> <K>" >&2
    exit 2
fi

# Resolve repo root from this script's location so we can find
# sim-flow + the smoke spec fixture without hard-coded paths.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SIM_FLOW_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FOUNDATION_ROOT="$(cd "$SIM_FLOW_ROOT/../.." && pwd)"
SMOKE_SPEC="$SIM_FLOW_ROOT/src/bin/dm_flow_smoke_spec.md"
SIM_FLOW_BIN="$FOUNDATION_ROOT/target/debug/sim-flow"
E2E_AUTO_BIN="$FOUNDATION_ROOT/target/debug/e2e_auto"

for required in "$SIM_FLOW_BIN" "$E2E_AUTO_BIN" "$SMOKE_SPEC"; do
    if [[ ! -e "$required" ]]; then
        echo "missing: $required" >&2
        echo "(run \`cargo build -p sim-flow --bins\` first)" >&2
        exit 2
    fi
done

# Model slug used in path components. Replace any chars cargo /
# filesystems don't love (/, :).
MODEL_SLUG="$(echo "$MODEL" | tr '/:' '__')"
MODEL_ROOT="$STUDY_ROOT/$MODEL_SLUG"
mkdir -p "$MODEL_ROOT"

# Manifest line on the study root so future readers know what was run.
MANIFEST="$STUDY_ROOT/manifest.jsonl"
START_TS="$(date -u +%FT%TZ)"
printf '{"started_at":"%s","model":"%s","backend":"%s","base_url":"%s","K":%s,"spec":"%s"}\n' \
    "$START_TS" "$MODEL" "$BACKEND" "$BASE_URL" "$K" \
    "$SMOKE_SPEC" >> "$MANIFEST"

# Randomness control. vLLM, llama.cpp, and most openai-compat
# servers honor `seed` in the chat-completions body; the agent
# reads it from `SIM_FLOW_SEED`. We set `seed = trial_idx` so each
# trial is reproducible and any anomaly can be re-rolled with the
# same seed for debugging. Backends without a seed knob (claude
# CLI, some local servers) silently ignore the env var.
#
# Per-trial isolation. Every trial gets its own tempdir, so the
# project's `.sim-flow/state.toml`, `docs/`, `src/`, `target/`,
# and `.sim-flow/checkpoint.json` are entirely fresh. There is
# no shared per-model state, so a trial reaching DM2c can NOT
# bias the next trial toward also reaching DM2c -- they don't
# share state. Run-to-run variance you observe in the catalog is
# purely random sampling unless the seed is fixed.
#
# Thinking-control (optional). When `SIM_FLOW_DISABLE_THINKING=1`
# is exported before running this script, the agent adds
# `chat_template_kwargs.enable_thinking=false` to the request body
# for families with thinking-section chat templates (qwen3.6,
# deepseek-r1, ...). Saves tokens on every turn; pairs well with
# tight `max_tokens`. Not enabled by default for the study so
# trial captures stay comparable to Phase 0.
#
# Trial loop. We do NOT abort on a single trial failure; record the
# outcome to the per-model summary and move on so the analyzer sees
# the failure mode rather than us erasing it.
FAILURES=0
for ((TRIAL=1; TRIAL<=K; TRIAL++)); do
    TRIAL_DIR="$MODEL_ROOT/trial-$(printf '%02d' "$TRIAL")"
    mkdir -p "$TRIAL_DIR"

    # Fresh scratch project per trial. Tempdir keeps the model+toolchain
    # state isolated and means a trial's gate-write state can never
    # leak into the next trial's run.
    PROJECT_PARENT="$(mktemp -d -t "robustness-${MODEL_SLUG}.XXXXXX")"
    PROJECT_NAME="proj"
    PROJECT_DIR="$PROJECT_PARENT/$PROJECT_NAME"

    echo "=== model=$MODEL trial=$TRIAL/$K ==="
    echo "    project_dir   = $PROJECT_DIR"
    echo "    capture_jsonl = $TRIAL_DIR/protocol.jsonl"
    echo "    seed          = $TRIAL"

    # 1. Bootstrap a fresh model project (skip cargo check; the
    #    DM2d gate runs cargo anyway).
    if ! "$SIM_FLOW_BIN" --foundation-root "$FOUNDATION_ROOT" \
            new model "$PROJECT_NAME" \
            --destination "$PROJECT_PARENT" \
            --library-path "$FOUNDATION_ROOT/../sim-models" \
            --skip-cargo-check \
            > "$TRIAL_DIR/setup.log" 2>&1; then
        echo "  FAILED: new_model bootstrap (see $TRIAL_DIR/setup.log)" >&2
        FAILURES=$((FAILURES + 1))
        continue
    fi

    # 2. Run e2e_auto with capture. Stderr (tracing rollups +
    #    orchestrator diagnostics) goes to a sibling .log.
    #    `SIM_FLOW_SEED` is read by the openai-compat agent and
    #    forwarded as `seed` in the chat-completions body.
    #    `SIM_FLOW_DISABLE_THINKING` (when exported by the
    #    caller) toggles `chat_template_kwargs.enable_thinking`.
    TRIAL_START="$(date +%s)"
    if SIM_FLOW_SEED="$TRIAL" \
            "$E2E_AUTO_BIN" \
            --foundation-root "$FOUNDATION_ROOT" \
            --project-dir "$PROJECT_DIR" \
            --backend "$BACKEND" \
            --base-url "$BASE_URL" \
            --model "$MODEL" \
            --spec "$SMOKE_SPEC" \
            --no-watch-socket \
            --capture-jsonl "$TRIAL_DIR/protocol.jsonl" \
            > "$TRIAL_DIR/stdout.log" \
            2> "$TRIAL_DIR/stderr.log"; then
        OUTCOME="ok"
    else
        OUTCOME="error"
        FAILURES=$((FAILURES + 1))
    fi
    TRIAL_END="$(date +%s)"
    WALL_S=$((TRIAL_END - TRIAL_START))

    printf '{"trial":%d,"outcome":"%s","wall_s":%d,"seed":%d,"disable_thinking":%s,"project_dir":"%s"}\n' \
        "$TRIAL" "$OUTCOME" "$WALL_S" "$TRIAL" \
        "$([ "${SIM_FLOW_DISABLE_THINKING:-}" = "1" ] && echo true || echo false)" \
        "$PROJECT_DIR" \
        >> "$MODEL_ROOT/trials.jsonl"

    echo "    -> $OUTCOME in ${WALL_S}s"

    # Keep the project dir around for forensics. If disk pressure
    # ever bites we can add a flag to purge here.
done

END_TS="$(date -u +%FT%TZ)"
printf '{"ended_at":"%s","model":"%s","K":%s,"failures":%d}\n' \
    "$END_TS" "$MODEL" "$K" "$FAILURES" >> "$MANIFEST"

echo
echo "=== summary: $MODEL: $((K - FAILURES))/$K trials clean ==="
echo "    study root: $STUDY_ROOT"

if [[ "$FAILURES" -gt 0 ]]; then
    exit 1
fi
exit 0
