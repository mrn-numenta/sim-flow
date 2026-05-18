# Chapter 4: Agent Tools

This chapter specifies four new agent-facing tools wired into
the universal tool catalog alongside the existing LSP tools
(`api_search`, `api_hover`, `api_impls`, `api_references`,
`api_expand_macro`), file tools (`read_file`, `write_file`,
`edit_file`, etc.), and project tools (`run_cargo`, `search`,
`list_dir`). Three are retrieval tools backed by the LanceDB
index from Chapter 3; the fourth (`ask_user`) is a
user-interaction tool the agent invokes when a TBD, an
ambiguous design choice, or any other blocking question
prevents forward progress.

## 4.1 Tools at a Glance

| Tool | Category | Use case |
| --- | --- | --- |
| `api_semantic_search` | retrieval (L1) | "I don't know the name; find me candidate framework symbols" |
| `spec_semantic_search` | retrieval (L2) | "Find source-spec sections matching this question" |
| `signal_table_query` | retrieval (L7) | "Find / compare signal-table rows by structured filter" |
| `ask_user` | user-interaction | "I cannot proceed without an answer the spec doesn't give me; ask the user" |

The L6 cross-spec metadata is consumed via
`spec_semantic_search` filters (`source = <peer_id>`) and via
direct file reads of `references.toml`; it does not get its own
top-level tool.

## 4.2 api_semantic_search

### Signature

```
api_semantic_search(
    query: string,
    k?: int = 8,
    kind?: string?
) -> ApiSemanticHits
```

### Argument schema (JSON-schema for native tool calls)

```json
{
  "type": "object",
  "required": ["query"],
  "properties": {
    "query": {
      "type": "string",
      "description": "Natural-language description of the framework concept, signature shape, or behavior you need."
    },
    "k": {
      "type": "integer",
      "minimum": 1,
      "maximum": 20,
      "default": 8,
      "description": "Number of candidates to return."
    },
    "kind": {
      "type": "string",
      "enum": ["api-page", "src-fn", "src-impl", "src-trait", "src-mod-doc", "src-other"],
      "description": "Optional filter restricting results to one chunk kind."
    }
  }
}
```

### Return shape

```json
{
  "hits": [
    {
      "path": "fw:src/model/dataflow/mod.rs",
      "name": "HasLogic",
      "kind": "src-trait",
      "snippet": "...the first paragraph of the chunk plus the canonical signature line if detectable...",
      "score": 0.42
    }
  ],
  "embedder_used": "openai-compat:nomic-embed-text",
  "elapsed_ms": 87
}
```

`score` is L2 distance; lower is better. Returned in ascending
order. The agent treats this as a ranked list; absolute values
are informational.

`snippet` is the first paragraph (up to 500 chars) of the
chunk's text plus, when detectable from `kind = src-*`, the
canonical signature line. The agent's next move is typically to
`api_hover` one of the returned `name` values for the live
signature.

### Prompt nudge

Added to the DM2d / DM3b / DM3c work prompts and to the
universal-tools system message:

> **Framework-symbol discovery (when you don't know the name).**
> Call `api_semantic_search` with a natural-language description
> of what you need before guessing at a symbol name. Then call
> `api_hover` on each promising candidate to verify the live
> signature. Never write against a framework signature you have
> not `api_hover`ed — `api_semantic_search` returns approximate
> matches, `api_hover` returns truth.

### Failure modes

- **Lance index missing**: returns
  `{"error": "framework index not built; run `sim-flow build-framework-index`"}`.
- **Embedder mismatch**: returns
  `{"error": "embedder mismatch — index built with <X>, configured <Y>"}`.
- **Embedder unreachable**: returns
  `{"error": "embedder at <url> not responding (cause: ...)"}`.

Errors are surfaced as `Result::Err` from the tool dispatch and
re-fed to the agent as a tool-result with `status = error`.

## 4.3 spec_semantic_search

### Signature

```
spec_semantic_search(
    query: string,
    k?: int = 5,
    source?: string?,
    kind?: string?
) -> SpecSemanticHits
```

### Argument schema

```json
{
  "type": "object",
  "required": ["query"],
  "properties": {
    "query": {
      "type": "string",
      "description": "Natural-language description of the spec content you need."
    },
    "k": {
      "type": "integer",
      "minimum": 1,
      "maximum": 20,
      "default": 5
    },
    "source": {
      "type": "string",
      "description": "Optional filter: 'primary' for the project's primary spec, or a peer id from manifest.toml."
    },
    "kind": {
      "type": "string",
      "enum": ["prose", "table", "stub", "mixed"]
    }
  }
}
```

### Return shape

```json
{
  "hits": [
    {
      "chunk_id": "<sha256>",
      "source_id": "primary",
      "breadcrumb": ["Introduction to the RV12", "Execution Pipeline", "Instruction Fetch (IF)"],
      "section_heading": "Instruction Fetch (IF)",
      "source_page_range": [13, 14],
      "snippet": "...first 500 chars of the chunk body...",
      "chunk_path": ".sim-flow/spec-ingest/primary/chunks/0118-instruction-fetch.md",
      "contained_signal_tables": ["tables/signals/003-if.toml"],
      "contained_figures": ["figures/page-013.png"],
      "score": 0.31
    }
  ],
  "embedder_used": "openai-compat:nomic-embed-text",
  "elapsed_ms": 54
}
```

`chunk_path` is the relative path of the chunk's markdown file
on disk. The agent reads the full chunk via `read_file
<chunk_path>` when the snippet is insufficient.

`contained_signal_tables` and `contained_figures` let the
agent jump from "I found the relevant section" to "give me the
structured signal table" or "show me the figure" without a
second search.

### Prompt nudge

> **Source-spec retrieval.** When you need detail beyond what
> spec.md normalizes, call `spec_semantic_search` with a
> natural-language description. It returns the most relevant
> source-spec sections; use the returned `chunk_path` to
> `read_file` the full body when needed. Each hit's
> `breadcrumb` tells you where in the source spec hierarchy
> the chunk lives.
>
> spec.md is the normalized truth for the design; the source
> spec is the underlying material spec.md was derived from. Use
> `spec_semantic_search` when spec.md does not carry enough
> detail; otherwise prefer the structured artifacts in spec.md.

### Failure modes

Mirror `api_semantic_search`. Additional:

- **No spec corpus**: when the project has no source spec (the
  no-source case discussed in §1.2), this tool returns
  `{"hits": [], "note": "no source spec registered for this project"}`.
  Not an error; the agent's authoring loop is the path.

## 4.4 signal_table_query

### Signature

```
signal_table_query(
    filter: SignalTableFilter,
    conflicts_only?: bool = false,
    limit?: int = 50
) -> SignalTableHits
```

### Argument schema

```json
{
  "type": "object",
  "required": ["filter"],
  "properties": {
    "filter": {
      "type": "object",
      "properties": {
        "signal_name": { "type": "string" },
        "stage": { "type": "string" },
        "peer": { "type": "string" },
        "direction": { "type": "string", "enum": ["in", "out", "inout"] },
        "source_kind": { "type": "string", "enum": ["source-spec", "spec-md"] },
        "source_id": { "type": "string" }
      },
      "additionalProperties": false,
      "description": "Equality filters; any subset. AND'd together. Omit for no filter on that field."
    },
    "conflicts_only": {
      "type": "boolean",
      "default": false,
      "description": "When true, return only rows where spec.md disagrees with the source spec on the same (stage, signal_name) pair."
    },
    "limit": {
      "type": "integer",
      "minimum": 1,
      "maximum": 500,
      "default": 50
    }
  }
}
```

### Return shape

```json
{
  "rows": [
    {
      "row_id": "<sha256>",
      "source_kind": "source-spec",
      "source_id": "primary",
      "chunk_id": "<sha256>",
      "stage": "Instruction Fetch (IF)",
      "breadcrumb": ["Introduction to the RV12", "Execution Pipeline", "Instruction Fetch (IF)"],
      "signal_name": "if_nxt_pc",
      "direction": "out",
      "peer": "Bus Interface",
      "description": "Next address to fetch parcel from"
    }
  ],
  "total_matching": 11,
  "limited": false,
  "conflict_pairs": []
}
```

When `conflicts_only = true`:

```json
{
  "rows": [],
  "conflict_pairs": [
    {
      "stage": "Instruction Fetch (IF)",
      "signal_name": "if_nxt_pc",
      "spec_md_row": { /* row */ },
      "source_spec_row": { /* row */ },
      "differs_on": ["direction"]
    }
  ]
}
```

`differs_on` enumerates which fields disagree (`direction`,
`peer`, `description`). spec.md is treated as authoritative;
the agent's typical response to a non-trivial mismatch is to
ask the user (manual mode) or record an Auto-decision
(automated mode).

### Prompt nudge

> **Signal-contract query.** To find I/O signals for a block /
> stage or to look up a specific signal across all blocks,
> call `signal_table_query`. Use `conflicts_only=true` after
> editing spec.md to verify your changes match the source spec
> on signal direction, peer, and meaning.

### Failure modes

- **No signal-table data**: returns `{"rows": [], "note": "no
  signal tables registered for this project"}`. Not an error.
- Other failures mirror the other tools.

## 4.5 ask_user

The user-interaction tool. Distinct from the three retrieval
tools in three respects:

1. It does not query a Lance index. It surfaces a question to
   the user via the existing chat-panel transport and returns
   the user's reply.
2. It carries different scheduling semantics: it is a
   **turn-boundary** tool. The agent emits it only when it
   cannot make further progress without an answer. If the
   agent can complete other useful work in the current turn
   first (writing a partial artifact, recording a hypothesis,
   reading more context), it should do that work and only
   then call `ask_user` as the closing tool call of the turn.
3. It triggers a **step-mode flip** when invoked during an
   automated run: the orchestrator transitions
   `step_mode = auto` to `step_mode = manual` before
   surfacing the question, preserving the "automated = no
   human in the loop" policy by exiting that mode the moment
   one is needed.

### Signature

```
ask_user(
    question: string,
    context?: string,
    kind?: string,
    choices?: list<string>,
    default?: string,
    record_as?: string,
    thread_id?: string
) -> AskUserAnswer
```

### Argument schema

```json
{
  "type": "object",
  "required": ["question"],
  "properties": {
    "question": {
      "type": "string",
      "description": "The question to surface to the user. Keep it focused and answerable in one short reply."
    },
    "context": {
      "type": "string",
      "description": "Optional. One short paragraph explaining why you're asking and what's blocked. The user sees this above the question."
    },
    "kind": {
      "type": "string",
      "enum": ["free-form", "yes-no", "choice", "value"],
      "default": "free-form",
      "description": "Shape of the expected reply. 'choice' requires the 'choices' array; 'value' may carry a 'default'."
    },
    "choices": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Required when kind='choice'. The chat panel renders these as quick-reply chips."
    },
    "default": {
      "type": "string",
      "description": "Optional default the user gets if they reply with empty input or 'default'."
    },
    "record_as": {
      "type": "string",
      "enum": ["open-question", "auto-decision", "none"],
      "default": "open-question",
      "description": "How to persist this Q+A pair in spec.md. 'open-question' appends a resolved Open Question; 'auto-decision' records as an Auto-decision; 'none' is ephemeral (not persisted). For a chained thread, this value applies to the WHOLE thread; intermediate calls in a thread should use 'none' and only the closing call sets the resolved record_as."
    },
    "thread_id": {
      "type": "string",
      "description": "Optional. Omit on a fresh question. To chain a follow-up clarification onto a prior question, pass the same thread_id you used (or that was returned to you) on the prior ask_user call. The orchestrator groups same-thread calls into one conversation thread in the chat panel and coalesces their persistence into a single spec.md entry."
    }
  }
}
```

### Return shape

```json
{
  "answer": "...",                             // verbatim user reply
  "kind": "free-form",                         // echoes the kind from the call
  "thread_id": "ask-2026-05-17-1842-0001",     // either echoed from the call, or generated by the orchestrator for a fresh thread
  "thread_turn_index": 0,                      // 0 for the first call in a thread; increments per follow-up
  "recorded_at": "spec.md#open-questions",     // anchor where persisted, or "" for none / for intermediate calls
  "mode_changed": "auto-to-manual",            // present when the call flipped step-mode
  "elapsed_ms": 18742,                         // wall-clock the agent was paused for
  "cancelled": false,                          // true when the user cancelled (this call OR the whole thread)
  "thread_cancelled": false                    // true when the user issued /cancel-thread mid-conversation
}
```

`answer` is the literal user reply text. The agent is
responsible for interpreting it (parsing as a value,
validating against `choices`, etc.). When `default` was set
and the user reply was empty, `answer` equals `default`.

### Behavior

The tool dispatches into the orchestrator's session-flow
control rather than into a standalone async function. The
dispatch path:

1. **Validate args.** If `kind = "choice"` but `choices` is
   empty / missing, return a structured error. The agent
   self-corrects.
2. **Mode handling.**
   - If current step-mode is `manual`: no mode change.
   - If current step-mode is `auto`: emit a
     `StepModeChanged { mode: manual }` event, persist the
     change to `state.toml`, and continue. Subsequent steps
     remain in manual mode until the user explicitly flips
     back via the chat panel's mode toggle.
3. **Surface the question.** Emit a `RequestUserInput` event
   with `prompt = question`, `placeholder = (kind hint)`, and
   a structured `followups` list when `kind = "choice"`.
4. **Park the work session.** The orchestrator suspends the
   current LLM turn after the model emits the tool call. The
   agent does not receive a tool-result yet.
5. **Receive the user's reply.** A `UserMessage` from the
   chat panel resumes the suspended turn. The orchestrator
   packages the reply as the `ask_user` tool result and
   delivers it as the next message in the agent's tool-result
   stream.
6. **Persist.** Per `record_as`, append the Q+A to
   `docs/spec.md`'s Open Questions or Auto-decisions section,
   or to the session's open-questions buffer when spec.md is
   not yet authored (DM0 picks this up).
7. **Return.** The agent's next LLM call receives the
   `AskUserAnswer` as the `ask_user` tool result and
   continues.

### Chaining for incomplete or ambiguous answers

Users do not reliably answer in a single shot. A real reply
may be partial ("the first one, yes — not sure about the
rest"), ambiguous ("probably the standard way"), incomplete
("4 entries, I think"), or itself a question ("why are you
asking?"). The agent must be able to clarify, and the
orchestrator must support chains of follow-up `ask_user`
calls as a normal mode of operation — not as an exception.

The chaining primitive is `thread_id`.

**Starting a thread.** When the agent emits `ask_user`
without a `thread_id`, the orchestrator generates one (e.g.
`ask-<step>-<unix-ms>-<random>`) and returns it in the
`AskUserAnswer.thread_id` field. This becomes the handle for
any follow-up calls.

**Continuing a thread.** When the agent emits `ask_user`
with a `thread_id` matching a prior call, the orchestrator
treats it as a follow-up:

- The chat panel renders it as a connected reply in the
  same conversation (visually grouped, threaded).
- The `context` field SHOULD restate or quote the prior
  question + answer so the user has continuity (the chat
  panel may also surface the history natively, but the
  agent shouldn't depend on it).
- Persistence is **deferred**: the orchestrator does not
  write to spec.md on intermediate calls. The agent
  uses `record_as = "none"` on intermediate calls in the
  thread.

**Closing a thread.** The agent ends a thread by emitting
a final `ask_user` call with `record_as` set to
`"open-question"` or `"auto-decision"`. The orchestrator
then coalesces the thread into ONE spec.md entry:

- For `record_as = "auto-decision"`: a single Auto-decision
  row whose `decision` field is the synthesized final
  answer and whose `rationale` is the user's wording at
  thread close (plus optionally a one-line "(arrived at
  through N rounds of clarification)" annotation).
- For `record_as = "open-question"`: a single resolved
  Open Question entry whose body is the original question
  and whose resolution is the synthesized final answer.

The intermediate Q+A turns are NOT written to spec.md by
default. They live in the orchestrator's session log and
the chat panel transcript for audit; the spec.md entry is
the clean resolved form.

**When the agent decides a thread is complete.** The agent
is the judge of "is this answer good enough?" The prompt
nudge (below) describes the heuristic: if the answer
contains all the information the agent needs to proceed,
close the thread; otherwise chain another `ask_user`. The
agent should NOT chain indefinitely — after a reasonable
number of clarifications (configurable; default 5), the
orchestrator emits a `Diagnostic::Warning`
(`ask_user thread {id} exceeded 5 turns; consider recording
a TBD and moving on`) and the agent is expected to either
close the thread with the best available answer or abandon
it (close with `record_as = "open-question"` marked
unresolved).

**Thread cancellation by the user.** At any point during a
thread the user can issue `/cancel-thread` (typed into the
chat panel) to abort the conversation. The orchestrator:

- Returns `{ cancelled: true, thread_cancelled: true,
  answer: "" }` to the agent's pending call.
- Persists the thread to spec.md's Open Questions as
  unresolved, with body "User cancelled clarification after
  N exchanges. Last reply: `{text}`."
- The agent's prompt convention: record the cancellation,
  proceed with a documented assumption / best effort, and
  continue. Do NOT immediately re-open the thread.

**Interleaved threads.** The agent MAY have multiple
threads open concurrently (e.g. one question per
MissingField in DM0's no-source loop). Each `ask_user` call
carries its own `thread_id`; the orchestrator handles them
independently. The chat panel renders them as distinct
conversations. The agent is responsible for not
double-asking the user the same thing.

**Persistence ordering on resume.** When the orchestrator
buffers intermediate calls in a thread, it keeps the buffer
in `<project>/.sim-flow/<step>/ask-threads/<thread_id>.toml`
so a reload mid-thread recovers cleanly. On thread close,
the buffer is consumed (single spec.md write); on session
end with open threads, the buffers are preserved and a
subsequent session can resume them.

### Turn-boundary discipline

The agent is responsible for scheduling. Prompts (Chapter 6)
direct the agent to:

- Complete every other useful operation it can in the
  current turn.
- Call `ask_user` LAST in the turn — as the closing tool
  call — so the model's working memory at resume includes
  what just happened plus the user's answer.

The tool does not enforce this with a runtime check (the
orchestrator does not parse the tool-call order). It is a
prompt convention. Violation produces a wasted turn (the
agent emits `ask_user` then more tool calls; the orchestrator
suspends at `ask_user`; the rest is discarded). The Phase 7
prompt rewrites make the convention explicit.

### Prompt nudge

Added to the universal-tools system message:

> **When you cannot proceed without an answer the spec
> doesn't give you, call `ask_user` as the LAST tool call of
> the turn.** Complete every other useful operation you can
> first (write the parts of the artifact you have, record
> hypotheses, read more context). Then call `ask_user` with
> a focused, answerable question. The user's reply arrives
> as the tool result on the next turn.
>
> Do NOT call `ask_user` to confirm a decision you already
> have evidence for in the spec. Do NOT call `ask_user` for
> framework-symbol questions (use `api_semantic_search` and
> `api_hover` instead). Do NOT call `ask_user` for spec
> detail that's retrievable (use `spec_semantic_search`
> first).
>
> If you are running in automated mode and you call
> `ask_user`, the run flips to manual mode for the rest of
> the session. This is intentional — once a human is needed,
> automated mode no longer applies.
>
> **When the user's reply is incomplete or ambiguous, chain a
> follow-up.** Real answers are often partial. The
> `AskUserAnswer` you receive carries a `thread_id`; pass that
> same `thread_id` on a follow-up `ask_user` call to continue
> the conversation in the same chat-panel thread. Use
> `record_as = "none"` on intermediate clarification calls;
> only the LAST call in the thread (the one that resolves the
> question) sets `record_as = "open-question"` or
> `"auto-decision"` and triggers persistence to spec.md.
> Restate or quote prior Q+A in the `context` field so the
> user has continuity.
>
> Heuristics for "is the answer good enough to close the
> thread":
>
> - The reply contains a concrete value / decision you can
>   apply.
> - The reply explicitly defers to your judgment (e.g. "you
>   pick", "doesn't matter", "use the default") — close the
>   thread, record the decision you made and the user's
>   delegation in the Auto-decision rationale.
> - The reply is a non-answer ("not sure", "ask me later",
>   "skip") — record a TBD via `record_as = "open-question"`
>   marked unresolved and close.
>
> Do NOT chain indefinitely. After ~3 rounds without
> convergence, close the thread with the best-available
> answer (or as an unresolved Open Question) and proceed.
> The orchestrator emits a warning at 5 rounds.

### Failure modes

- **Malformed args**: tool returns a structured error; the
  agent retries with corrected args.
- **User cancels a single turn** (closes the chat panel,
  types `/cancel`): tool returns `{ "answer": "",
  "cancelled": true, "thread_cancelled": false }`. The agent
  may either close the thread (record what it has so far) or
  abandon (record a TBD and proceed).
- **User cancels the whole thread** (types `/cancel-thread`):
  tool returns `{ "answer": "", "cancelled": true,
  "thread_cancelled": true }`. The orchestrator
  auto-persists the thread to Open Questions as unresolved.
  The agent records a documented assumption and proceeds; do
  NOT immediately re-open the thread.
- **Unknown `thread_id`** (agent passes a thread_id the
  orchestrator never issued): tool returns a structured
  error; the agent should start a fresh thread.
- **Thread turn cap exceeded** (>5 turns without
  resolution): the tool still returns the user's reply; the
  orchestrator emits a `Diagnostic::Warning` and includes
  `thread_turn_index >= 5` in the response. Agent's
  convention is to close the thread on the next call.
- **Mode flip refused** (operator has disabled mode flips
  via a config flag): the tool returns an error rather than
  blocking forever; agent records a TBD.

### What ask_user is NOT

- **Not a chat channel.** The agent shouldn't use `ask_user`
  for casual conversation, status updates, or to narrate
  what it's doing. Prompts explicitly forbid this.
- **Not unbounded back-and-forth.** Threading via `thread_id`
  is for clarifying genuinely ambiguous answers, not for
  iterative design exploration. The agent closes a thread
  after a small number of exchanges (target: ≤ 3; warning at
  5; the orchestrator flags but does not force-close beyond
  that).
- **Not a retrieval tool.** Calling `ask_user` to look up
  framework documentation is a prompt violation. The agent
  has `api_*` tools for that.

## 4.6 Sync / Async Bridge

The orchestrator's tool dispatch is synchronous (`tool.invoke(...)`
returns directly). LanceDB's Rust API is async. The three new
tools wrap their async calls in a per-orchestrator tokio
runtime:

```
struct RetrievalRuntime {
    rt: tokio::runtime::Runtime,
}

impl RetrievalRuntime {
    fn new() -> Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(Self { rt })
    }

    fn block_on<F: Future<Output = T>, T>(&self, f: F) -> T {
        self.rt.block_on(f)
    }
}
```

The runtime is constructed once at orchestrator startup (lazy:
on first use of any retrieval tool) and lives for the
orchestrator's lifetime. A single-threaded current-thread
runtime suffices — the workload is small bursts of async I/O
(embedder HTTP call + lance read), not concurrent throughput.

The runtime is owned by a `RetrievalService` struct that:

- Holds the runtime handle.
- Holds open Lance dataset handles (`framework_chunks`,
  `spec_chunks`, `signal_table_rows`, `cross_spec_refs`).
- Holds the embedder client (Chapter 5).
- Exposes synchronous methods (`semantic_search_framework`,
  `semantic_search_spec`, `query_signal_table`) that
  `block_on` their async implementations.

Each tool implementation (`ApiSemanticSearchTool`, etc.) holds
an `Arc<RetrievalService>` and forwards calls.

## 4.7 Cold-Start Behavior

Three cold-start concerns:

- **Lance dataset open**: microseconds. Negligible.
- **Embedder warm**: for Ollama on M5 Max, the first call after
  laptop wake / Ollama restart loads the model into unified
  memory; can take 5-15 seconds. For vLLM-on-A100, first call
  is fast (~hundreds of ms). For hosted APIs (OpenAI, Voyage),
  first call latency is network-bound (~100-300ms).
- **Vector index warm-up**: Lance's IVF_FLAT loads the
  centroids on first query; subsequent queries reuse.

The first retrieval-tool call per orchestrator session
absorbs the cold-start cost. The chat panel surfaces a
"warming retrieval index..." status during the first call so
the user knows the delay is expected and not a hang. Subsequent
calls are sub-second on a warm M5 Max + Ollama setup.

## 4.8 Caching

Each call to a retrieval tool is independent — no cross-call
caching in v1. Two reasons:

- The agent's queries vary substantively turn-to-turn; cache
  hit rate would be low.
- LRU caching of query→results introduces correctness risk
  when the index changes underfoot (e.g. mid-session
  re-index).

Embedder caching IS performed at the embedder client (Chapter
5): identical query strings produce identical vectors for the
duration of a session.

## 4.9 Tool Registration

The four tools register in the universal tool catalog at
[`src/__internal/steps/mod.rs`](../../src/__internal/steps/mod.rs):

```
"read_file",
"list_dir",
"write_file",
"edit_file",
"delete_file",
"search",
"run_cargo",
"declare_fix",
"declare_hypothesis",
"log_bug",
"resolve_bug",
"record_run",
"api_search",
"api_hover",
"api_impls",
"api_references",
"api_expand_macro",
"api_semantic_search",      -- NEW
"spec_semantic_search",     -- NEW
"signal_table_query",       -- NEW
"ask_user",                 -- NEW
```

The catalog stays universal (the prior decision to remove
per-step gating stands). Steps with high retrieval relevance
(DM0, DM1, DM2*, DM3a, DM3b) get prompt nudges; steps with no
retrieval need (e.g. DM4*) see the tools advertised but rarely
called.

## 4.10 Observability

Each retrieval-tool call records a metrics event:

```
tracing::info!(
    target = "sim_flow::metrics",
    event = "retrieval_call",
    tool = "spec_semantic_search",
    elapsed_ms = ...,
    k_requested = ...,
    k_returned = ...,
    embedder_elapsed_ms = ...,
    lance_elapsed_ms = ...,
);
```

This feeds the existing `metrics.jsonl` stream from the
robustness study (Chapter 1 of the brainstorm collection), so
retrieval-tool effectiveness can be measured alongside other
turn-level metrics (e.g. did `api_semantic_search` reduce the
invented-API rate on rgb_toy DM2d replay?).

`ask_user` calls record a parallel event:

```
tracing::info!(
    target = "sim_flow::metrics",
    event = "ask_user_call",
    step = "DM2d",
    kind = "free-form" | "yes-no" | "choice" | "value",
    mode_before = "auto" | "manual",
    mode_after = "manual",
    record_as = "open-question",
    thread_id = "ask-DM2d-...",
    thread_turn_index = 0,
    thread_status = "open" | "closed" | "cancelled" | "thread-cancelled",
    user_wait_ms = ...,
    answer_length = ...,
    cancelled = false,
);
```

Plus a thread-close summary at thread resolution:

```
tracing::info!(
    target = "sim_flow::metrics",
    event = "ask_user_thread_closed",
    step = "DM2d",
    thread_id = "ask-DM2d-...",
    turn_count = 3,
    closed_as = "auto-decision" | "open-question" | "cancelled" | "abandoned",
    total_user_wait_ms = ...,
);
```

Together these surface "when does the agent get stuck",
"how often does automated mode flip to manual", and
"how many turns of clarification do users need" as
first-class signals.

## 4.11 What This Chapter Does Not Specify

- The wire-format details of native tool-call dispatch (covered
  by the existing tool-call infrastructure).
- The exact byte-level cap on snippet length (configurable;
  default 500 chars).
- The exact prompt text added to step prompts beyond the
  representative nudges shown above. Final wording is an
  implementation-plan concern.
- The CLI for ad-hoc query (e.g. `sim-flow query
  spec-semantic "..."`). Useful for debugging; an
  implementation-plan add-on, not load-bearing.
- Eviction / cache strategies beyond §4.7. Future revision if
  measured.
