# Finetuning corpus from sim-flow captures (scoping doc)

**Status:** draft / scoping. No code changes yet.
**Created:** 2026-05-16
**Owner:** mneilly@numenta.com
**Motivation:** sim-flow already emits almost everything needed
to assemble a finetuning corpus — `protocol.jsonl` captures,
`llm-metrics.jsonl`, `tool-timings.jsonl`, gate verdicts,
diagnostic events. The robustness study captures these on every
trial. What's missing is a deliberate export path that turns
those captures into training data, primarily for distilling
Opus/Sonnet traces into a smaller local model. First concrete
target: Qwen3.6-35b-a3b in the LM Studio lineup, since
[agent/adaptation.rs](../../src/__internal/session/agent/adaptation.rs)
already has a family profile for it and the robustness study
already exercises it end-to-end.

This doc does **not** propose code changes. The goal is to align
on (a) what's already captured vs. what's missing, (b) the
dataset shapes worth emitting (SFT, DPO/ORPO), (c) where labels
come from, and (d) a phased rollout that ships the SFT path
before the preference-pair path.

---

## 1. What's already captured

The capture surfaces already in production:

- **`protocol.jsonl`** via
  [`capture_host.rs`](../../src/__internal/session/capture_host.rs).
  `e2e_auto --capture-jsonl <PATH>` and
  `e2e_manual --capture-jsonl <PATH>` tee every protocol event in
  both directions. Each `RequestLlmResponse` carries the full
  `messages` + `tools` stack (= the prompt the model saw); the
  following `LlmEnd` / `AssistantText` carries the completion.
  This is the primary raw material.
- **`llm-metrics.jsonl`** via
  [`llm_metrics.rs`](../../src/__internal/session/llm_metrics.rs).
  Per-turn `(step, kind, backend, model, request_id, turn_index,
  wall_ms, finish_reason, prompt_bytes, completion_bytes,
  tokens_in, tokens_out, tokens_exact)`. Useful for filtering
  (drop pathologically long turns) and weighting.
- **`tool-timings.jsonl`** via
  [`tool_timings.rs`](../../src/__internal/session/tool_timings.rs).
  Per-tool wall time. Mostly orthogonal to training but lets us
  drop turns whose tool calls failed structurally.
- **`bug-log.jsonl`** via the bug-log subsystem. Optional signal.
- **`GateResult`** events. Tell us whether a step *actually
  landed* — the strongest "this trajectory was good" signal we
  have.
- **`Diagnostic`** events. Empty-response retries, allowlist
  rejections, identical-response-streak warnings, runaway-guard
  trips. These flag turns we should NOT imitate.
- **Robustness study analyzer** (in
  [model-robustness-study.md](model-robustness-study.md)). Specs
  a `study_analyze` binary that already needs to extract most of
  the labels a corpus exporter wants.

Everything required to assemble per-turn `(prompt, completion,
outcome)` rows is therefore on disk after a robustness-study
run; it just needs to be joined.

## 2. What's missing for finetuning

1. **Outcome labels per turn / per session.** We have raw events
   but no canonical labels: `gate_passed_eventually`,
   `was_rejected_by_orchestrator`, `terminated_by` (clean /
   auto-iter-cap / critique-iter-cap / runaway-guard).
2. **Normalization from `protocol.jsonl` → SFT-shaped rows.** A
   converter that produces the standard chat-format JSONL the
   trl / axolotl / HF stack expects.
3. **Quality filters.** Drop empty-response turns,
   `write-outside-allowlist` turns, `tool-call-bad-args` turns,
   `identical-response-streak` turns. Keep only turns from
   sessions whose step eventually passed its gate without
   flipping to manual.
4. **Preference pairs (DPO/ORPO).** The orchestrator's retry
   loops produce these naturally — a rejected response followed
   by a corrective nudge and a corrected response is a (rejected,
   chosen) pair with the same prompt prefix. Today this is only
   recoverable by pattern-matching the raw event stream; a
   structural marker would make the pairing deterministic.
5. **Run-level success summary.** A per-`protocol.jsonl` sibling
   `outcome.json`: steps advanced, anomalies tripped,
   terminated_by, per-turn quality flags. Overlaps almost
   entirely with the analyzer the robustness study already
   specs.
6. **A canonical dataset CLI.** `sim-flow dataset export --from
   <study-root|project-dir> --format sft|dpo --out <path>` so
   this isn't a heap of one-off scripts.
7. **Outbound prompt/response normalization.** The per-family
   runtime profiles in
   [agent/adaptation.rs](../../src/__internal/session/agent/adaptation.rs)
   already canonicalize incoming responses. The exporter should
   reuse those so the training data matches the canonical wire
   form, not whatever the source model happened to emit.

## 3. Dataset shapes

Two emit modes, sharing the same join pipeline.

### 3.1 SFT

One row per `(RequestLlmResponse, LlmEnd)` pair from a clean
run. Shape (HF / OpenAI `messages` style):

```jsonl
{
  "messages": [...system + history...],
  "tools": [...universal catalog...],
  "completion": {"text": "...", "tool_calls": [...]},
  "meta": {
    "step": "DM2c",
    "kind": "work",
    "backend": "anthropic",
    "model": "claude-opus-4-7",
    "turn_index": 3,
    "source_run": "study-2026-05-16/claude-opus-4-7/trial-04",
    "gate_passed_eventually": true
  }
}
```

Default filters:

- `gate_passed_eventually == true`
- `was_rejected_by_orchestrator == false`
- `finish_reason in {"stop", "tool_use"}` (not `"length"`)
- Tool calls landed cleanly (no follow-up `tool-call-bad-args`
  diagnostic)
- Containing run terminated with reason `"clean"` or
  `"step-advanced"` (not auto-iter-cap, not runaway-guard)

### 3.2 DPO / ORPO

Pairs with a shared prompt prefix:

```jsonl
{
  "prompt": {"messages": [...], "tools": [...]},
  "rejected": {"text": "...", "tool_calls": [...]},
  "chosen":   {"text": "...", "tool_calls": [...]},
  "meta": {
    "step": "DM2c",
    "kind": "work",
    "rejection_kind": "write-outside-allowlist",
    "source_run": "...",
    "rejected_turn": 5,
    "chosen_turn": 7
  }
}
```

The (rejected, chosen) pair is `(turn_n.completion,
turn_{n+k}.completion)` where the orchestrator marked turn `n`
as having triggered a corrective nudge, and turn `n+k` is the
first non-rejected continuation under that same prefix. The
prefix is everything up to and including the system + user
messages that preceded turn `n`. Critically, the corrective
nudge itself does **not** appear in the prompt — the goal is to
teach the model to produce `chosen` directly without needing the
nudge.

## 4. Where labels come from

| Label | Source |
| --- | --- |
| `gate_passed_eventually` | `GateResult` event for the step |
| `was_rejected_by_orchestrator` | Diagnostic events emitted in the same turn boundary (today) / structural marker (after §5) |
| `rejection_kind` | Diagnostic event payload (`empty-response`, `write-outside-allowlist`, `tool-call-bad-args`, ...) — same taxonomy as the robustness study |
| `terminated_by` | `SessionEnd.reason` |
| `anomaly_kind` (per turn) | Robustness study analyzer output |
| `finish_reason` | Already on each `llm-metrics.jsonl` row |
| `tokens_in/out`, `wall_ms` | Already on each `llm-metrics.jsonl` row |

The robustness study analyzer and the corpus labeler want
essentially the same per-turn record — they should be one
binary, not two.

## 5. The one orchestrator change: corrective-nudge marker

Today the orchestrator synthesizes user-role messages in
response to recoverable failures, and those messages are
distinguishable from real user input only by content pattern
match. That's brittle and forecloses cleanly partitioning the
corpus.

Proposal: a structural tag on each synthesized user message,
carried in the protocol's user-message envelope. Sources:

- Empty-retry nudge (after `MAX_EMPTY_RETRIES` short-circuit)
- Allowlist-rejection feedback (`write-outside-allowlist`)
- Bad-args feedback (`tool-call-bad-args`)
- Bare-JSON / fence salvage failure feedback
- Critique-retry feedback
- Loop-guard hint (strike-2 on `tool-call-loop`)
- `identical-response-streak` one-strike warning
- Structural-gate retry hint
- `edit_file` stale-`old_string` feedback

One enum-typed field on the synthesized message, surfaced on the
existing user-role event (no new event type). Models don't see
the tag — the prompt text is unchanged — so this is a
pure-metadata change. With this in place the DPO pairing is
deterministic: `corrective_nudge.is_some()` on a user message
means the immediately preceding assistant turn is a `rejected`,
and the next assistant turn is a candidate `chosen`.

This is the smallest change that unlocks reliable preference
pairs. Without it the exporter has to pattern-match diagnostic
events, which is workable but lossy.

## 6. Target model and why

**First target: Qwen3.6-35b-a3b.**

Rationale:

- Already exercised end-to-end in the robustness study (L1 +
  L2). We know it walks the flow, we know which anomalies it
  trips at what rate.
- `agent/adaptation.rs` has a `QWEN3_6_MODEL_FAMILY` runtime
  profile; the exporter can reuse it for outbound normalization
  so training data and inference time agree on chat-template
  shape, reasoning-tag handling, and tool-call serialization.
- Locally served (LM Studio). Free training rolls, free
  evaluation rolls, no API budget for the inner loop.
- Concrete payoff if it works: cuts Opus 4.7 dependency for the
  flow, which is the biggest per-run line item.

Subsequent targets (out of scope here): Gemma 4, Kimi-VL
thinking, in that order, gated on the Qwen run actually showing
a measurable anomaly-rate reduction.

## 7. Tooling surface

Three pieces, in order of dependency:

1. **`run-outcome` labeler.** Reads a `protocol.jsonl` (plus
   the sibling `llm-metrics.jsonl`), emits `outcome.json`:
   `steps_advanced`, `anomalies_tripped`, `terminated_by`,
   per-turn `(step, kind, turn_index, finish_reason,
   was_rejected, rejection_kind, gate_passed_eventually)`. Merge
   with the robustness-study `study_analyze` spec — these two
   want the same code path.
2. **`sim-flow dataset export`.**
   ```
   sim-flow dataset export
     --from <study-root|project-dir>
     --format sft|dpo
     --normalize <family-id>   # optional; default = source-family
     --out <path>
   ```
   Joins `protocol.jsonl` turns with `outcome.json` labels,
   applies filters per §3, normalizes per §8, emits JSONL.
3. **Training smoke** under `scripts/`. Python sibling to
   `run-robustness-study.sh` that hands an exported JSONL to
   `trl` (LoRA SFT) and confirms a Qwen3.6 base can ingest the
   data and produce a checkpoint. Acceptance: training loop
   runs end-to-end and eval loss decreases on a held-out slice.
   The training stack stays in scripts / Python; sim-flow owns
   data export only.

## 8. Normalization at export time

`agent/adaptation.rs` per-family profiles canonicalize on the
*inbound* side: incoming Qwen `<think>...</think>` tags get
stripped, bare-JSON critiques get fence-recovered, tool-call-as-
JSON-blob shapes get parsed back into structured calls. The
exporter should run the analogous transformation on the
completion side:

- Strip reasoning tags (or move them to a `meta.thinking` field
  outside the trained-on completion, depending on whether we
  want to teach reasoning or suppress it — see §10).
- Re-serialize tool calls into the family's canonical wire form
  (native tool-call JSON for OpenAI-shaped backends, fenced for
  legacy fallback).
- Drop the `corrective_nudge` tag from prompt messages before
  emit (it's metadata, not training input).

The bet: training data should match the *canonical* wire form
the orchestrator wants to see, not the raw form any one source
model happened to emit. Otherwise the finetune just learns the
source model's quirks.

## 9. Phased rollout

### Phase 0 — run-outcome labeler (shared with robustness study)

Build `study_analyze` + `run-outcome` as one binary. Emits the
per-run `outcome.json`. No exporter, no orchestrator change.
This unblocks both the robustness study's anomaly aggregation
and §1-2 of the corpus work.

### Phase 1 — SFT exporter

`sim-flow dataset export --format sft`. Joins `protocol.jsonl`
turns with Phase 0 labels, applies the §3.1 filters, normalizes
per §8, emits SFT JSONL. Targets the existing captured Opus
runs as input.

### Phase 2 — corrective-nudge marker

The one orchestrator change. Field on synthesized user messages,
surfaced in protocol events. No retraining of historical
captures — future captures (and the Phase 2-3 robustness study
runs) carry the marker; old captures keep working with the
pattern-match fallback.

### Phase 3 — DPO exporter

`sim-flow dataset export --format dpo`. Consumes Phase 2-tagged
captures preferentially; falls back to pattern-matching
diagnostics for older captures (lossy but non-zero yield).

### Phase 4 — training smoke

LoRA SFT on Qwen3.6-35b-a3b against a Phase 1 export.
Acceptance: training loop runs, checkpoint loads in LM Studio.
No claims about quality yet.

### Phase 5 — acceptance loop

Re-run the robustness study against the finetuned Qwen on the
same L1 (and L2) fixtures. Compare per-anomaly rates against
base Qwen on the same fixture from the existing study results.
The win condition is **reduced anomaly rates on the same
fixtures** (specifically `wrong-fence-info-string`,
`write-outside-allowlist`, `tool-call-bad-args`,
`thinking-tags-in-content`) and equal-or-better step-advance
depth. Perplexity is not a goal; flow performance is.

If Phase 5 shows no reduction, the diagnosis path is:

- Anomaly rate unchanged → data too noisy / filter too loose →
  tighten §3.1 filters, re-export.
- Anomaly rate unchanged AND filters already tight → SFT alone
  isn't enough → land Phase 3 DPO and try again.
- Step-advance depth worse → finetune destroyed general
  capability → reduce LoRA rank or training epochs, re-export
  with broader Opus-trace diversity.

## 10. Open questions

- **Volume.** Order of magnitude per run? Each Opus run on the
  L1 fixture appears to be ~80-200 LLM turns. K=20 production
  trials give 1600-4000 turns per fixture per model. Is that
  enough for a useful LoRA on a 35B model, or do we need to
  expand the fixture pool?
- **Per-step balance.** Heavy steps (DM2c, DM3a, DM3b) dominate
  turn counts. Should the exporter re-weight so DM0 / DM1 don't
  get drowned out, or is the natural distribution fine?
- **Critique vs. work.** Different prompt shapes, different
  output shapes. Train one model on both, or partition into two
  finetunes? Probably one model; cheaper to evaluate and the
  prompt itself disambiguates kind. Worth confirming once Phase
  1 lands and we can see the loss curve per kind.
- **Reasoning content.** Strip `<think>...</think>` from
  completions and lose the chain of thought, or keep it and
  teach the model to emit it? Qwen3.6's adaptation profile
  already normalizes inbound; outbound the choice is policy.
  Default: strip, since the orchestrator already discards it.
- **Native tool calls vs. fenced.** Most existing captures
  predate the
  [native-tool-calls migration](native-tool-calls-migration.md).
  Training on fenced data and then inferring with native tool
  calls is a mismatch. Options: (a) retrain after the migration
  lands, (b) emit dual-form training data, (c) emit only
  native-form data and accept smaller corpus. Lean (a); the
  migration is the bottleneck either way.
- **Multi-family dataset.** One finetune learning the canonical
  form, or per-family finetunes with family-specific
  normalization? Per-family is cleaner; canonical is cheaper.
  Start per-family (Qwen first), revisit once Phase 5 has data.
- **Held-out split.** Per-trial, per-fixture, or per-run? Per-
  trial leaks turn-level patterns across train/eval. Per-
  fixture is the safe default but limits eval volume given how
  few fixtures we have. Acceptable for the SFT smoke; tighter
  for the Phase 5 acceptance run.

## 11. What this is not

- **A model training plan.** Hyperparameters, LoRA rank, learning
  rate schedules, etc. live in the training stack, not
  sim-flow. The exporter's contract ends at JSONL.
- **A replacement for the robustness study.** The study
  *analyzes* anomalies and informs orchestrator hardening; this
  doc *exports* the same captures as training data. They share
  a labeler; they don't share a goal.
- **A benchmark.** Whether the finetuned model is "better" is
  answered by re-running the robustness study against it, not
  by a held-out perplexity metric. Flow-level acceptance only.
- **A commitment to a specific training framework.** trl /
  axolotl / unsloth are interchangeable downstream of the
  JSONL.
