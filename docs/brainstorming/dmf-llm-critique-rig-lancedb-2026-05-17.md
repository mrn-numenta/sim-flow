# DMF Critique Against Modern LLMs, and Whether Rig + LanceDB Is a Better Foundation

Date: 2026-05-17
Scope: [02-direct-modeling-flow.md](../flow/02-direct-modeling-flow.md), with cross-reference to [08-orchestrator-tools.md](../flow/08-orchestrator-tools.md), [09-multi-model-adaptation.md](../flow/09-multi-model-adaptation.md), and [04-experiment-tracking.md](../flow/04-experiment-tracking.md).

## Purpose

Two questions, answered together:

1. How does the Direct Modeling Flow (DMF) hold up against how modern LLMs (Claude Opus 4.7, Qwen 3.6) actually behave, and where are the pitfalls?
2. Would Rig + LanceDB be a better foundation than the current sim-flow architecture, and could DMF / DSF / SVF be implemented on top of them instead?

## What the DMF actually is

A 9-step pipeline (DM0 → DM5) where each step pairs a **work session** with an independent **critique session**, gated by markdown-file existence checks plus a grep for `UNRESOLVED:` / `BLOCKER:` lines in the critique. Artifacts are exclusively markdown (decomposition, pipeline mapping, plan, etc.) and Rust source. Independence of critique is enforced either by subprocess isolation (`per-step`) or by prompt structure (`single`). The orchestrator advertises a fixed tool catalog ([08-orchestrator-tools.md:32-39](../flow/08-orchestrator-tools.md#L32-L39)).

## How modern LLMs actually behave (the parts that matter for DMF)

1. **Attention degrades in the middle of long context.** Opus and Qwen 3.6 will accept 200k–1M tokens; they won't reliably *use* anything past the first ~50k. By DM2d the work session is being asked to integrate spec + targets + testbench + decomposition + data-movement + pipeline-mapping + plan + selected `fw:api` pages. That's well past the reliable-use band.
2. **Thinking is now a structured channel.** Opus 4.7 returns typed `thinking` blocks; Qwen 3.6 emits `<think>...</think>`. The current design treats thinking as something to *strip* ([09-multi-model-adaptation.md:347-352](../flow/09-multi-model-adaptation.md#L347-L352)) — it is never used as a *gate input*, even though it's the strongest signal of whether the model is bluffing.
3. **Native tool use is the cheap path.** Both providers have first-class tool APIs. The fenced-block fallback ([08-orchestrator-tools.md:172-184](../flow/08-orchestrator-tools.md#L172-L184)) is a competence cliff — text-only tool emission is where hallucination injects.
4. **Self-bias on self-critique is real and not fixed by a new subprocess.** A fresh Claude session reading its own output under a different prompt still agrees with itself the large majority of the time. Process isolation is not independence.
5. **Numerical constraints get treated as decoration.** "Gate-budget-per-cycle is a hard input" assumes a calculator-grade constraint; models routinely round-trip the number through prose and lose it.
6. **API surface hallucination is the dominant DM2d failure mode.** Confirmed locally: rgb_toy DM2d invented `take_input(...) -> Option<T>` because the model had a plausible prior, and the flow had no retrieval check.

## Specific pitfalls in DMF

Each tied to a concrete failure mode rather than a theoretical concern.

### P1. Critique as theatre

The gate checks ([02-direct-modeling-flow.md:271-281](../flow/02-direct-modeling-flow.md#L271-L281)) are: (a) the file exists, (b) it contains the right regex, (c) it lacks `BLOCKER:` lines that the same model class chose whether to emit. The gate is a self-signed certificate. The rgb_toy DM2d critiques correctly named the architectural defect three times; nothing in the gate noticed that the agent didn't act on it.

### P2. "Single" session policy provides no review independence

A long-lived agent re-prompted to critique its own work has the same KV-cache attention bias as before. The doc admits this is "enforced by prompt structure" ([02-direct-modeling-flow.md:30](../flow/02-direct-modeling-flow.md#L30)) but that is wishful — there is no measurable enforcement.

### P3. No retrieval over `fw:api`

DM2d says "start with `fw:api/toc.md`, then read only the pages you need" ([02-direct-modeling-flow.md:558-561](../flow/02-direct-modeling-flow.md#L558-L561)). That is manual navigation by intuition. In rgb_toy, the agent guessed wrong about `take_input` and never read the source. There is no semantic-search step that forces "what function returns `Option<T>` for an input port?" to actually hit `fw:src`.

### P4. Artifacts are markdown, so the orchestrator can only grep, not reason

Decomposition operations *should* be a typed list the orchestrator can intersect against pipeline-mapping stages — instead the gate check is "operation names appear in the other file" ([02-direct-modeling-flow.md:472-473](../flow/02-direct-modeling-flow.md#L472-L473)), which fails on the first rename.

### P5. No cross-session memory

Each step prompt is static. Lessons from one milestone never get injected into the next milestone's prompt. The rgb_toy run made the same architectural mistake across ~30 sessions because there was no "previously rejected approach" pile.

### P6. Gate progression doesn't penalise stagnation

`no-progress` caps fire but on total-test-count, not behaviour bisection. Five caps fired without changing the agent's plan because each cap was a "try again with more thinking" event.

### P7. Prompt cache thrash

A `per-step` subprocess discards the prompt cache between work and critique. Each ~30k-token re-read of spec.md + analysis docs costs full input pricing. Over a DM2d run with 10 milestones × work+critique, this is real money.

### P8. Multi-model adaptation handles syntax, not competence

The four-layer profile ([09-multi-model-adaptation.md:205-211](../flow/09-multi-model-adaptation.md#L205-L211)) makes Qwen and Claude both emit clean tool calls — but it doesn't notice Qwen 3.6 8B can't actually do the architectural decomposition Claude Opus can. There is no per-step model-tier policy.

## Improvements (ranked by impact)

1. **RAG over `fw:api` + `fw:src` as a mandatory pre-step for DM2d / DM3b / DM3c.** Before the work session asks for any framework symbol, the orchestrator pre-loads relevant chunks based on the active decomposition / pipeline-mapping. Direct fix for the DM2d stuck pattern. LanceDB is the right tool here.
2. **Structured artifacts, not just markdown.** `decomposition.toml` / `pipeline-mapping.toml` with markdown views generated *from* the structured form. The orchestrator can then do real consistency checks: every operation has a stage, every stage's gate-budget is ≤ target, every payload has a producer-consumer pair. Kills P4 and most of P1.
3. **External adversarial check.** The critique session should not be the only review. Add a deterministic step: a test harness that builds a minimal-repro from the artifacts (e.g., elaborate the model with one input vector) and only then runs the LLM critique. The harness output is what the gate scans, not free-form prose.
4. **Use structured `thinking` as gate input.** When Claude / Qwen emit thinking, the normaliser ([09-multi-model-adaptation.md:347-352](../flow/09-multi-model-adaptation.md#L347-L352)) currently throws it away. Keep it, fingerprint it, and require the critique session to read the *work session's* thinking — much closer to real independent review than "fresh subprocess on same prompt."
5. **Per-step model tier.** `[steps.DM2d] model = "claude-opus-4-7"` is already a hook ([02-direct-modeling-flow.md:144](../flow/02-direct-modeling-flow.md#L144)). Use it: DM0 (parsing the spec) is fine on a small model; DM2d should default to Opus with extended thinking.
6. **A "previously rejected" pile per step.** Each `BLOCKER:` resolution writes a structured entry; the next work session in the same step gets the rejection list prepended. Trivial implementation, big effect on P5.
7. **Bisection-driven `no-progress`.** Caps should fire on "tests failing in the same way 3 sessions in a row" — and the next session's prompt becomes "you have failed identically 3 times; produce a minimal repro before any edit."
8. **Prompt cache discipline.** For `per-step`, share a stable prefix (project-context block) across work / critique invocations so the provider's cache survives the subprocess boundary. Claude's prompt-cache TTL plus a stable prefix gives ~5 min cross-process hits.

## Rig + LanceDB analysis

### Short answer

No, Rig + LanceDB are not a better foundation. They would replace ~20% of sim-flow's surface area and leave the 80% that is actually hard untouched. DMF / DSF / SVF *could* be implemented on top of them, but you would be rebuilding the same orchestration code on a thinner base.

### What Rig + LanceDB give you

- HTTP provider abstraction (Anthropic, OpenAI-compat, etc.) — overlaps with [client.rs / clients/claude.rs / clients/codex.rs / clients/copilot.rs](../flow/02-direct-modeling-flow.md#L1019-L1033).
- A tool-use loop (advertise tools, dispatch `tool_calls`, thread results back). Overlaps with the native-tool-use branch of [08-orchestrator-tools.md:156-171](../flow/08-orchestrator-tools.md#L156-L171).
- RAG primitives + LanceDB integration. New capability for sim-flow.
- Structured-output extractors. Nice-to-have for parsing critique files.
- A pipeline DSL. Cosmetically similar to step composition, but not workflow-aware.

That is the catalog. Useful, but it's *the* easy layer.

### What they don't give you (the hard parts of DMF / DSF / SVF)

1. **State machine + gate semantics.** `.sim-flow/state.toml`, step ordering, back-transitions resetting downstream gates, critique-file grep for `BLOCKER:` ([02-direct-modeling-flow.md:151-173](../flow/02-direct-modeling-flow.md#L151-L173)). Rig has no concept of this.
2. **Multi-phase iteration loops.** Author → build (cargo check) → test (cargo test) → coverage → done, with iteration counters and external validators between LLM turns ([08-orchestrator-tools.md:111-146](../flow/08-orchestrator-tools.md#L111-L146)). Rig's agent loop is "LLM ↔ tools," not "LLM ↔ tools ↔ external validators ↔ LLM with errors injected."
3. **Session protocol + host renderer.** The VS Code extension is a thin renderer over a typed event stream (`PhaseChanged`, `ToolInvoked`, `BuildOutput`, `RequestUserInput`, `GateResult`). Rig has nothing for cross-host streaming; you would build it on top.
4. **Per-step write scoping and path sandboxing.** Universal tool catalog with per-step `work_write_paths` enforced at the dispatcher and the artifact-write extractor ([08-orchestrator-tools.md:62-104](../flow/08-orchestrator-tools.md#L62-L104)). Rig dispatches tools but doesn't enforce workflow-scoped policy.
5. **Subprocess CLI agents.** Rig assumes you own the LLM HTTP call. It cannot drive Claude Code, the codex CLI, or copilot CLI — those are entire agents with their own tool stacks, file editing, prompt caching, and TTY UX. Today sim-flow can ride on a user's existing Claude Code subscription and let them type into the agent's TTY in manual mode. Moving to Rig means becoming your own agent — re-implementing diff-based file editing, multi-line patch application, retry / timeout semantics, prompt caching, TTY interactivity. Months of work currently obtained for free.
6. **Experiment tracking + run identity.** `.sim-flow/experiments.db`, run-id generation, manifest correlation ([04-experiment-tracking.md:78-107](../flow/04-experiment-tracking.md#L78-L107)). Orthogonal to Rig.
7. **Multi-model adaptation profile model.** The four-layer transport / runtime / model-family / normalizer design ([09-multi-model-adaptation.md:205-211](../flow/09-multi-model-adaptation.md#L205-L211)) is *richer* than Rig's provider abstraction, because it also wraps CLI agents, vscode.lm, and processor-local paths.

### Could DMF / DSF / SVF be implemented in Rig + LanceDB?

Yes, in the same sense that they could be implemented in any HTTP client + Rust. You would write:

- Your own `StateMachine` for step transitions
- Your own gate validator
- Your own session protocol over the wire to VS Code
- Your own per-step tool scoping
- Your own multi-phase iteration loop with external validators (cargo) in the middle
- Your own subprocess-CLI client(s) for Claude Code / codex / copilot

That is just sim-flow, sitting on Rig instead of on `reqwest`. The orchestration code doesn't get smaller; it gets the same, with a different HTTP layer underneath.

Note: SVF is not in [docs/flow/](../flow/) as of writing — assumed to be a third workflow with the same shape as DMF / DSF (specification → analysis → gated step sequence → artifacts → critiques). If so, the same argument holds: the workflow machinery is the value; the LLM transport is interchangeable.

### What Rig is actually well-suited for

- A new HTTP backend client alongside the existing CLI clients. Treat Rig as one transport in the multi-model layer ([09-multi-model-adaptation.md:247-275](../flow/09-multi-model-adaptation.md#L247-L275)), not as the orchestrator.
- The RAG layer (LanceDB indexing of `fw:api` / `fw:src`, `api_search` tool). This is the high-leverage piece. Rig's `vector_store::lancedb` + `embeddings` modules can be used without buying into `Agent`.
- Structured-output extraction when parsing critique files or LLM-emitted structured artifacts.

### Recommended split

| Concern                                            | Owner               |
| -------------------------------------------------- | ------------------- |
| State machine, gates, step ordering                | sim-flow (keep)     |
| Session protocol, host events, phases              | sim-flow (keep)     |
| Per-step write scoping, tool dispatch              | sim-flow (keep)     |
| Subprocess CLI clients (claude / codex / copilot)  | sim-flow (keep)     |
| HTTP API clients (Anthropic Messages, OpenAI-compat) | sim-flow, optionally implemented *via* Rig provider crates |
| Embeddings + vector index + retrieval              | Rig + LanceDB       |
| `api_search` tool wiring                           | sim-flow uses LanceDB directly |

This gets the LanceDB win without coupling the workflow engine to a framework that doesn't actually solve the workflow problem.

## Bottom line

The DMF is structurally sound — gate-driven, step-decomposed, hosts-as-renderers — but it currently treats LLMs as deterministic compilers and self-critique as oracle. The two highest-impact changes are:

1. **Retrieval-augmented framework access** (LanceDB over `fw:api` / `fw:src`, exposed as an `api_search` tool, mandatory before DM2d edits).
2. **Structured artifacts + deterministic cross-step checks** so the orchestrator stops grepping markdown and starts validating semantic consistency.

Rig + LanceDB are good *components*, not a good *foundation*. The thing that makes sim-flow valuable is the workflow state machine, the gate model, the host protocol, and the ability to ride on top of existing CLI agents. None of that is what Rig provides. Adopting Rig wholesale means losing the CLI-agent path and rebuilding the orchestration on a thinner base. Adopting LanceDB on its own — and optionally using Rig's RAG modules to do so — is the high-leverage move and doesn't force the rewrite.
