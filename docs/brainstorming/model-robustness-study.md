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

### Transport / context anomalies (added after Phase 0)

| kind                          | trigger                                                                          | recovery today                                       |
| ----------------------------- | -------------------------------------------------------------------------------- | ---------------------------------------------------- |
| `llm-truncated-at-max-tokens` | upstream stops the stream at `finish_reason=length` mid-turn                     | `LlmError`; retry path may absorb (see notes below)  |
| `edit-file-stale-old-string`  | `edit_file`'s `old_string` doesn't match disk content (recent rewrite, drift)    | tool returns error; counts toward tool-error streak  |

Notes on the new entries:

- `llm-truncated-at-max-tokens`: real fix is to raise
  `SIM_FLOW_MAX_TOKENS`, prompt the agent to write fewer files per
  turn, or shrink the per-turn message stack. The orchestrator
  refuses to commit a partial response because the agent's tool
  calls / writes would be incomplete.
- `edit-file-stale-old-string`: the agent's mental model of the
  file body has drifted from disk. Stacking 5 in a row trips
  `consecutive_tool_error_turns` and aborts the session via
  `RunawayGuard`.

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
   `summary.json` (sketch):

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

## Trial isolation (per-trial freshness)

Each trial in `scripts/run-robustness-study.sh` runs in a unique
`mktemp -d` directory and bootstraps a fresh project via
`sim-flow new model`. There is NO shared per-model state across
trials -- not `state.toml`, not `docs/`, not `src/`, not
`target/`, not `.sim-flow/checkpoint.json`. A trial reaching DM2c
cannot bias the next trial; they don't see each other's disk.

Trial-to-trial variance in advance depth on the same fixture is
therefore purely random sampling unless the seed is fixed.

**Seed control**. As of the post-Phase-0 patch, the openai-compat
agent reads `SIM_FLOW_SEED` and forwards it as `seed` in the
chat-completions body. The driver script sets `SIM_FLOW_SEED=$TRIAL`
per trial, so a captured anomaly can be re-rolled with the same
seed for debugging. vLLM, llama.cpp, and sglang honor the field;
backends that don't ignore it silently.

Phase 0 captures predate this and were fully stochastic -- the
"each of the 3 trials got farther" pattern (DM1 -> DM2b -> DM2c)
is a coincidence of three random rolls, not a state leak. K=20
on seeded trials is what we draw rates from.

## Transport vs. content (vLLM <-> Qwen-family separation)

vLLM is the wire-format compatibility layer; the model's CONTENT
is whatever Qwen3.6 was trained to produce. Two separable concerns:

- **Wire format**: vLLM's `/v1/chat/completions` is OpenAI-shaped.
  Our `OpenAiCompatibleRequest` produces an OpenAI-shaped body
  (`messages[]`, `max_tokens`, optional `seed`, optional
  `chat_template_kwargs`). vLLM passes the body through to the
  model's Jinja chat template.
- **Content shape**: Qwen3.6 produces Qwen-specific output:
  `<think>...</think>` preambles, bare-JSON critiques (no fence,
  or ```json fence), occasional tool-call-as-JSON-blob style.
  These are NOT bugs in the OpenAI-compat layer -- they're Qwen
  speaking Qwen.

Our `agent/adaptation.rs` is the seam between the two. Per-family
profiles (`QWEN3_6_MODEL_FAMILY`, `GEMMA4_MODEL_FAMILY`,
`KIMI_VL_THINKING_MODEL_FAMILY`, `CLAUDE_MESSAGES_MODEL_FAMILY`)
encode what each family does and how the orchestrator should
adapt:

- `thought_marker_style` controls how `normalize_response_text`
  strips `<think>` / `<thinking>` blocks from surfaced content.
- `prefers_bare_json_critique` (post-Phase-0) downgrades the
  salvage diagnostic from Warning to Info when the family
  routinely skips the artifact-write fence.
- `supports_thinking_controls` + `thinking_control_mode` describe
  whether the model has a runtime thinking toggle. The
  openai-compat agent now consults this and, when
  `SIM_FLOW_DISABLE_THINKING=1`, adds
  `chat_template_kwargs.enable_thinking=false` so the chat
  template skips the thinking section entirely.

If a Qwen-specific shape ever DOES leak through the orchestrator
to the gate or critique parser, it's a missing adaptation -- not
a vLLM bug. Add a hypothesis to the taxonomy + a per-family flag
under `ModelFamilyProfile`.

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

## Phase 0 findings (qwen3.6 via vLLM, K=3, dm_flow_smoke_spec.md)

First real captures landed at `/tmp/robustness-phase0/qwen3.6/`.
This is hand-eyeballed, not analyzer output -- the goal here is
to refine the taxonomy before the analyzer code ships (see
"Suggested first cut" point 4).

### Outcomes

| trial | wall (s) | last advance       | terminator                                                |
| ----- | -------- | ------------------ | --------------------------------------------------------- |
| 1     | 502      | DM0 -> DM1         | DM1 critique-iter-cap (2 findings after 3 retries)        |
| 2     | 1534     | DM2a -> DM2b       | DM2b critique-iter-cap (2 findings after 3 retries)       |
| 3     | 1528     | DM2b -> DM2c       | DM2c critique-iter-cap (3 findings after 3 retries)       |

0/3 reached DM4b. The variance is the headline: trial 1 stalled
on DM1, trial 3 reached DM2c. Same model, same spec, different
seeds -- exactly the "single roll lies, K trials reveal the
range" reason K=20 is the production target.

### Anomalies seen (counts across the 3 trials)

| kind                            | t1 | t2 | t3 | notes                                                    |
| ------------------------------- | -- | -- | -- | -------------------------------------------------------- |
| `bare-json-no-fence`            |  2 |  1 |  3 | every critique session salvaged; 100% rate               |
| `critique-iter-cap`             |  1 |  1 |  1 | the terminator in all three trials                       |
| `llm-truncated-at-max-tokens`   |  0 |  1 |  1 | shows up on long-turn steps (DM2a / DM2b)                |
| `tool-error-streak` (5-in-a-row)|  0 |  1 |  0 | trial 2 burned 5 consecutive failed turns                |
| `edit-file-stale-old-string`    |  0 | 14 |  2 | t2: 14 failed `edit_file` (mental model drifted)         |
| `write_file:error`              |  0 |  1 |  7 | t3: 7 rejected writes -- worth a follow-up to classify   |
| `identical-response-streak`     |  0 |  0 |  0 | did NOT fire on this fixture                             |
| `auto-iter-cap` (work-side)     |  0 |  0 |  0 | also didn't fire -- critique cap fired first             |
| `empty-response`                |  0 |  0 |  0 | qwen3.6 never returned an empty turn                     |

### Headline interpretations

1. The `bare-json-no-fence` shape is a systemic qwen3.6 behavior:
   100% of critique sessions across all 3 trials emitted the
   critique JSON without the artifact-write fence and were caught
   by `salvage_critique_json`. The salvage works, but every
   critique starts with a `[warning]` diagnostic and a wasted
   parser pass. Hardening target: tighten the critique system
   prompt to lead with the fenced-write convention, OR teach the
   Qwen runtime profile in `agent/adaptation.rs` to expect
   bare-JSON output and skip the warning.
2. `critique-iter-cap` is the dominant terminator. The retries
   are running with `max_critique_iters=3`. The captures'
   per-retry blocker counts on the terminator step were:
   trial 1 DM1 `2 -> 2 -> 2` (flat), trial 2 DM2b `3 -> 2 -> 2`
   (one step of progress then plateau), trial 3 DM2c
   `3 -> 3 -> 3` (flat). Two takeaways:
   - The cap today is flat-retries (`critique_iters >
     max_critique_iters` in `auto.rs::run_auto_loop`), NOT a
     no-progress check. Changing it to "fail after N
     consecutive retries with non-decreasing blocker count"
     would have spared the last wasted retry in trials 1 and 3
     (~25-30% wall savings on those trials) and let trial 2
     run a 4th retry while it was still making progress. The
     `critique_pass` tracing event already computes the per-pass
     `delta`; the no-progress logic would consume that signal.
   - The flat-count trials are pointing at the same finding
     across retries -- worth dumping the per-retry
     `<step>-critique.json` to confirm whether the agent is
     literally re-flagging the same item, and if so whether
     it's a real gate-blocking issue or a model-stuck-on-nit
     pattern. That sample-and-classify task is open.
3. `llm-truncated-at-max-tokens` confirms the doc's hypothesis
   that long-turn steps would trip the upstream cap. The default
   `SIM_FLOW_MAX_TOKENS` is too tight for qwen3.6's tool-heavy
   verbose style on DM2a/DM2b. Either raise the cap (cheap, but
   masks the real "fewer files per turn" problem) or split the
   instruction into smaller turns.
4. The `edit-file-stale-old-string` outlier (trial 2: 14 failures
   out of 24 attempts) is wild variance. Sometimes qwen3.6 stays
   in sync with disk; sometimes its mental model drifts hard.
   This is the kind of anomaly K=20 would let us call out at a
   real rate.

### Anomalies that didn't fire on this fixture

- `identical-response-streak`: even when truncated/retried,
  qwen3.6's responses varied enough.
- `auto-iter-cap`: work-side cap of 3 never tripped; the
  critique-side cap always fired first.
- `empty-response`: no zero-byte turns from qwen3.6.

We may still see these on other fixtures or other models. The
taxonomy keeps them.

### What to do next

Two parallel tracks, both informed by the K=3 catalog above:

1. Build the analyzer (`study_analyze`) so we can stop
   hand-eyeballing. The shapes above are the seed rules; the
   analyzer codifies them and emits per-model summaries.
2. First hardening pass on qwen3.6 specifically:
   - Add a Qwen-runtime-profile knob in `agent/adaptation.rs`
     that recognizes bare-JSON critique output as canonical
     (drop the salvage warning).
   - Raise the per-call max-tokens default for the Qwen family,
     or add a per-step heuristic that shrinks the inlined
     critique body for long-turn steps.
   - Update the DM2-step critique prompts to lead with the
     fenced-write block before any reasoning.
   - Convert the critique-iter cap from flat retries to a
     no-progress cap (fail after N retries with non-decreasing
     blocker count). The `critique_pass` tracing event already
     emits the per-pass delta; the change is purely in
     `auto.rs::run_auto_loop`. Backend-agnostic, so it
     benefits every model in the study.

We should NOT scale to K=20 yet -- the hardening work above
might reshape the anomaly rates, so K=20 against a tightened
orchestrator is worth more than K=20 against today's.

## Phase 0b findings (qwen3.6 via vLLM, K=3, hardening pass ON)

Same fixture, same K, same backend as Phase 0; with the hardening
pass enabled: `SIM_FLOW_DISABLE_THINKING=1`,
`max_critique_iters=10`, `max_critique_no_progress_iters=3`,
`max_tokens` default 65K, per-trial `seed=trial_idx`.

### Outcomes (Phase 0b vs Phase 0)

| trial | Phase 0 wall / depth / terminator       | Phase 0b wall / depth / terminator                |
| ----- | --------------------------------------- | ------------------------------------------------- |
| 01    | 502s / DM1 / critique-iter-cap          | 773s / **DM2cd** / work-side `max_auto_iters`     |
| 02    | 1534s / DM2b / critique-iter-cap        | 353s / **(none)** / work-side `max_auto_iters`    |
| 03    | 1528s / DM2c / critique-iter-cap        | 416s / DM2a / work-side `max_auto_iters`          |

### Headlines

1. **Token efficiency win.** Median wall 416s vs 1528s
   (~73% faster). Disable-thinking removed the `<think>...</think>`
   preamble from every turn; the cost-per-turn dropped 3-4x.
2. **New failure mode: work-side stall.** Phase 0 was 3/3
   critique-iter-cap (model couldn't satisfy gate within the
   retry budget). Phase 0b is 3/3 **work-side
   `max_auto_iters`** -- the work session burned its turn budget
   without committing a single artifact write. Trial 2 is the
   clearest example: DM0 work pass 1 wrote spec.md OK; DM0
   critique flagged 1 blocker; DM0 work pass 2 (retry) read
   files for 3 turns without re-writing. Disable-thinking
   appears to remove the planning room the model needs on
   retry passes -- with no place to think, it reads + considers
   in the surface text but doesn't commit a write.
3. **Two-cap critique policy worked as intended.** Trial 1's
   DM2cd critique trajectory was `2 -> 1 -> 1 -> 1` (one
   strict-progress pass, then plateau). The Info diagnostic
   surfaced the streak (`(no progress, streak 1/3)`,
   `streak 2/3`) so an operator can see the plateau forming
   before the cap trips.
4. **Reproducibility plumbed.** With `SIM_FLOW_SEED=$TRIAL`, a
   future re-run with the same seed should produce the same
   trajectory. Phase 0 trials were unreproducible (no seed
   plumbed); Phase 0b is the first reproducible run.

### Next hardening: bump work-side cap 3 -> 6

The work-side `max_auto_iters` default of 3 was set before
`disable_thinking` existed. Post-hardening, the cap fires
before the model settles into a write on retry passes. Bumping
to 6 doubles the forcing-prompt budget (the orchestrator
pushes "Produce the artifact file(s) now..." after each empty
turn) without letting a truly-stuck run waste forever. Same
pattern as the critique cap (3 -> 10 absolute + 3 no-progress).

Phase 0c is "0b settings + work-side cap = 6". Reported in
the next decision-log entry.

### Cross-model spot-check (LM Studio / qwen3.6-35b-a3b, trial 1 only)

A parallel LM Studio K=3 (`qwen/qwen3.6-35b-a3b`) was started
with the same hardening but killed after trial 1 to land the
work-side cap bump first. Trial 1 result is consistent with
the vLLM cross-confirmation:

- Walked: DM0 -> DM2a (same as vLLM trial 3)
- Terminator: DM2a work-side `max_auto_iters` -- **same
  pattern across both backends and both model sizes in the
  family**.
- Wall: 3388s (~4x slower than vLLM/27b). 35B model + LM
  Studio overhead.
- 3 `llm-truncated-at-max-tokens` events during the run
  (vs 0 on vLLM Phase 0b) -- the 35B variant is chattier.

The work-side stall isn't quirky to qwen3.6-27b or vLLM. It's
a qwen3.6-family + disable-thinking interaction. The cap bump
applies to both backends.

## Phase 0c findings (K=12 vLLM/qwen3.6 + K=1 Anthropic/opus-4-7)

K=12 captured by running four parallel `e2e_auto` jobs against
vLLM with seeds 1/2/3 per job (12 trials total). Settings
match the post-hardening defaults: `disable-thinking=on`,
`max_auto_iters=6`, `max_critique_iters=10`,
`max_critique_no_progress_iters=3`.

### Advance-depth histogram (K=12)

| step  | trials | %   |
| ----- | ------ | --- |
| DM2d  | 3      | 25% |
| DM2cd | 3      | 25% |
| DM1   | 3      | 25% |
| DM2c  | 2      | 17% |
| DM2b  | 1      |  8% |

Median advance: DM2cd (placeholder-mode milestone walk). No
trial reached DM3+; DM2d is the deepest, and the trials that
got there terminated on either the runaway-loop guard or
cargo-test no-progress (model couldn't drive `cargo test` to
clean within the iter budget).

### Terminator histogram (K=12)

| kind                 | count | notes                                                                |
| -------------------- | ----- | -------------------------------------------------------------------- |
| work-no-artifact     | 8     | model burns `max_auto_iters=6` reading + considering without writing |
| critique-no-progress | 3     | NEW cap (blocker count flat across retries -- fires correctly)       |
| runaway-loop         | 1     | 3 structurally-identical responses                                   |

Critique-iter-cap (absolute) and cargo-test-no-progress never
fired in K=12. The work-side stall remains the dominant failure
mode (67%) even with the cap=6 bump. **The new
`max_critique_no_progress_iters` cap caught 25% of trials**
cleanly -- exactly what we built it for. Without it those 3
trials would have burned 7 more retries each before tripping
the absolute critique cap.

### Per-job × seed grid (cross-job variance at fixed seed)

| job  | seed=1 | seed=2 | seed=3 |
| ---- | ------ | ------ | ------ |
| job1 | DM2d   | DM1    | DM2cd  |
| job2 | DM1    | DM2c   | DM2cd  |
| job3 | DM2c   | DM2d   | DM2cd  |
| job4 | DM2b   | DM2d   | DM1    |

Same-seed-different-job variance is enormous for seeds 1 and 2
(spans DM1 -> DM2d). **Seed=3 matched at DM2cd on 3/4 jobs** --
the only case where vLLM produced reproducible output under
concurrent load. **vLLM's batched scheduler is nondeterministic
across concurrent sessions**, so seed-fixing alone is not
sufficient for trial reproducibility when the GPU is shared.
Pin this in the doc; future studies running multiple parallel
jobs against the same vLLM instance should not expect
seed-determinism.

### Phase 0 -> 0b -> 0c progression

| phase | hardening                       | K  | median wall | median depth | dominant terminator   |
| ----- | ------------------------------- | -- | ----------- | ------------ | --------------------- |
| 0     | none                            | 3  | 1528s       | DM2b         | critique-iter-cap     |
| 0b    | disable-thinking, cap=3         | 3  | 416s        | DM2a         | work-no-artifact      |
| 0c    | + cap=6                         | 12 | ~915s       | DM2cd        | work-no-artifact      |

- Phase 0b cut median wall ~3.7x but capped early on the work
  side.
- Phase 0c restored advance depth (median DM2cd vs Phase 0b's
  DM2a) at the cost of doubling median wall.
- Neither reached DM3+ on the smoke fixture. The work-side
  stall is still the gating issue. Next investigation:
  inspect what the model is actually doing in the wasted
  work turns -- if it's reading + reading + reading without
  committing, prompt-side intervention (forcing-prompt
  wording, milestone-walk scoping) may help more than another
  cap bump.

### Anthropic / claude-opus-4-7 K=1 (685s, 1 trial)

First direct-API run via the new `AnthropicAgent`:

- Last advance: **DM1**
- Terminator: **critique-no-progress** (4 blockers reported on
  every retry; streak hit 3/3 and tripped at retry 5/10)
- 5 `stop_reason=max_tokens` truncations at the prior
  8192-token default (since bumped to 32K in commit b7b4e78)
- **0 salvage warnings**: Claude correctly emits fenced
  critique blocks, confirming `prefers_bare_json_critique=false`
  for the `claude_messages` family is the right setting
- 11 `read_file:error` events: Opus probed for files that
  didn't exist yet (e.g. DM2-stage analysis docs on a
  DM0/DM1-stage flow). Not in the taxonomy; new candidate.

DM1's gate flagged 4 findings persistently. Hypothesis: Opus is
more thorough about the smoke fixture's `targets.md` /
`testbench.md` requirements than the local Qwen models, and
refuses to drop a real finding under prompt pressure. The new
no-progress cap kicks in to keep wall time bounded; in
practice the right move is to either lower the gate
expectations on the smoke fixture or accept that Opus needs
human intervention here.

K=20 against `claude-opus-4-7` should probably wait until:

1. The Anthropic-side truncation cap retest confirms 32K is
   enough.
2. We decide whether to also disable Opus's extended-thinking
   via the API (currently it's not threaded -- the openai-compat
   `disable_thinking` chat_template_kwargs doesn't apply to
   Anthropic; the equivalent would be `thinking: {type:
   "disabled"}` in the Messages body). Phase 0c's qwen3.6 data
   strongly suggests disable-thinking improves token efficiency,
   so the analog for Opus is worth testing.

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
- **2026-05-11 -- K=3 results (qwen3.6, vLLM)**: 0/3 trials
  reached DM4b. All three hit the critique-iter-cap on different
  steps (DM1 / DM2b / DM2c) -- huge run-to-run variance on the
  same fixture confirms K=3 is shake-out only, not reportable.
  Two anomaly kinds added to the taxonomy
  (`llm-truncated-at-max-tokens`, `edit-file-stale-old-string`);
  three didn't fire on this fixture (`identical-response-streak`,
  work-side `auto-iter-cap`, `empty-response`) -- kept in case
  another model surfaces them. Hold K=20 until the first round
  of hardening lands (bare-JSON-as-canonical for the Qwen runtime
  profile, max-tokens bump for verbose-tool-use families,
  fenced-write-first critique prompts).
- **2026-05-11 -- two-cap critique policy landed**: the
  critique-retry cap is now two caps that flip to manual
  whichever trips first. `max_critique_iters` is the absolute
  ceiling, default 10 (was 3). `max_critique_no_progress_iters`
  is new, default 3, and trips when this many consecutive
  retries fail to strictly decrease the gate-failing-finding
  count. The `critique_pass` tracing event already emitted the
  per-pass delta; the no-progress logic consumes it. Wired
  through `sim-flow auto`, `e2e_auto`, `e2e_manual`,
  `dm_flow_smoke`, the VS Code dashboard settings, and the
  capture meta header. Reapplying this policy to the Phase 0
  captures: trial 1 (`2 -> 2 -> 2`) would have flipped one
  retry earlier; trial 2 (`3 -> 2 -> 2`) would have been
  allowed retry 3 because retry 2 made progress, then stopped
  on the no-progress trip; trial 3 (`3 -> 3 -> 3`) would have
  flipped one retry earlier. The absolute cap (10) covers the
  pathological "agent shaves one blocker per pass forever"
  case while preserving freedom for legitimately-progressing
  runs.
- **2026-05-11 -- seed + thinking-control plumbed**: Two
  follow-on patches addressing operator questions on the
  Phase 0 captures.
  1. `OpenAiCompatibleRequest` now carries an optional `seed`,
     read from `SIM_FLOW_SEED`. The driver script sets it to
     `$TRIAL` so trials are reproducible per-index instead of
     fully stochastic. Phase 0 captures predate this -- the
     trial-to-trial advance variance (DM1 / DM2b / DM2c) was
     random sampling, not state leakage (per-trial freshness
     audit is now documented in the doc).
  2. `OpenAiCompatibleRequest.disable_thinking` (env
     `SIM_FLOW_DISABLE_THINKING=1`) emits
     `chat_template_kwargs: {"enable_thinking": false}` in the
     body. Gated on `supports_thinking_controls` so the kwarg
     is only sent to families with thinking-section templates
     (qwen3_6, gemma4, claude_messages). Hot-confirmed against
     vLLM: a `seed=42` + `enable_thinking=false` request to
     qwen3.6 returns a 4-byte content (`"ok"`) instead of the
     full `<think>...</think>` preamble, saving ~99% of the
     tokens on quick turns.
- **2026-05-11 -- work-side cap 3 -> 6 (Phase 0c prep)**:
  Phase 0b's 3/3 trials terminated on work-side
  `max_auto_iters` (no artifact after 3 turns). LM Studio's
  trial 1 (qwen3.6-35b-a3b) hit the identical pattern,
  confirming it's a family-level disable-thinking interaction
  not specific to vLLM or the 27B variant. Bumped the default
  3 -> 6 in `cli.rs` / `e2e_auto` / `e2e_manual` /
  `dm_flow_smoke` and the VS Code dashboard setting; the
  orchestrator's empty-turn forcing prompt now gets ~6 chances
  to land an artifact instead of 3. Symmetric with the
  critique-cap bump (3 -> 10).
- **2026-05-11 -- first hardening pass landed** (3 changes
  motivated by the Phase 0 catalog):
  1. Added `prefers_bare_json_critique: bool` to
     `ModelFamilyProfile`. `qwen3_6 / gemma4 / kimi_vl_thinking`
     get `true`; `claude_messages / generic_chat` get `false`.
     The orchestrator's salvage path consults the flag and
     downgrades the post-salvage diagnostic from `Warning` to
     `Info` when the family routinely emits bare-JSON. Phase 0
     captures had every Qwen3.6 critique trip a Warning; that
     noise is gone now, and a regression on Claude / generic
     still pings.
  2. Bumped the `openai-compat` default `max_tokens` 32K -> 65K.
     Qwen3.6's max_model_len is 262K and the truncations we
     saw in Phase 0 (1 each on trials 2/3) were on long-turn
     DM2a / DM2b steps where 32K was tight; 65K still leaves
     ~200K for context. Env var `SIM_FLOW_MAX_TOKENS` still
     wins for narrower-context backends.
  3. Updated every DM*-critique prompt to lead the `## Output`
     section with the canonical fenced-write form (info-string
     = path, JSON inline, no `json` language tag). The
     orchestrator's salvage path still catches bare-prose and
     `\`\`\`json`-fenced variants -- this is just the
     documented happy path, not a behavior change.

  K=3 reruns with these in place are the next milestone, then
  K=20 to draw rates from.

## Open questions

(empty -- the questions above moved to the Decision log. Add new
ones here as they come up.)
