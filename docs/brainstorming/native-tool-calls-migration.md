# Native tool-calls migration (scoping doc)

**Status:** draft / scoping. No code changes yet.
**Created:** 2026-05-11
**Owner:** mneilly@numenta.com
**Motivation:** the model-robustness study shows that fenced
artifact writes are a structural bottleneck on smaller open
models. After Phase 0d prompt hardening,
`wrong-fence-info-string` still affected 33% of trials -- and
the failure is one substitution away (the model writes
` ```markdown ` instead of ` ```docs/spec.md `). Real OpenAI /
Anthropic tool calls have a named `path` argument that can't
be confused with a language tag. qwen-code (the official Qwen
Code CLI) uses tool calls exclusively, and the broader vendor
trend is the same direction.

This doc scopes the migration. It does **not** propose code
changes -- the goal is to align on the target architecture,
the module layout (one OpenAI module + one Anthropic module
swappable per model, mirroring qwen-code), and a phased plan
that keeps the existing fenced-block path alive as a fallback
during the transition.

---

## 1. Why fenced blocks are a structural ceiling

From `model-robustness-vllm-anomalies.md` (21 vLLM/qwen3.6
trials):

| anomaly | trials affected | rate | structural? |
|---|---|---|---|
| `wrong-fence-info-string` | 13/21 | 62% | **yes** |
| `work-no-artifact` | 12/21 | 57% | downstream of fence issues |
| `bare-json-no-fence` | 5/21 | 24% | **yes** (critique JSON shape) |

`wrong-fence-info-string` dropped from 92% trials-affected
(Phase 0c) to 33% (Phase 0d) with the prompt hardening pass.
The remaining 33% will not yield to more prompt tuning. The
substitution is one wrong token (` ```markdown ` vs
` ```docs/spec.md `) and the prior pre-training distribution
overwhelmingly favors the language-tag form. The orchestrator
salvage path catches some shapes but cannot magically reroute
a write to the right path when the path was never spoken.

Real function calls eliminate the ambiguity:

- The schema defines `name = "write_file"` and `arguments.path:
  string, arguments.content: string`. The model cannot put a
  language tag where the path goes -- the field is named.
- Vendor backends (OpenAI, Anthropic, vLLM, LM Studio,
  Ollama) all accept `tools: [...]` in the request and stream
  `tool_calls` / `tool_use` blocks back. The shape is
  industry-standard.
- The bare-JSON critique anomaly disappears too: critiques
  become `write_file(path="docs/critiques/<step>-critique.json",
  content="{...}")`.

## 2. What qwen-code teaches

Read of `/Users/mneilly/Projects/ThirdParty/qwen-code` on
2026-05-11. Relevant patterns:

### Clean per-backend module split

`packages/core/src/core/openaiContentGenerator/` and
`packages/core/src/core/anthropicContentGenerator/` are
sibling modules with the same shape:

- a `ContentGenerator` class (the wire-level transport)
- a `converter.ts` (translates between the internal
  representation and the vendor-specific request/response
  shape)
- per-provider sub-modules under `openaiContentGenerator/
  provider/` that customize headers, request shaping, and
  parsing options for sub-vendors (DashScope, DeepSeek,
  Mistral, ModelScope, MiniMax, OpenRouter, default).

The provider sub-module pattern is overkill for us today --
we have one OpenAI-compat provider (vLLM / LM Studio /
Ollama all speak the same dialect) and one Anthropic provider
(api.anthropic.com). But the **converter** split is exactly
what we need: pure functions that translate
`Vec<LlmMessage>` + tool catalog into a backend-specific
request body, and translate the backend's response into
`(assistant_text, tool_calls, metrics)`.

### Tool-call streaming parser

`streamingToolCallParser.ts` handles every messy edge of
streamed tool-call assembly:

- chunks arrive with inconsistent indices / IDs
- arguments are fragmented across N chunks and need to be
  reassembled
- JSON inside `arguments` may be malformed (unclosed strings,
  trailing commas)
- multiple parallel tool calls can be interleaved

It tracks per-tool-call state (depth, in-string, escape) and
attempts parsing only when the structure is closed. Falls back
to `jsonrepair` for malformed payloads. We are non-streaming
today; this becomes relevant if/when we add streaming.

### Tool-result conversation shape

After a tool runs, qwen-code appends a turn with `role: "tool",
tool_call_id: "<id>", content: "<tool output>"` (OpenAI shape)
or a `tool_result` content block (Anthropic shape). The
conversation history is preserved end-to-end so the model can
chain tool calls coherently. We do roughly the same today
with our fenced-block path, but the wire shape will change.

### Anthropic vs OpenAI shape differences

The two converters in qwen-code (1457 lines for OpenAI, 839
for Anthropic) are big because they handle:

- thinking blocks (Anthropic) vs reasoning strings (OpenAI)
- tool_use content blocks (Anthropic) vs tool_calls array
  (OpenAI)
- system as top-level string (Anthropic) vs system message
  in array (OpenAI)
- Anthropic-compatible-but-non-native endpoints (DeepSeek,
  IdeaLab, sglang/vllm) needing per-host workarounds

The DeepSeek-compatible-anthropic example in
`anthropicContentGenerator.ts` (need to inject empty thinking
blocks on `tool_use` turns when thinking is on) is a useful
warning: we will eventually hit one of these and the
converter is the right place to absorb it.

## 3. Current sim-flow state

Most of the infrastructure already exists. The catalog is
in place; the orchestrator already dispatches by name.

### What we have

- `Tool` trait
  (`tools/sim-flow/src/__internal/session/tools/mod.rs:107`)
  with `name() / description() / args_schema() / invoke()`.
- 6 tools registered: `read_file`, `list_dir`, `write_file`,
  `edit_file`, `search`, `run_cargo`. Each implements
  `args_schema()` returning a real JSON Schema.
- `ParsedToolCall { name, body }`
  (`tools/sim-flow/src/__internal/session/tools/mod.rs:316`)
  -- the orchestrator's internal tool-call representation.
- `extract_tool_calls(response_text)` -- parses fenced
  ` ```tool:<name> ` blocks and `\`\`\`json {"name":"...",
  "arguments":...}` blocks into `Vec<ParsedToolCall>`.
- `invoke_tool(dispatcher, ctx, call)`
  (`tools/sim-flow/src/__internal/session/orchestrator.rs:1482`)
  -- runs the parsed call, emits `Event::ToolInvoked`.
- `ProtocolToolDescriptor { name, description, args_schema }`
  (`tools/sim-flow/src/__internal/session/protocol.rs:347`)
  -- sent to hosts on session-start.
- `AnthropicAgent` (`session/agent/anthropic.rs`) -- direct
  Messages API client, splits system out of messages, handles
  stop_reason=max_tokens, no tool support yet.
- `OpenAiCompatibleAgent` (`session/agent/openai_compatible.rs`)
  -- chat/completions client with disable-thinking,
  per-family adaptation, no tool support yet.
- `ModelFamilyProfile` (`session/agent/adaptation.rs`) -- per-
  family knobs (thought_marker_style,
  prefers_bare_json_critique, supports_thinking_controls,
  reasoning_history_policy).

### What's missing

1. **Request-side `tools: [...]` serialization** in both
   `OpenAiCompatibleAgent` and `AnthropicAgent`. We build the
   catalog at session-start (the `ProtocolToolDescriptor`
   list is already there); we just need to include it in the
   wire request body.
2. **Response-side `tool_calls` / `tool_use` parsing.**
   OpenAI: `choices[0].message.tool_calls: [{id, function:
   {name, arguments}}]`. Anthropic: content blocks where
   `type == "tool_use"` carry `{id, name, input}`. Translate
   each into the existing `ParsedToolCall` shape.
3. **Tool-result message shaping.** Today we feed
   `ToolResult.display` back as a User-role text turn.
   Native tool-use needs an OpenAI `{role: "tool",
   tool_call_id, content}` or Anthropic `{role: "user",
   content: [{type: "tool_result", tool_use_id, content}]}`.
4. **Artifact-write convention shift.** Today the
   orchestrator looks for fenced ` ```path ` blocks and
   persists their bodies. With tool calls, every artifact
   becomes a `write_file` call. The orchestrator's
   `extract_artifacts(...)` shrinks to "no-op when tool-mode
   is active" -- writes are already in the tool-call stream.
5. **Prompt rewrites.** Every DM work + critique prompt
   currently ends with a `## Output` section saying "emit
   a fenced block with info-string = <path>." Those become
   "call `write_file(path=..., content=...)`." Tool-call
   prompts are typically shorter (no fence-shape reminder
   needed -- the schema does the work).

## 4. Target module layout

Mirrors qwen-code's split. New layout under
`src/__internal/session/agent/`:

```text
agent/
├── adaptation.rs          # unchanged (model/runtime profiles)
├── mod.rs
├── mock.rs                # unchanged (test agent)
├── anthropic/
│   ├── mod.rs             # AnthropicAgent
│   ├── converter.rs       # LlmMessage <-> Anthropic shape
│   ├── tool_use.rs        # tool_use / tool_result blocks
│   └── thinking.rs        # thinking-block handling (lifted from claude.rs)
├── openai_compat/
│   ├── mod.rs             # OpenAiCompatibleAgent
│   ├── converter.rs       # LlmMessage <-> ChatCompletions shape
│   ├── tool_calls.rs      # tool_calls array <-> ParsedToolCall
│   └── streaming.rs       # (deferred) StreamingToolCallParser port
└── (legacy: openai_compatible.rs, anthropic.rs)
    # Kept as thin shims that re-export the new modules until
    # the cutover is complete; then deleted.
```

Selection between modules happens at session start (already
the case today: model id => family => runtime profile). The
existing `claude.rs / codex.rs / gh_copilot.rs / ollama.rs`
shims either keep their wrapper roles or get folded into the
two main modules (most are thin profile aliases).

### What each converter does (Rust types, sketch)

```rust
// agent/openai_compat/converter.rs

pub struct OpenAiRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<RequestMessage<'a>>,
    pub tools: Option<Vec<ToolDescriptor>>,
    pub tool_choice: Option<&'static str>, // "auto" | "required"
    pub max_tokens: u32,
    pub seed: Option<u32>,
    pub chat_template_kwargs: Option<ChatTemplateKwargs>,
}

pub fn build_request(
    messages: &[LlmMessage],
    tool_catalog: &[ProtocolToolDescriptor],
    family: &ModelFamilyProfile,
    knobs: &RequestKnobs,
) -> OpenAiRequest<'_> { /* ... */ }

pub struct OpenAiResponse {
    pub assistant_text: String,
    pub tool_calls: Vec<ParsedToolCall>,
    pub metrics: LlmCallMetrics,
}

pub fn parse_response(
    raw: ChatResponseBody,
    family: &ModelFamilyProfile,
) -> Result<OpenAiResponse> { /* ... */ }
```

```rust
// agent/anthropic/converter.rs

pub struct AnthropicRequest<'a> {
    pub model: &'a str,
    pub system: Option<String>,
    pub messages: Vec<AnthropicMessage>,
    pub tools: Option<Vec<AnthropicToolDescriptor>>,
    pub max_tokens: u32,
    pub thinking: Option<AnthropicThinkingConfig>,
}

pub fn build_request(
    messages: &[LlmMessage],
    tool_catalog: &[ProtocolToolDescriptor],
    family: &ModelFamilyProfile,
    knobs: &RequestKnobs,
) -> AnthropicRequest<'_> { /* ... */ }

pub struct AnthropicResponse {
    pub assistant_text: String,
    pub tool_calls: Vec<ParsedToolCall>,
    pub thinking_blocks: Vec<ThinkingBlock>,
    pub metrics: LlmCallMetrics,
}

pub fn parse_response(
    raw: AnthropicMessageResponse,
    family: &ModelFamilyProfile,
) -> Result<AnthropicResponse> { /* ... */ }
```

Both expose the same `ParsedToolCall` to the orchestrator, so
nothing in `orchestrator.rs` cares which backend served the
turn.

### `LlmMessage` extension

Today: `LlmMessage { role, content, attachments }`.

Needs to carry tool-call metadata so conversation history can
be reconstructed for the next turn:

```rust
pub struct LlmMessage {
    pub role: LlmRole,          // System | User | Assistant | Tool (NEW)
    pub content: String,
    pub attachments: Vec<LlmAttachment>,
    pub tool_calls: Vec<ToolCallRef>,  // NEW: on Assistant turns
    pub tool_call_id: Option<String>,  // NEW: on Tool turns
}
```

Existing fenced-block tool calls flow into the same fields --
the converter on the way out decides whether to emit them as
native tool_calls or as text (compat with non-tool-supporting
backends).

## 5. Tool catalog as JSON Schema

`Tool::args_schema()` already returns JSON Schema. Verify each
existing schema is OpenAI-compatible (no exotic keywords,
`type: "object"`, `properties: {...}`, `required: [...]`,
`additionalProperties: false`). Spot-check:

| tool | current schema status |
|---|---|
| `read_file` | `{ path: string }` -- ok |
| `list_dir` | `{ path: string }` -- ok |
| `write_file` | `{ path: string, content: string }` -- ok |
| `edit_file` | `{ path: string, old_string: string, new_string: string }` -- ok |
| `search` | `{ pattern: string, path: string }` -- ok |
| `run_cargo` | `{ subcommand: string, args: array<string> }` -- needs verification |

Add `additionalProperties: false` and `description` strings
to each schema so the model's auto-completion is sharp. Most
of the current schemas only have `type` + `properties` -- the
extra fields are cheap and meaningfully improve tool-call
quality on smaller models.

## 6. Migration phases

Each phase is independently shippable. Each ends with K=3 on
vLLM/qwen3.6 to measure impact, mirroring the Phase 0/0b/0c/
0d cadence.

### Phase A: module split (no behavior change)

Move existing `openai_compatible.rs` / `anthropic.rs` into the
`agent/{openai_compat,anthropic}/` directory structure with
`converter.rs` carved out. No new functionality. Verify all
67 agent unit tests still pass and e2e_auto / e2e_manual
still walk DM0 -> DM2cd on the smoke fixture.

Risk: low. Pure refactor.
Effort: 1 day.

### Phase B: native tool_calls on OpenAI-compat (new code path, off by default)

Add `tools: [...]` to the OpenAI request, parse `tool_calls`
from the response. Gate behind `SIM_FLOW_TOOL_MODE=native`
env var so the existing fenced path stays default. Run K=3
on vLLM with `SIM_FLOW_TOOL_MODE=native` and compare:

- `wrong-fence-info-string` rate (expected: 0)
- `bare-json-no-fence` rate (expected: 0, since critiques
  become tool calls)
- advance depth median (expected: at least DM2cd, preferably
  DM3+)
- `work-no-artifact` rate (expected: << 33%)

If the data confirms the structural improvement, proceed to
Phase C. If not, the prompt-rewrite premise is wrong and we
should revisit.

Risk: medium. Wire-level changes to the request body; vLLM /
LM Studio may have undocumented quirks (qwen-code's per-
provider provider/ folder exists for a reason).
Effort: 3-4 days including the K=3 measurement.

### Phase C: native tool_use on Anthropic (new code path, off by default)

Same delta as Phase B but for the Anthropic API. Run K=1 on
Claude Opus 4.7 (paid; minimize trials).

Risk: medium. Claude's tool_use shape is different from
OpenAI's tool_calls. Need to handle thinking blocks
interleaved with tool_use blocks.
Effort: 2-3 days.

### Phase D: prompt rewrite + cutover

Rewrite every DM work + critique prompt's `## Output` section
to reference tool calls. Flip `SIM_FLOW_TOOL_MODE=native` to
default. Keep the fenced-block path as a fallback for
backends that don't advertise tool support (`mock.rs` test
agent, future plain-completion endpoints).

Risk: medium-high. Prompts are the user-facing contract;
breaking them affects every step.
Effort: 2-3 days (mostly prompt content + critique JSON
schema migration).

### Phase E: cleanup

Delete the dead fenced-block code paths once tool-mode has
been the default for two weeks without regressions. Squash
the legacy shims. Update `08-orchestrator-tools.md` and
`09-multi-model-adaptation.md`.

Risk: low.
Effort: 1 day.

## 7. Risks and open questions

### vLLM tool-call support quality

The vLLM instance on `localhost:8012` is already configured
with `--tool-call-parser qwen3_coder`. This parser extracts
the XML-tagged tool-call format Qwen-Coder is trained for:

```text
<tool_call>
<function=write_file>
<parameter=path>
docs/spec.md
</parameter>
<parameter=content>
...file body...
</parameter>
</function>
</tool_call>
```

vLLM does the format translation transparently: we send the
standard OpenAI `tools: [...]` parameter, vLLM injects the
schema into the chat template, the model emits the XML, the
parser extracts it, and the response carries standard OpenAI
`tool_calls: [{id, function: {name, arguments}}]`. Same shape
Claude Code and Codex see (which the operator confirmed
works against this endpoint).

**Three practical implications:**

- Phase B's converter does NOT need qwen-specific handling.
  vLLM owns the XML translation; we work in the OpenAI
  abstraction.
- The model is RLHF'd to emit structured tool calls. The
  pre-training distribution actively prefers the structured
  form -- opposite of the fenced-markdown bias that hurts us
  today.
- The `chat_template_kwargs.enable_thinking: false` knob we
  already ship is orthogonal to tool calls and should
  continue to work.

**Caveat resolved (2026-05-11)**: the Hugging Face model card
for `Qwen/Qwen3.6-27B` explicitly recommends
`--tool-call-parser qwen3_coder` for tool use on this model.
The "qwen3_coder" parser name refers to the **XML output
format** Qwen3.6 emits, not to the qwen3-coder model variant.
The operator's actual serving config:

```bash
vllm serve Qwen/Qwen3.6-27B \
    --attention-backend flashinfer \
    --default-chat-template-kwargs '{"preserve_thinking": true}' \
    --dtype auto \
    --enable-auto-tool-choice \
    --enable-chunked-prefill \
    --enable-prefix-caching \
    --generation-config auto \
    --override-generation-config '{"temperature": 0.6, "presence-penalty": 0.0}' \
    --gpu-memory-utilization 0.9658 \
    --language-model-only \
    --load-format instanttensor \
    --max-model-len 262144 \
    --performance-mode interactivity \
    --reasoning-parser qwen3 \
    --served-model-name qwen3.6 \
    --speculative-config '{"method": "mtp", "num_speculative_tokens": 2}' \
    --max-num-batched-tokens 8192 \
    --tensor-parallel-size $SLURM_GPUS_ON_NODE \
    --tool-call-parser qwen3_coder
```

Three observations from this config that change our plan:

- **Server-side sampling defaults are tuned for thinking
  mode**, not the non-thinking mode we actually run.
  `--override-generation-config` sets `temperature: 0.6`
  (the card's "precise coding" thinking-mode value) and
  `presence_penalty: 0.0` (also thinking-mode). Since
  sim-flow sends `enable_thinking: false`, we are running
  non-thinking inference with **thinking-mode sampling
  defaults** -- the worst of both worlds. Section 7b is
  now the highest-priority pre-migration hardening
  (was: nice-to-have).
- **`--reasoning-parser qwen3` is enabled.** The server
  already splits `<think>` content out into
  `choices[0].message.reasoning_content`. Our client-side
  `strip_known_reasoning_markers` for Qwen turns is now
  dead code on this endpoint. Defer cleanup until after
  Phase A; keep the strip pass alive for backends that
  don't have the reasoning parser configured.
- **`--enable-auto-tool-choice` is enabled.** We can use
  `tool_choice: "auto"` (or omit `tool_choice` entirely)
  without worrying that vLLM will reject it. Phase B's
  request shape is unblocked.

Pre-Phase-B smoke test (verifies that vLLM's translation
round-trips a standard OpenAI tools request into our
expected response shape):

```bash
curl -s http://localhost:8012/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{
    "model":"qwen3.6",
    "messages":[{"role":"user","content":"list the current directory using the list_dir tool"}],
    "tools":[{"type":"function","function":{
      "name":"list_dir",
      "description":"List a directory",
      "parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}}],
    "tool_choice":"auto",
    "max_tokens":2048
  }' | jq '.choices[0].message'
```

Expected on success: `message.tool_calls: [{id, type:
"function", function: {name: "list_dir", arguments:
"{\"path\":\".\"}"}}]`. Anything else (empty `tool_calls`,
tool call body in `content`, parse errors) means
`--enable-auto-tool-choice` is missing or some other vLLM
flag is off; coordinate with the operator before proceeding.

### Reasoning parser separates thinking from content

The model card also recommends `--reasoning-parser qwen3`.
With that flag set, the response's `choices[0].message`
splits into:

- `content: "..."` -- the user-visible answer
- `reasoning_content: "..."` -- the model's thinking text
  (was `<think>...</think>` inline before)

Our current `decode_choice` already prefers `content` and
falls back to `reasoning` when content is empty, so the wire
path works either way. Open follow-up after Phase A: if the
operator has the reasoning parser enabled, we can drop the
client-side `strip_known_reasoning_markers` pass for Qwen
turns (the server already split them out). Cheap and removes
a code path; defer until we confirm the operator's config
includes `--reasoning-parser qwen3`.

### Sampling parameters Qwen3.6 wants

The model card prescribes specific sampling values for
**non-thinking (Instruct) mode** -- the mode we use today
via `chat_template_kwargs.enable_thinking: False`:

| param | recommended | current sim-flow default |
|---|---|---|
| `temperature` | 0.7 | unset (vLLM default = 1.0) |
| `top_p` | 0.80 | unset (= 1.0) |
| `top_k` | 20 | unset (= -1) |
| `min_p` | 0.0 | unset |
| `presence_penalty` | **1.5** | unset (= 0.0) |
| `repetition_penalty` | 1.0 | unset |

The standout is `presence_penalty=1.5`. The card explicitly
flags it as the lever to "reduce endless repetitions" -- which
maps onto our `runaway-loop` anomaly (1/21 trials) and is also
suspected to drive a chunk of `work-no-artifact` (model
re-emits identical reading-only turns without writing). This
is a separate, much smaller hardening that can ship now,
independent of the tool-calls migration. Tracked as a
follow-up action item below.

Recommended values for **thinking mode** (different;
documented for completeness):
`temperature=1.0, top_p=0.95, top_k=20, min_p=0.0`
(presence_penalty stays at 0). We never want this with our
current workflow but it's the right setting if we ever flip
disable-thinking off.

### Context length

Native 262K tokens, extensible to 1M via YaRN. Recommended
max output 32K typical, 81K for complex math/coding. Our
current 65K default sits comfortably between the two and
leaves plenty of context room. No change needed.

### Streaming vs non-streaming

We are non-streaming today (single response body, parse the
whole thing). Tool-call streaming adds complexity
(`streamingToolCallParser.ts` is 350+ lines for a reason).
Defer streaming until we have data showing it's needed.

### Backwards compat with fenced-block muscle memory

Some operators may have local DM prompt overrides under
`.sim-flow/prompts/` that still use the fenced convention.
The fallback path should remain operational for at least one
release after the cutover so overrides don't silently break.

### Critique JSON schema

Today's critiques are a markdown file at
`docs/critiques/<step>-critique.json` with the JSON as the
body. With tool calls, the path/content split is cleaner but
the JSON schema needs to be explicit (we relied on the
balanced-brace salvage today). Worth adding a strict schema
check + a friendlier diagnostic when arguments don't match.

### Mock agent

`mock.rs` currently returns fenced-block responses for the
e2e tests. Either teach the mock to emit `ParsedToolCall`
records directly (cleaner), or keep it on the fenced path
and document that the mock exercises the fallback. The first
is the right call long-term.

## 7b. Pre-migration hardening: Qwen3.6 sampling params

The HF model card recommendations above can ship before any
of the tool-calls migration phases. Effort: ~1 hour. Impact:
direct mitigation of `runaway-loop` and the
re-reading-without-writing pattern that drives
`work-no-artifact`.

Implementation outline:
- Add `temperature`, `top_p`, `top_k`, `min_p`,
  `presence_penalty` fields to `OpenAiCompatibleRequest` and
  the wire `ChatRequestBody`.
- Source defaults from the model family profile: a new
  `recommended_sampling: Option<SamplingDefaults>` on
  `ModelFamilyProfile`. `qwen3_6` carries the non-thinking
  mode values. Other families default to `None` (no override
  on the request -- the server's defaults stand).
- Honor env overrides (`SIM_FLOW_TEMPERATURE`,
  `SIM_FLOW_TOP_P`, etc.) for ad-hoc tuning.
- Skip sampling override emission when `disable_thinking ==
  false` and the family has separate thinking-mode defaults
  (today: never -- we always disable thinking).
- Add unit tests covering: (a) request body includes the
  Qwen-recommended values when the family is `qwen3_6`,
  (b) request body omits them when the family is
  `generic_chat` or `claude_messages`,
  (c) env override beats the family default.
- K=3 rerun on vLLM with these knobs to measure
  `runaway-loop` and `work-no-artifact` impact.

This is independent of the migration but should land first --
it's faster to validate and any improvement carries forward
into Phase B's measurements.

## 8. What we are NOT doing in this migration

- Streaming responses. Out of scope.
- Per-step tool gating. Memory `feedback_per_step_tool_gating`
  is explicit: sim-flow uses a universal tool catalog and
  per-step gating was removed. Stays removed.
- Adding more providers (DashScope, OpenRouter, MiniMax,
  etc.). The qwen-code provider/ folder is overkill for us
  today; revisit if/when we add a real second OpenAI-compat
  provider beyond vLLM/LM Studio/Ollama.
- Rewriting `mock.rs` to use a real backend. Tests stay
  hermetic.

## 9. Success criteria

After Phase D ships:

- `wrong-fence-info-string`: 0 events / 21 trials on vLLM
  smoke fixture
- `bare-json-no-fence`: 0 events / 21 trials
- `work-no-artifact`: < 20% trials-affected
- Median advance depth: at least DM3a on vLLM/qwen3.6 smoke
  fixture
- Anthropic Opus 4.7 K=1: cleaner critique-no-progress path
  (no salvage warnings, structured critique JSON)
- No regression on Claude / generic-chat (which already
  emitted fenced blocks reliably)

If those land, the structural anomaly category is closed.
Remaining anomalies (`edit-file-stale-old-string`,
`work-gate-still-dirty`) are then the next focus.
