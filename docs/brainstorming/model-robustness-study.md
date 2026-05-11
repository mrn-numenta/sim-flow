# Model Robustness Study (brainstorm)

**Status:** brainstorm. Nothing here is committed-to behavior; we'll
turn the parts we agree on into a plan + skill before any code lands.

## Goal

Catalog the kinds of inconsistencies and ill-formed responses we
should expect from each LLM we drive `sim-flow auto` against, then
use that catalog to make the orchestrator + prompts robust to them.
Two intended outputs:

1. **Per-model failure-mode profile**: a structured list of "model X
   in step Y produces shape Z at rate R", so we can decide where to
   add adaptation logic, tighten a prompt, or just document that
   the model isn't a good fit.
2. **Replay corpus**: captured (request, response) pairs we can
   re-feed through the orchestrator off-line, so we can iterate on
   parser / fallback / adaptation changes without burning tokens
   every revision.

## Out of scope (for this doc)

- Picking which models we ship-bless. The study informs that
  decision; it doesn't make it.
- Benchmarking "quality of output." A spec the model authored might
  pass every gate and still be a bad spec. That's the e2e binaries'
  job against domain experts.
- Cost-per-step accounting. `api-cost-management.md` already owns
  that. The capture format below piggybacks the same per-call
  metrics so we don't double-instrument.

## Approach in one paragraph

Run `e2e_auto` against each candidate model on a small, stable
fixture (start with `dm_flow_smoke_spec.md`); capture every
`RequestLlmResponse` / `AssistantText` / `ToolInvoked` /
`Diagnostic` / `GateResult` event from the orchestrator's JSONL
stream to disk; post-process the stream into a structured anomaly
log keyed by `(model, step, sub-session, turn-index, anomaly-kind)`;
aggregate across runs to produce the per-model profile; archive
the raw turns as a replay corpus.

## What we already have (signal, not noise)

We do not need to add new tracing for most of this. The orchestrator
already emits:

- **Wire-protocol events** (`Event::*` written to the host). Every
  `RequestLlmResponse` carries the prompt stack; every
  `AssistantText` carries the response; `ToolInvoked` carries
  per-tool status; `ArtifactWritten` records persisted writes;
  `GateResult` carries the gate verdict per step; `Diagnostic`
  surfaces orchestrator-side anomalies (allowlist rejections,
  empty-response retries, cap-exceeded, runaway-guard, identical-
  response streaks, ...).
- **`sim_flow::metrics` tracing events**: `llm_call`, `turn_end`,
  `critique_pass`, `step_end`, `milestone_tasks_auto_ticked`,
  `post_work_cargo_checks`, `post_work_cargo_failed`,
  `critique_retry_block_truncated`. These cover the higher-level
  step / pass-level signal.

The JSONL stream is the source of truth; the tracing events are
the per-step rollup. The study tooling reads both.

Mocked e2e tests in `tests/e2e_mocked.rs` exercise the
orchestrator's transitions against `MockAgent`; the study uses the
same orchestrator code paths against real models.

## Capture plan

### Tap point

Add a `--capture-jsonl <PATH>` flag to `e2e_auto` (and `e2e_manual`)
that writes a copy of every `Event` and every `HostEvent` to a
JSONL file as they cross the protocol boundary. We already have
`EventTap` for read-only socket broadcasts; reuse it. One JSONL
file per run, named
`<study-root>/<model-slug>/<run-id>/protocol.jsonl`. Stable shape:

```jsonl
{"ts": <unix_ms>, "dir": "out", "event": {...}}    // orchestrator -> host
{"ts": <unix_ms>, "dir": "in",  "event": {...}}    // host -> orchestrator
```

Per-call metrics from `LlmCallMetrics` (`tokens_in`, `tokens_out`,
`wall_ms`) ride on the existing `tracing::info!(target =
"sim_flow::metrics", event = "llm_call", ...)` event; same flag
also opens a second file `metrics.jsonl` and pipes that subscriber
to it.

### Fixture choice

Three layers, in order of decreasing cost and increasing realism:

- **L1 small fixture**: a tiny one-page spec (`dm_flow_smoke_spec.md`)
  that walks the full flow in minutes. Default for the study.
- **L2 mid fixture**: a 5-10 page spec that triggers spec-pages
  ingestion + paginated reads, so we see how each model handles
  `read_file lib:`-prefix lookups, partial reads, and the spec TOC.
- **L3 real model project**: one of the `sim-models/users/...`
  projects. **Gated**: we do NOT run L3 until (a) the L1+L2 study
  has produced its anomaly catalog, (b) we've landed orchestrator /
  prompt / adaptation hardening against that catalog, and (c) the
  L1+L2 reruns confirm the changes actually reduced anomaly rates.
  Running L3 earlier just burns LLM time on failure modes we
  already know how to fix; running it after gives us "does the
  hardening hold up on a real spec" as the final acceptance signal.

We do not need the spec to be domain-realistic for L1/L2. We need
it to exercise every code path: every step's gate, every
milestone-walk mode (placeholder + execution), at least one
cargo-gated step.

### Run matrix

Per (model, fixture) we run K trials with varied randomness so we
measure variance, not a single roll. Two phases:

- **Phase 1 -- shake-out**: `K = 3`. Just enough to confirm the
  capture / analyzer pipeline works and to spot the loud failure
  modes. We don't trust rare-anomaly rates from 3 trials.
- **Phase 2 -- production**: `K = 20` once the pipeline is solid.
  This is what we report against. 20 trials lets us call out
  ~5% tail anomalies with reasonable confidence.

**Randomness control** (per backend):

- If the backend exposes a `seed` knob (OpenAI-compat / vLLM /
  llama.cpp, Anthropic via API), use it: `seed = trial_idx` so
  trials are reproducible and any anomaly we observe can be
  re-rolled with the same seed for debugging.
- If the backend doesn't expose a seed (some local servers,
  Claude Code CLI), sweep `temperature` instead across a
  reasonable band (e.g. `0.4, 0.6, 0.8`) cycled per trial. The
  per-model report records which method was used so we don't
  conflate "anomaly is rare under seed-fixed" with "anomaly is
  rare under temp-sweep."

Each trial captures its own JSONL. The aggregator computes
per-trial anomaly counts; per-model rollups carry medians +
stddev so a one-off weird run doesn't read as a systemic problem.

## Anomaly taxonomy (draft)

The shapes worth tagging. Each anomaly has a stable kind id so the
aggregator can roll up + visualize, and a short description of
what the orchestrator did about it (some are recoverable today,
some flip to manual).

### Protocol-shape anomalies

| kind                              | trigger                                                                                  | recovery today                                 |
| --------------------------------- | ---------------------------------------------------------------------------------------- | ---------------------------------------------- |
| `empty-response`                  | model returned zero-byte text                                                            | empty-retry nudge (up to `MAX_EMPTY_RETRIES`)  |
| `wrong-fence-info-string`         | fenced block with a path that doesn't match the artifact-write allowlist                 | rejected, fed back to agent as User turn       |
| `bare-json-no-fence`              | critique JSON inline as prose without `\`\`\`docs/critiques/...json` fence               | salvage path tries balanced-brace JSON extract |
| `markdown-critique-no-fence`      | critique session emits prose with `BLOCKER:` markers but no fenced artifact block        | legacy markdown fallback persists the prose    |
| `tool-call-bad-args`              | fenced `tool:` block with wrong arg shape (missing path, extra keys, ...)                | tool returns error result, fed back            |
| `write-outside-allowlist`         | agent fences a file outside `work_write_paths` (e.g. `src/` during DM0)                  | rejected + ToolInvoked status=error            |
| `tool-followup-after-write`       | agent narrates a write it didn't actually emit (no fenced block, says "I wrote spec.md") | structural gate stays dirty; auto-iter cap     |

### Drift / loop anomalies

| kind                              | trigger                                                                          | recovery today                          |
| --------------------------------- | -------------------------------------------------------------------------------- | --------------------------------------- |
| `identical-response-streak`       | normalized response hash repeats `max_identical_responses` times                 | runaway-guard SessionEnd                |
| `tool-call-loop`                  | same `read_file` call repeated turn after turn without making progress           | strike-2 loop-guard hint injected       |
| `no-progress-test-streak`         | repeated `cargo test` with non-decreasing failure count                          | no-progress cap; flip to manual         |
| `auto-iter-cap`                   | `max_auto_iters` consecutive turns without a fresh artifact                      | flip to manual                          |
| `critique-iter-cap`               | `max_critique_iters` retries and the critique still flags gate-failing findings  | flip to manual                          |

### Semantic / content anomalies (need authored, not just shape)

| kind                              | trigger                                                                                  | recovery today                       |
| --------------------------------- | ---------------------------------------------------------------------------------------- | ------------------------------------ |
| `milestone-rows-flipped-early`    | agent ticked `- [ ]` -> `- [x]` without the artifact landing                             | retry walker stays on same milestone |
| `milestone-deferred-as-x`         | agent wrote `- [-]` (deferred) on a step where `forbid_deferred=true`                    | gate fails on `MilestonesAllResolved`|
| `critique-clean-with-blockers`    | critique JSON body says "All clean" prose but findings array has blockers                | gate trips; no specific feedback     |
| `wrong-step-critique`             | agent wrote `docs/critiques/DM2-critique.md` during DM3a                                 | gate fails (file_exists wrong path)  |
| `lib-prefix-misuse`               | agent tried to write through a `lib:` path                                               | rejected by WriteFileTool            |
| `preamble-burning-budget`         | model spends N turns of CoT-style preamble before any tool call or artifact              | auto-iter cap eventually flips       |

### Adaptation-layer anomalies (Phase 10 stuff)

| kind                              | trigger                                                                                  | recovery today                                          |
| --------------------------------- | ---------------------------------------------------------------------------------------- | ------------------------------------------------------- |
| `thinking-tags-in-content`        | reasoning tags leaked into the surfaced text (`<thinking>...</thinking>`)                | normalize via `normalize_response_text` runtime profile |
| `tool-call-as-json-blob`          | model emits the structured-tool-call as a JSON object inside prose, not a real tool call | fenced-tool-call fallback might catch                   |
| `system-prompt-echoed`            | model repeats the system prompt verbatim back as the first turn                          | none today; treated as a regular turn                   |

The taxonomy list is the *deliverable* of the study's analyzer pass.
The shapes above are starting hypotheses; ground truth is what the
captured JSONL actually contains.

## Analyzer

A small Rust binary `study_analyze` next to `e2e_auto` that:

1. Reads a study root (`<study-root>/`) of `protocol.jsonl` files.
2. For each file, replays a state machine over the events to detect
   each anomaly kind above. Most are pattern matches on `Event::*`
   shapes; a few (drift, semantic) need to look across N turns.
3. Emits one `anomalies.jsonl` per run + a per-model aggregate
   `summary.json`:

```json
{
  "model": "qwen3-coder-6b@lmstudio",
  "fixture": "dm_flow_smoke_spec",
  "trials": 3,
  "steps_completed": {"median": 4, "min": 4, "max": 6},
  "anomalies": {
    "empty-response":             { "median_per_run": 1, "max": 4 },
    "write-outside-allowlist":    { "median_per_run": 0, "max": 1 },
    "identical-response-streak":  { "median_per_run": 0, "max": 1 },
    "preamble-burning-budget":    { "median_per_run": 6, "max": 11 }
  },
  "terminated_by": {"auto-iter-cap": 2, "critique-iter-cap": 1}
}
```

4. (Stretch) Renders a per-model markdown report with the worst
   offending turn quoted verbatim per anomaly kind, so we can scan
   the actual response shape and decide what to harden.

## Replay corpus

The same JSONL captures double as the corpus. To replay:

- Bind `MockAgent::from_corpus(<protocol.jsonl>)` to play back each
  `RequestLlmResponse`'s response text in order.
- Run the orchestrator with the same `OrchestratorOptions` recorded
  in the capture header.
- Compare the new event stream against the captured one. Any
  divergence is interesting: either we changed orchestrator
  behavior intentionally (recovered an anomaly that used to escape)
  or we regressed (used to handle it, now don't).

The corpus is also useful for `e2e_mocked.rs`-style transition
tests: a corpus run is the closest thing we have to "what a real
LLM would do" without running one, and lets us pin observed
failure modes in CI.

## Phased rollout

### Phase 0 -- pipeline shake-out (vLLM / qwen3-27b only, K=3)

Smallest experiment that exercises every piece of plumbing
without yet caring about model diversity.

1. Add `--capture-jsonl <PATH>` to `e2e_auto` (and `e2e_manual`).
   Most of the plumbing is `EventTap` write-to-file; per-call
   metrics tap the existing `sim_flow::metrics` tracing target.
2. Run K=3 trials of `qwen3-27b` served via vLLM at
   `http://localhost:8012/v1` (invoked via
   `e2e_auto --backend openai-compat --base-url
   http://localhost:8012/v1 --model qwen3-27b`) against
   `dm_flow_smoke_spec.md`. vLLM is the chosen shake-out backend
   because (a) it's already wired into `e2e_auto`, (b) it
   exposes a `seed` knob so the trial-reproducibility path gets
   exercised, and (c) it's local so we don't pay API for an
   experiment we expect to throw out.
3. Hand-eyeball the 3 captured JSONLs. Confirm the JSONL shape
   is what the analyzer will want to consume; confirm the
   pre-run `cargo check` smoke fires; confirm anomalies actually
   appear in the captures rather than getting swallowed.
4. Refine the taxonomy (drop entries that didn't fire, add ones
   we missed) BEFORE writing the analyzer -- if the analyzer
   ships first it locks in our prior hypotheses.

### Phase 1 -- LM Studio lineup (K=3, then K=20)

Once the pipeline shake-out is clean, expand to the LM Studio
locally-served lineup. All on `http://localhost:1234/v1` (LM
Studio's conventional OpenAI-compat port); pass each model id
via `--model` per trial:

- `google/gemma-4-26b-a4b`
- `kimi-vl-a3b-thinking-2506`
- `qwen/qwen3.6-35b-a3b`

These three give us coverage across the model families we already
adapt to in Phase 10's runtime profiles (Gemma, Kimi, Qwen), so
anomalies we see here directly inform the per-family adaptation
work. They're all locally-served, so we can run K=20 against each
without API cost. Order:

1. **Sub-phase 1a -- K=3 per model** to confirm each one walks
   the flow at all and to seed the per-model anomaly hypotheses.
2. **Sub-phase 1b -- K=20 per model** for the reported study.
   This is the run we draw conclusions from.

### Phase 2 -- hardening + replay validation

Use the Phase 1 catalog to land the first round of fixes:
prompt edits in `prompts/*.md`, parser tweaks in
`orchestrator.rs` (the salvage paths), runtime-profile additions
in `agent/adaptation.rs`. After each landed change, **replay the
captured corpus** (no new model runs) to confirm the anomaly is
caught + the orchestrator no longer trips. Only re-run real
models when replay alone can't tell us (e.g. we changed the
system prompt and need to see the model's new response).

### Phase 3 -- L3 acceptance (local lineup)

Run L3 (real / complex specs) against the surviving local lineup
with the hardening in place. This is the gate for "we're ready to
ship as a supported backend." See "Fixture choice" above for the
L3 gating criteria.

### Phase 4 -- Claude API (Opus 4.7), the paid run

Only after the local study (vLLM + LM Studio lineup) has been
through Phase 0 -> Phase 3 do we extend the same protocol to
the Claude API with `claude-opus-4-7`. The sequencing is
deliberate:

- Every taxonomy entry, capture format, analyzer feature, and
  hardening change shakes out on free local backends first. By
  the time we point at the paid API, we're not iterating on the
  tooling -- we're collecting the per-model profile.
- The hardening landed in Phase 2 reduces the rate at which
  Opus 4.7 trips the recoverable failure modes, so the paid
  runs spend their tokens on the model's actual behavior rather
  than on us re-discovering things qwen3-27b already showed us.
- K=3 shake-out on Opus 4.7 confirms the per-call metrics +
  capture format hold up over an HTTP API + streaming, then
  K=20 for the reported profile. Budget the K=20 ahead of time
  (`max_llm_requests` cap + run-level token budget) so a runaway
  loop doesn't burn through credits before we notice.
- Hosted Claude Sonnet (current production target) is already
  covered by the existing `e2e_auto --backend claude` runs;
  Opus 4.7 is the focus here.

## Cargo-failure attribution

Cargo-gated steps (DM2d / DM3b / DM3c / DM4b) can fail for reasons
that have nothing to do with the model: stale `target/`, toolchain
version drift, disk pressure, a transient registry timeout, a
foundation-framework API change the project hasn't caught up with
yet. We cannot let those count as model failures or the per-model
profile is garbage.

Plan to separate them:

1. **Pre-run smoke**: before each trial, run a `cargo check` on a
   pristine `new_model` bootstrap (no model writes yet). If that
   fails, the toolchain or the framework is the problem and the
   trial doesn't run. Surface the failure in the study log so we
   know to investigate, but it never counts against a model.
2. **Per-cargo-failure classification**: when a cargo gate trips
   during a trial, the analyzer reads the cargo stderr tail. We
   tag the failure as:
   - `cargo-fail-model`: error message references a path the
     model wrote in this session (we track persisted writes per
     turn from `Event::ArtifactWritten`), OR the failure is a
     compile error in `src/model/`, `src/sim.rs`, or `tests/` --
     all model-authored surfaces.
   - `cargo-fail-toolchain`: error references resolver, lockfile,
     registry, network, missing toolchain components.
   - `cargo-fail-framework`: error references
     `foundation-framework` itself or a `lib:` crate the project
     depends on but doesn't author.
   - `cargo-fail-ambiguous`: anything else. Hand-classify these
     when there are few; if `ambiguous` is more than ~10% we
     refine the heuristics.
3. **Report only the model class**. The per-model anomaly profile
   counts `cargo-fail-model` only. The other classes get a
   separate "study health" report so we can see when a toolchain
   regression is contaminating runs.

## Privacy / distribution

Study captures stay internal: project-relative paths inside the
JSONL can leak project structure, and any L3 run against a real
sim-models project carries the spec itself. No replay corpus
leaves the org without an explicit scrub pass. The L1 smoke fixture
is intentionally generic and safe to share; L2 will be authored
with that constraint in mind.

## Decision log

- **2026-05-11 -- trial count**: start at K=3 to get the pipeline
  working end-to-end; bump to K=20 for the production study. The
  smaller phase is just for pipeline shake-out; we don't report
  rates from it.
- **2026-05-11 -- randomness control**: prefer per-trial `seed =
  trial_idx` when the backend exposes it; fall back to a
  temperature sweep across `0.4 / 0.6 / 0.8` cycled per trial.
  Record which method was used per model.
- **2026-05-11 -- L3 timing**: hold real / complex specs until the
  L1+L2 study has produced its anomaly catalog AND the hardening
  has been landed AND L1+L2 reruns confirm anomaly rates dropped.
  L3 is the acceptance signal, not the discovery signal.
- **2026-05-11 -- cargo attribution**: explicitly distinguish
  `cargo-fail-model` from `cargo-fail-toolchain` /
  `cargo-fail-framework` / `cargo-fail-ambiguous`; only the first
  counts against a model in the profile. Pre-run smoke on a
  pristine bootstrap gates each trial so we don't blame a model
  for a broken toolchain.
- **2026-05-11 -- distribution**: study output is internal-only.
  Treat all replay corpora as project-internal artifacts.
- **2026-05-11 -- backend lineup**: shake-out on vLLM at
  `http://localhost:8012/v1` serving `qwen3-27b` (K=3). Phase 1
  lineup is three LM Studio locally-served models on
  `http://localhost:1234/v1`: `google/gemma-4-26b-a4b`,
  `kimi-vl-a3b-thinking-2506`, `qwen/qwen3.6-35b-a3b`. Local-only
  so K=20 is affordable. These four (vLLM qwen3-27b + the three
  LM Studio models) span the Phase-10 runtime family profiles
  (Qwen / Gemma / Kimi) so anomaly findings flow directly into
  per-family adaptation work.
- **2026-05-11 -- paid-API sequencing**: Claude API
  (`claude-opus-4-7`) is Phase 4. We clear out every taxonomy
  refinement, capture-format fix, analyzer feature, and
  recoverable-failure-mode hardening on free local backends
  first. By the time we point at the paid API the tooling is
  frozen and we're spending tokens on Opus 4.7's actual
  behavior, not on us re-discovering things qwen3-27b already
  showed us. K=3 shake-out, then K=20, with explicit token
  budget caps to bound damage from any runaway loop.

## Open questions

(empty -- the questions above moved to the Decision log. Add new
ones here as they come up.)
