# vLLM/qwen3.6 anomaly catalog with implemented fixes

Aggregated across **21 vLLM/qwen3.6 trials** of the model-robustness
study (Phase 0: 3 + Phase 0b: 3 + Phase 0c jobs 1-4: 12 + Phase 0d: 3),
ordered by trials affected. This is the operational counterpart to the
draft taxonomy in `model-robustness-study.md` -- it captures only the
anomalies that actually fired against vLLM and what we shipped in
response.

| anomaly | trials affected | rate | description | implemented fix |
|---|---|---|---|---|
| `wrong-fence-info-string` | 13/21 | 62% | Fenced block opened with a language tag (` ```markdown `, ` ```json `) instead of the relative path required by the artifact-write convention; the orchestrator silently drops the body and the gate stays dirty. | Phase 0d prompt hardening (4ea2f9d): expanded `_conventions/fenced-blocks.md` with a "Language-tag info-strings are SILENTLY DROPPED" section + canonical-form reminder in every DM work prompt's `## Output` block. Detector added to `study_analyze` so we can track residual rate. K=3 rerun showed 92% → 33% trials-affected and 74 → 2 total events. |
| `edit-file-stale-old-string` | 12/21 | 57% | `edit_file` tool's `old_string` doesn't match disk content -- model's mental file body has drifted (often after its own prior rewrite). | No targeted fix yet. Existing `consecutive_tool_error_turns` streak limit + `RunawayGuard` aborts the session once 5+ stack in a row. Open follow-up: prompt the agent to re-read before editing, or return the actual on-disk tail in the error to ground the next attempt. |
| `work-no-artifact` | 12/21 | 57% | Work session burned `max_auto_iters` consecutive turns without writing a single allowlisted artifact. | Multi-step: `max_auto_iters` 3 → 6 (Phase 0c). Phase 0d fence fix attacked the root cause (most stalls were dropped `wrong-fence-info-string` writes), dropping the rate 67% → 33%. |
| `write-file-error` | 11/21 | 52% | `write_file` tool returned a non-OK ToolInvoked status -- path outside allowlist, encoding issue, etc. | No targeted fix yet. Existing rejection + diagnostic feedback to agent. Open follow-up: classify the sub-reasons (allowlist vs. encoding vs. permission) in the analyzer so we know which one to attack first. |
| `bare-json-no-fence` (incl. `-expected`) | 5/21 | 24% | Critique session emitted the JSON inline as prose without the canonical ` ```docs/critiques/<step>-critique.json` fence; salvage path's balanced-brace extractor recovered it. | First hardening pass (8017519): added `prefers_bare_json_critique: bool` to `ModelFamilyProfile`. `qwen3_6 / gemma4 / kimi_vl_thinking = true` downgrades the post-salvage diagnostic from `Warning` to `Info`. Qwen3.6's 100% salvage rate is now silent; a regression on Claude / generic still pings. |
| `critique-no-progress` | 4/21 | 19% | Critique retry loop saw blocker count stay flat across `max_critique_no_progress_iters` retries -- model replying but not converging. | Two-cap split (7d0de07): added `max_critique_no_progress_iters = 3` as a separate cap alongside the absolute `max_critique_iters = 10`. Cap fired correctly on 25% of K=12 trials, terminating early instead of burning the full 10 retries. |
| `critique-iter-cap` | 3/21 | 14% | Critique session hit the absolute retry cap while still flagging gate-failing findings. | Two-cap split (7d0de07) bumped absolute cap 3 → 10; vanished entirely from Phase 0c/0d data. The no-progress cap now fires earlier in plateau cases. |
| `llm-truncated-at-max-tokens` | 2/21 | 10% | Backend returned `finish_reason=length` mid-turn; orchestrator refuses to commit the partial response. | First hardening pass (8017519): bumped openai-compat default `max_tokens` 32K → 65K (qwen3.6 has 262K max_model_len). Anthropic separately bumped 8K → 32K (b7b4e78). `SIM_FLOW_MAX_TOKENS` env override available for narrower-context backends. Cleared from Phase 0b onward. |
| `cargo-test-no-progress` | 1/21 | 5% | Repeated `cargo test` with non-decreasing failure count. | No targeted fix needed. Existing no-progress cap catches it cleanly; only fired on one trial that reached DM2d. Rare-by-construction since most trials don't get to a cargo-gated step. |
| `runaway-loop` | 1/21 | 5% | Three structurally-identical responses in a row tripped the runaway guard. | No targeted fix needed. Existing `RunawayGuard` ends the session correctly. K=12 fixed-seed variance under concurrent vLLM load (documented separately) makes this hard to reproduce on demand. |
| `work-gate-still-dirty` | 1/21 | 5% | Model wrote artifacts and advanced, but a milestone-walk gate still flagged unresolved `- [ ]` rows. New terminator surfaced in Phase 0d. | Analyzer-side only so far (5d57415): added `WorkGateStillDirty` variant to `TerminatorKind` so it doesn't get conflated with `work-no-artifact`. Cure (broaden `tick_resolved_milestone_tasks` auto-flip, or prompt the milestone-walk steps to tick rows whose body cites a written file) is the next prompt/orchestrator follow-up. |

## Preventative fix landed alongside the study

`thinking-tags-in-content` did **not** fire in any of the 21 vLLM
trials, but the qwen-code upstream study (2026-05-11) found a leak
path our strip pass didn't cover: qwen3.6 sometimes emits `<Think>`
or `<thinking>` (capitalized / long form) instead of the lowercase
`<think>` we matched literally.

Commit 0e1bc9e extended `strip_known_reasoning_markers` to accept
`<thinking>...</thinking>` as an alias for `<think>...</think>` on
QwenThinkTag-family models, with ASCII-case-insensitive matching
(byte-length stable so multi-byte chars like Kimi's `◁` stay
aligned). Mirrors qwen-code's `TaggedThinkingParser`.

## Anomalies in the draft taxonomy that did NOT fire on vLLM

`empty-response`, `identical-response-streak`, `auto-iter-cap`
(general work-side, distinct from `work-no-artifact`),
`tool-call-loop`, `markdown-critique-no-fence`,
`write-outside-allowlist`, `tool-followup-after-write`,
`milestone-rows-flipped-early`, `milestone-deferred-as-x`,
`critique-clean-with-blockers`, `wrong-step-critique`,
`lib-prefix-misuse`, `preamble-burning-budget`,
`thinking-tags-in-content`, `tool-call-as-json-blob`,
`system-prompt-echoed`.
