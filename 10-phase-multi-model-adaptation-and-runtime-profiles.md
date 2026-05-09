# Phase 10 - Multi-Model Adaptation And Runtime Profiles

Phase dependency: Phase 9 (orchestrator-driven sessions and multi-host
support). Design reference:
[09-multi-model-adaptation.md](../../../tools/sim-flow/docs/flow/09-multi-model-adaptation.md),
[06-vscode-extension.md](../../architecture/ai-flow/06-vscode-extension.md),
[07-session-protocol.md](../../architecture/ai-flow/07-session-protocol.md),
[08-orchestrator-tools.md](../../architecture/ai-flow/08-orchestrator-tools.md).

## Problem Statement

Phase 9 established `sim-flow` as the single orchestrator for CLI and IDE
hosts, but the current LLM integration surface still treats too much behavior as
backend-specific or provider-specific glue. That is workable for a small set of
models, but it becomes brittle when we need to support multiple model families
and multiple runtimes with materially different semantics:

1. **Model families differ in prompt and output semantics.** Gemma 4, Qwen
   3.6, Kimi-VL, and Claude do not agree on how thinking, tool use, or
   multimodal input should be represented.
2. **Serving stacks differ independently from the model family.** vLLM, LM
   Studio, Ollama, Claude's Messages API, and processor-centric local inference
   all impose different request/response constraints even when the higher-level
   workflow is the same.
3. **Response normalization is not a first-class boundary yet.** Some code
   paths still flatten structured provider output too early, while others rely
   on backend-specific heuristics in places that should be model- or
   runtime-owned.
4. **Cross-context runtime concerns now span multiple hosts.** The shared API
   key work landed a stable env -> shared credentials file -> host secret store
   resolution chain. Similar cross-host behavior is likely needed for other
   runtime capabilities, and the plan should treat that as a runtime concern,
   not an accidental side effect of one frontend.

Without a dedicated phase for adaptation, each new family or runtime will add
more ad hoc conditionals to the transport backends, increasing drift between the
Rust and TypeScript implementations and making tool-use and artifact-writing
bugs harder to reason about.

## Goal

Land a layered adaptation model for `sim-flow` that cleanly separates:

- transport backend behavior
- runtime capability behavior
- model-family behavior
- response normalization

The implementation must support both the TypeScript extension path and the Rust
session-agent path, and it must preserve the orchestrator's existing invariants:

- tool calls are normalized before orchestration consumes them
- reasoning does not leak into artifact writes
- host-specific runtime concerns stay outside model-family policy
- the same project behaves consistently across CLI and IDE surfaces

## Non-Goals

- Rewriting the orchestration protocol or replacing the session host model from
  Phase 9.
- Introducing every possible model family in one pass. The goal is a good seam
  plus a small set of representative profiles.
- Building new deployment tooling for local model serving.
- Turning all backend features into a fully generic plug-in system on day one.
  A small, explicit internal profile model is sufficient.

## Milestone 1 - Shared adaptation interfaces

Define the core internal vocabulary and make it concrete enough that both the TS
and Rust code paths can target the same conceptual boundary.

- [x] Add a small design-oriented module / type set for:
  - `TransportBackend`
  - `RuntimeCapabilityProfile`
  - `ModelFamilyProfile`
  - `ResponseNormalizer`
- [x] Define a normalized output shape for the session-driving layer:
  - `content`
  - `reasoning`
  - `tool_call`
- [x] Document which concerns belong to which layer in code comments near the
  new interfaces so future contributors do not re-collapse the boundaries.
- [x] Add regression-oriented unit tests for the interface adapters that do not
  require a real network backend.

After M1: `sim-flow` has explicit adaptation seams instead of relying on
backend-local conventions.

## Milestone 2 - Runtime capability profiles in the TypeScript path

The VS Code extension already has the richest backend surface and should be the
first place we factor out runtime-specific behavior.

- [x] Introduce runtime capability profiles for at least:
  - `openai_compat_generic`
  - `anthropic_messages`
  - `processor_local` (or equivalent placeholder for processor-centric flows)
- [x] Move current OpenAI-compatible runtime constraints behind this layer:
  - single-leading-system-message handling
  - runtime-specific request-body support
  - reasoning field availability
  - native tool-call field availability
- [x] Move the shared key-resolution chain under the runtime/integration policy
  vocabulary rather than letting it remain implicit backend glue.
- [x] Keep existing user-facing behavior stable while the implementation moves
  behind the new runtime profile interface.

After M2: the TS backends stop owning runtime-policy conditionals directly.

## Milestone 3 - Model-family profiles in the TypeScript path

Add a small number of representative model-family profiles that exercise the new
boundaries.

- [x] Add initial model-family profiles for:
  - `gemma4`
  - `qwen3_6`
  - `kimi_vl_thinking`
  - `claude_messages`
- [x] Model-family profiles own:
  - thinking controls / prompt toggles
  - default sampling preferences
  - multimodal ordering preferences
  - known thought / tool-call markers
  - history policy for prior reasoning
- [x] Ensure these profiles can be selected explicitly and, where safe, inferred
  from configured model ids without making inference mandatory.
- [x] Add profile-level tests for prompt shaping and multimodal content ordering.

After M3: the TS path can differentiate model-family behavior without adding
more provider-conditionals to backend classes.

## Milestone 4 - Response normalization and structured-provider support

Build the response normalizer into a first-class layer and use it to stop
flattening structured output too early.

- [x] Normalize OpenAI-compatible reasoning and tool-call output into the shared
  internal chunk/event shape.
- [x] Normalize Anthropic / Claude typed blocks into the same shape, preserving
  `thinking`, `tool_use`, and `tool_result` semantics rather than collapsing to
  concatenated text.
- [x] Add raw-text normalizers for families whose thinking is exposed as text
  markers rather than typed blocks:
  - Qwen `<think>...</think>`
  - Kimi custom think delimiters
  - any Gemma family marker handling needed by the selected runtime
- [x] Add negative tests that prove reasoning cannot leak into artifact writing
  or tool-call parsing once the normalizer boundary is in place.

After M4: all supported TS-backed runtimes emit one normalized internal stream.

## Milestone 5 - Rust session-agent parity

Mirror the same adaptation model into the Rust session-agent path so the CLI and
extension do not drift.

- [x] Introduce Rust-side runtime capability and model-family profile
  abstractions aligned with the TS design.
- [x] Move current Rust OpenAI-compatible and Anthropic adaptations behind the
  new layers without regressing existing behavior.
- [x] Ensure the Rust path shares the same conceptual normalization contract as
  the TS path for:
  - reasoning
  - tool calls
  - structured vs raw-text outputs
- [x] Add Rust tests for the profile logic and normalization boundaries.

After M5: the Rust and TS paths differ by host/transport details, not by
adaptation architecture.

## Milestone 6 - Configuration, diagnostics, and migration cleanup

Make the new adaptation model operable and debuggable for users and
contributors.

- [ ] Add configuration support for selecting or overriding:
  - runtime capability profile
  - model-family profile
  - profile-specific debug logging when needed
- [ ] Improve diagnostics so failures report the active backend, runtime
  profile, model-family profile, and key runtime capabilities in effect.
- [ ] Update flow docs and architecture notes to describe the landed
  implementation, not just the planned design.
- [ ] Add at least one end-to-end validation scenario per representative family
  or runtime category that can run in mocked or fixture-driven form.

After M6: the adaptation system is documented, configurable, and testable.

## Sequencing Notes

Recommended landing order:

1. TypeScript runtime profiles
2. TypeScript model-family profiles
3. TypeScript response normalization
4. Rust parity
5. configuration / diagnostics / cleanup

This order keeps the first implementation in the host path with the richest LLM
surface, then mirrors the design into Rust once the seam has proven itself.

## Risks

- **Over-generalization too early.** A profile model that tries to encode every
  possible provider behavior will become harder to maintain than the current
  code. Keep the first version intentionally small.
- **TS/Rust drift.** If the adaptation model lands in only one implementation
  path for too long, the other path will grow new special cases and erase the
  benefit of the refactor.
- **Normalization regressions.** Tool-use and artifact-writing flows are
  sensitive to formatting changes; the migration must be backed by focused tests
  before larger refactors proceed.
- **Configuration confusion.** Automatic profile inference should remain
  debuggable and overrideable so users can recover when model naming is
  ambiguous.

## Status

- [x] Multi-model adaptation design note added under
  `tools/sim-flow/docs/flow/09-multi-model-adaptation.md`.
- [x] Shared API key resolution landed across CLI and VS Code as an example of
  cross-context runtime behavior.
- [x] Milestone 1 completed in the TypeScript LLM layer with shared adaptation
  interfaces, normalized chunk vocabulary, and session-consumer adoption of
  `normalizeLlmChunk`.
- [x] Milestone 2 completed in the TypeScript backend path with explicit
  runtime capability profiles for OpenAI-compatible, Anthropic Messages, and
  processor-local placeholder runtimes.
- [x] Milestone 3 completed in the TypeScript backend path with explicit
  model-family profiles, inference/override support, prompt-policy helpers, and
  multimodal ordering policy.
- [x] Milestone 4 completed in the TypeScript backend path with shared response
  normalizers, raw-text thinking-tag parsing, and Anthropic structured block
  preservation.
- [x] Milestone 5 completed in the Rust session-agent path with shared
  adaptation helpers, model-family inference/override support, OpenAI-compatible
  request/response normalization, and Claude CLI runtime alignment.
