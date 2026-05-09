# 9. Multi-Model Adaptation

## Purpose

Describe the compatibility issues `sim-flow` faces when driving multiple
open-weight and hosted models through a common orchestration layer, and define
the abstraction boundaries needed to support those models without scattering
model-specific conditionals throughout the transport and orchestration code.

This document focuses on the session-driving and prompt/response path used by
`sim-flow`. It does not define model training, quantization, or deployment
infrastructure.

## Problem Statement

`sim-flow` currently treats many model integrations as variants of the same
backend:

- OpenAI-compatible HTTP transport
- streaming text chunks
- optional native tool schemas
- prompt text plus attachments

That transport-level similarity hides meaningful behavioral differences between
model families and serving stacks. In practice, these differences affect whether
the model:

- emits usable tool calls
- leaks chain-of-thought text into artifact writes
- expects prompt-template controls rather than request-body controls
- requires server-side parsers to make tool use reliable
- prefers a specific multimodal content ordering
- expects postprocessing of raw decoded output

Treating these differences as ad hoc backend quirks causes the wrong code to
own the wrong policy and makes multi-model support brittle.

## Examples of Divergence

The current research set already shows four distinct categories of divergence.

### Gemma 4

Gemma 4 function calling is template-oriented. The official documentation uses
`processor.apply_chat_template(..., tools=...)` and shows the model emitting
tool-call text that the application must parse and validate before execution.

Implications:

- tool-call syntax is model-family specific, even when the outer transport is
  not
- prompt construction matters as much as HTTP request shape
- output parsing cannot be delegated entirely to the transport backend

### Qwen 3.6

Qwen 3.6 exposes thinking behavior through template and runtime controls such as
`enable_thinking`, and commonly emits explicit `<think>...</think>` blocks.
Some serving stacks additionally recommend server-side reasoning parsers and
tool-call parsers to keep outputs structured.

Implications:

- thinking mode is not just a UI concern; it changes the model output contract
- server/runtime capability matters independently from model-family behavior
- response normalization must be able to strip or classify thought blocks

### Kimi-VL Thinking

Kimi-VL is processor-centric for multimodal use. The official repository uses
`AutoProcessor`, `apply_chat_template`, model-specific input packing, and raw
`batch_decode` of generated tokens. Thinking output is surfaced as normal text
rather than as a structured reasoning field.

Implications:

- multimodal preparation may need model-family handling beyond generic
  OpenAI-compatible content arrays
- raw decoded output may require model-specific postprocessing
- "OpenAI-compatible serving" does not imply "OpenAI-compatible semantics"

### Claude API

Claude's Messages API is structurally richer than the plain text-oriented path
used by many OpenAI-compatible integrations. The API can return typed content
blocks such as `thinking`, `text`, and `tool_use`, and expects typed
`tool_result` blocks to continue the tool loop. Extended thinking is a first-
class API feature with model-specific modes such as manual budgets and adaptive
thinking.

Implications:

- thinking is not merely a text convention; it is part of the API schema
- tool use is natively structured and should not be reduced to text fences when
  the API already provides typed blocks
- context handling during tool loops has Claude-specific rules, especially when
  thinking is enabled
- token counting, prompt caching, and context-management features belong in the
  runtime capability layer rather than in generic orchestration logic

### Serving Stack Differences

Even when two models are both served via an OpenAI-compatible endpoint, the
server may impose additional behavioral constraints:

- strict single-leading-system-message chat templates
- server-specific `extra_body` request fields
- parser flags for reasoning or tool calls
- multimodal limits and model-length constraints

Implications:

- transport and runtime capability must be modeled separately
- server-specific features must not leak into model-family policy by accident

## Current Failure Modes

Without a clearer separation of concerns, `sim-flow` risks the following
classes of bugs:

1. Thought text contaminates the orchestrator stream.
   - Example: `<think>...</think>` or model-specific thought markers reach the
     artifact writer and break path/tool parsing.

   - Claude-specific variant: native `thinking` blocks are flattened into plain
     text instead of being preserved as structured reasoning events.

2. Tool-use reliability depends on the wrong layer.
   - Example: a backend tries to solve a model-family formatting problem that
     should have been handled by the prompt/template policy.

3. Runtime-specific parser requirements are hardcoded as backend behavior.
   - Example: a `vllm`-specific tool parser recommendation gets treated as a
     universal `Qwen` behavior, or vice versa.

4. Multimodal message formatting is globally optimized for one family and harms
   another.

5. Request tuning becomes unmaintainable.
   - Example: temperature, thinking flags, parser fields, and decode limits are
     all attached directly to the transport client instead of to the model and
     runtime contracts that actually own them.

6. New model onboarding requires editing many unrelated files.
   - Adding one model should not require touching orchestration, prompt
     assembly, transport, UI rendering, and response parsing independently.

7. Native structured APIs get downgraded to text-only behavior.
   - Example: a Claude backend that extracts only `text` blocks loses
     `thinking`, `tool_use`, and tool-loop continuity features that are already
     available in the API.

## Design Goals

- Keep the orchestrator model-agnostic where possible.
- Keep backend/transport implementations focused on wire protocol concerns.
- Make model-family behavior explicit and testable.
- Make runtime/server capabilities explicit and composable.
- Normalize outputs into a small internal representation before orchestration
  logic consumes them.
- Support incremental adoption so existing backends continue to work while
  higher-fidelity profiles are added.

## Non-Goals

- Define a universal prompt template language for every model family.
- Replace provider-native tool calling with a custom protocol when native tool
  calling is available and reliable.
- Build model deployment automation in `sim-flow`.
- Standardize or preserve model chain-of-thought internally; `sim-flow` only
  needs enough structure to avoid contaminating artifacts and to render optional
  reasoning safely in the UI.

## Proposed Solution

Introduce four explicit layers:

1. **Transport backend**
2. **Runtime capability profile**
3. **Model-family profile**
4. **Response normalizer**

These layers compose into one per-session dispatch configuration.

### 1. Transport Backend

The transport backend is responsible only for how requests and responses cross
the boundary to the model service.

Responsibilities:

- endpoint URL selection
- auth and headers
- streaming vs non-streaming behavior
- request/response serialization format
- native tool schema transport when the provider supports it
- cancellation and timeout mechanics

Examples:

- `openai-compat`
- `anthropic`
- `vscode.lm`

The transport backend must not own model-family prompt decisions such as:

- whether to enable thinking
- how to order multimodal content for a specific family
- how to strip family-specific thought tags

However, the transport backend may expose provider-native structured fields to
the response normalizer instead of flattening them prematurely.

### 2. Runtime Capability Profile

The runtime capability profile captures behavior imposed by the serving stack or
integration surface rather than by the model weights themselves.

Responsibilities:

- extra request-body fields supported by the runtime
- parser flags or runtime-side tool/reasoning helpers
- system-message constraints
- supported multimodal message shape
- server-side limits that affect request construction

Examples:

- plain OpenAI-compatible server
- vLLM with family-specific parser support
- LM Studio generic OpenAI-compatible surface
- local Transformers/processor path

This layer owns statements like:

- "this runtime accepts `extra_body`"
- "this runtime can expose reasoning as a separate field"
- "this runtime requires a single leading system message"
- "this runtime supports token counting before dispatch"
- "this runtime supports prompt caching or context editing"
- "this runtime returns native structured tool/thinking blocks"

### 3. Model-Family Profile

The model-family profile captures model semantics that are stable across
different runtimes serving the same family.

Responsibilities:

- prompt/template controls for thinking and tool use
- preferred sampling defaults
- multimodal ordering preferences
- known tool-call markers
- known thought markers
- whether prior thinking should be preserved in history
- whether the family is processor-centric for multimodal input

Examples:

- `gemma4`
- `qwen3_6`
- `kimi_vl_thinking`

This layer owns statements like:

- "Qwen thinking may emit `<think>...</think>`"
- "Gemma tool calling is template-driven"
- "Kimi-VL often decodes raw thought text that must be normalized"
- "Claude thinking may be configured as disabled, adaptive, or budgeted"

### 4. Response Normalizer

The response normalizer converts raw model/runtime output into the compact
internal representation that `sim-flow` orchestration already wants:

- `content`
- `reasoning`
- `tool_calls`

Responsibilities:

- strip or classify family-specific thought blocks
- recover tool calls from model-family-specific markers
- preserve normal assistant content
- drop or quarantine malformed partial tool output when policy requires it
- preserve provider-native typed blocks when they already map cleanly to the
  internal event model

The normalizer is the boundary that protects orchestration and artifact writing
from model-specific text conventions.

## Internal Contract

The composed dispatch path should normalize everything into a provider-neutral
shape before the session pump and orchestrator consume it.

Suggested conceptual request pipeline:

```text
orchestrator messages
    -> transport backend selection
    -> runtime capability profile adapts request
    -> model-family profile applies prompt/template policy
    -> backend sends request
    -> backend receives raw chunks
    -> response normalizer emits {content, reasoning, tool_calls}
    -> session/orchestrator consumes normalized output
```

Suggested conceptual output shape:

```text
NormalizedResponseChunk {
    kind: content | reasoning | tool_call
    payload: ...
}
```

The exact type names are implementation details, but the normalized boundary
should be explicit and shared across the TypeScript extension path and the Rust
session-agent path.

## Policy Ownership

The following table defines where major decisions belong.

| Concern | Owner |
| ------- | ----- |
| HTTP headers, SSE parsing, auth | transport backend |
| single-leading-system-message workaround | runtime capability profile |
| `enable_thinking` / family-specific prompt toggles | model-family profile |
| temperature / top-p / family default sampling | model-family profile |
| runtime-specific parser flags / extra request body | runtime capability profile |
| image-before-text vs text-before-image | model-family profile |
| stripping `<think>` or custom thought markers | response normalizer |
| converting native or text tool calls to internal tool events | response normalizer |
| deciding whether prior reasoning stays in history | model-family profile |
| token counting / prompt caching / context-editing affordances | runtime capability profile |
| preserving native `thinking` / `tool_use` / `tool_result` structure | response normalizer |

## Why This Partitioning

This separation prevents three common mistakes:

1. Encoding model behavior as backend behavior.
   - Example: Qwen thinking blocks are not an `openai-compat` feature.

2. Encoding runtime behavior as model behavior.
   - Example: vLLM parser support is not a universal property of every Qwen
     deployment.

3. Letting orchestration depend on raw model text conventions.
   - Example: artifact writing should never depend on whether a model emits
     `<think>`, `◁think▷`, or a structured reasoning field.

4. Flattening a structured API too early.
   - Example: Claude's typed `thinking` and `tool_use` blocks should flow into
     the internal normalized event model instead of being converted to plain
     concatenated text at the transport edge.

## Migration Strategy

Adopt the new structure incrementally.

### Phase 1: Normalize Existing Behavior

- Extract current leading-system-message merge into an explicit runtime-capability
  policy.
- Extract current reasoning-field handling into a shared response-normalization
  layer.
- Keep existing backend classes intact while routing through the new policy
  interfaces.

### Phase 2: Add Model-Family Profiles

- Introduce profiles for the known families:
  - `gemma4`
  - `qwen3_6`
  - `kimi_vl_thinking`
  - `claude_messages`
- Start with prompt controls, thought markers, and sampling defaults.

### Phase 3: Add Runtime Profiles

- Introduce explicit runtime profiles for:
  - generic OpenAI-compatible
  - vLLM-style OpenAI-compatible
  - processor-centric local Transformers path
  - Claude Messages API with extended-thinking/tool-use support

### Phase 4: Unify Rust and TypeScript Semantics

- Ensure the VS Code extension path and the Rust session-agent path use the same
  conceptual profile model.
- Avoid a future state where TS supports model-family normalization but Rust
  still consumes raw output, or vice versa.

## Minimal Acceptance Criteria

The design is successful when:

- adding a new model family does not require changing orchestrator logic
- adding a new serving stack does not require changing model-family semantics
- thought text cannot leak into artifact writes without passing through an
  explicit normalizer
- tool-call extraction is testable independently of transport
- multimodal ordering policy is configurable per family rather than global

## Open Questions

1. Should the model-family profile be selected explicitly by config, inferred
   from model name, or both?
2. Should runtime capability be inferred from the selected backend, or allow a
   separate override for custom servers?
3. How much of the normalized request/response contract should live in the
   session protocol vs stay internal to each host implementation?
4. For processor-centric models, do we need a transport backend that bypasses
   OpenAI-compatible request construction entirely?
5. Should Claude token counting and prompt caching be surfaced as optional
   runtime hooks in the shared backend interface, or remain opportunistic
   backend-specific optimizations until more providers expose equivalent
   capabilities?

## Summary

`sim-flow` needs a layered adaptation strategy rather than more per-backend
special cases. The correct partition is:

- transport backend for wire protocol
- runtime capability profile for server/integration constraints
- model-family profile for prompt, sampling, and semantic behavior
- response normalizer for converting raw model output into safe internal events

This design keeps specialization well partitioned, lets us support Gemma 4,
Qwen 3.6, Kimi-VL, Claude, and future families cleanly, and protects the
orchestrator from model-specific output quirks.
