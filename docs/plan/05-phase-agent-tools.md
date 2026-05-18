# Phase 5: Agent Tools

## Goal

Implement the `RetrievalService` (sync/async bridge + held
state), the three retrieval tools (`api_semantic_search`,
`spec_semantic_search`, `signal_table_query`), and the
user-interaction tool `ask_user` (with turn-boundary
scheduling, suspend/resume protocol, and auto→manual
step-mode flip). Register all four tools in the universal tool
catalog, wire JSON-schema argument definitions for native
function-call dispatch, and add observability metrics.

## Inputs

- Architecture Chapter 4 (full — three retrieval tools plus
  `ask_user`).
- Architecture Chapter 6 §6.5 (`ask_user` integration
  semantics: turn-boundary discipline, mode flip,
  persistence).
- Phase 3 (embedder).
- Phase 4 (Lance tables and query API).

## Outputs

- New module `src/__internal/session/retrieval/`.
- New tool implementations under
  `src/__internal/session/tools/`:
  - `api_semantic_search.rs`
  - `spec_semantic_search.rs`
  - `signal_table_query.rs`
  - `ask_user.rs`
- New orchestrator integration for `ask_user`
  suspend/resume in `src/__internal/session/orchestrator/`
  (or wherever the work-session loop is dispatched from).
- Tool registration in the universal catalog.
- Observability metric events for retrieval calls AND
  `ask_user` calls (including mode-flip events).
- Unit + integration tests.

## Acceptance Gate

- [ ] `cargo build --package sim-flow` succeeds.
- [ ] `cargo test --package sim-flow retrieval::` passes.
- [ ] `cargo test --package sim-flow tools::api_semantic_search::`
      passes.
- [ ] `cargo test --package sim-flow tools::spec_semantic_search::`
      passes.
- [ ] `cargo test --package sim-flow tools::signal_table_query::`
      passes.
- [ ] `cargo test --package sim-flow tools::ask_user::`
      passes (suspend/resume protocol, mode flip, thread
      registry, persistence).
- [ ] `cargo test --package sim-flow --test
      retrieval_integration` passes against synthetic
      fixtures.
- [ ] `cargo test --package sim-flow --test
      ask_user_integration` passes with a scripted mock host
      driving the suspend/resume cycle, including multi-turn
      chained threads, thread cancellation, force-close on
      sub-session end, turn-cap warning, and interleaved
      threads.

## Milestones

### Milestone 5.1: RetrievalService scaffold

- [x] Create `src/__internal/session/retrieval/mod.rs` and
      `service.rs`.
- [x] Define `RetrievalService` struct holding:
  - A `tokio::runtime::Runtime` (current-thread).
  - An `Arc<dyn EmbeddingClient>` (Phase 3).
  - An optional framework `LanceConnection` (Phase 4) —
    `None` when the framework index isn't built.
  - An optional spec `LanceConnection`.
- [x] Implement `RetrievalService::new(project_root: &Path)
      -> Result<RetrievalService>`:
  - Construct the runtime.
  - Construct the embedder from `embedder.toml`.
  - Try to open the framework and spec connections; missing
    connections are not errors (just None).
- [x] Implement synchronous query wrappers that `block_on`
      the Phase 4 async query functions:
  - `semantic_search_framework_sync(...)`
  - `semantic_search_spec_sync(...)`
  - `query_signal_table_sync(...)`
- [x] Unit test: construct against a fixture project with no
      indexes built; service constructs cleanly with None
      connections.

Gate: service unit tests pass.

### Milestone 5.2: `api_semantic_search` tool

- [x] Create `tools/api_semantic_search.rs`.
- [x] Implement `ApiSemanticSearchTool` matching the existing
      tool-trait pattern (study `tools/api_search.rs` for the
      pattern).
- [x] Define the JSON-schema for arguments per Architecture
      §4.2 (query / k / kind).
- [x] Implement `dispatch(args: &ToolArgs, ctx: &ToolCtx) ->
      ToolResult`:
  - Resolve the `RetrievalService` from the orchestrator
    context.
  - Refuse with a structured error if the framework index
    is missing.
  - Embed the query via the service's embedder.
  - Call `semantic_search_framework_sync` with the embedding.
  - Construct the return JSON per §4.2 (hits, embedder_used,
    elapsed_ms).
- [x] Unit test with a mock RetrievalService:
  - Tool returns expected hits for a fixture index.
  - Tool returns the "index missing" error when no
    framework connection.
  - Tool propagates the embedder dimension-mismatch error
    cleanly.

Gate: api_semantic_search unit tests pass.

### Milestone 5.3: `spec_semantic_search` tool

- [x] Create `tools/spec_semantic_search.rs` mirroring 5.2's
      shape.
- [x] JSON-schema per Architecture §4.3 (query / k / source /
      kind).
- [x] Implement dispatch: embed query, call
      `semantic_search_spec_sync`, augment results with
      `chunk_path` (absolute path to the chunk md file) and
      `contained_signal_tables` / `contained_figures` from
      the chunk's front matter.
- [x] Return-shape per §4.3.
- [x] Special-case the no-source-spec case: return
      `{"hits": [], "note": "no source spec registered"}`
      rather than an error.
- [x] Unit tests.

Gate: spec_semantic_search unit tests pass.

### Milestone 5.4: `signal_table_query` tool

- [x] Create `tools/signal_table_query.rs`.
- [x] JSON-schema per Architecture §4.4 (filter object /
      conflicts_only / limit).
- [x] Implement dispatch: call `query_signal_table_sync` for
      regular queries; call `find_signal_conflicts_sync` when
      `conflicts_only=true`.
- [x] Return-shape per §4.4 with `conflict_pairs` field
      populated in conflicts mode.
- [x] Validate filter object schema (no extra keys).
- [x] Unit tests covering: filter by signal_name; filter by
      stage; conflicts_only; limit truncation.

Gate: signal_table_query unit tests pass.

### Milestone 5.5: ask_user -- suspend/resume protocol

This milestone implements the orchestrator-side machinery
`ask_user` depends on, BEFORE the tool itself is implemented.
The tool needs a way to pause a work session at a tool-call
boundary, surface a question, await a user reply, and resume
the agent's LLM turn with the reply threaded back as the
tool result.

- [ ] Identify the work-session dispatch site (likely under
      `src/__internal/session/orchestrator/` or
      `session/auto.rs`).
- [ ] Define an internal `PendingUserAsk` struct holding:
      `question`, `context`, `kind`, `choices`, `default`,
      `record_as`, `tool_call_id` (so the resume can return
      the answer in the tool-result frame matching the
      paused call), `triggered_at`, `step_mode_before`,
      `thread_id` (either passed in by the agent or generated
      by the orchestrator for fresh threads), `thread_turn_index`.
- [ ] Add a `pending_user_ask: Option<PendingUserAsk>` field
      to the work-session state.
- [ ] Implement `WorkSession::suspend_for_user_ask(ask:
      PendingUserAsk)`: stash the ask, emit
      `RequestUserInput` (with `prompt = question`,
      `placeholder` = kind hint, `followups` = `choices`
      when kind=choice), persist a checkpoint to
      `.sim-flow/<step>/pending-ask.toml` so a reload can
      recover.
- [ ] Implement `WorkSession::resume_from_user_ask(reply:
      UserMessage) -> AskUserAnswer`: package the reply,
      clear the pending state, return.
- [ ] Implement reload-recovery: at session startup, if
      `pending-ask.toml` exists, restore the pending state
      and wait for the next `UserMessage`.
- [ ] Implement thread management:
  - `ThreadRegistry` holding open threads keyed by
    `thread_id`. Each thread carries: turn count, accumulated
    Q+A history, time-of-first-call, current `step` /
    sub-session reference.
  - `ThreadRegistry::open_or_continue(ask: &PendingUserAsk)
    -> Result<ThreadHandle>`: if `ask.thread_id` is set,
    look up the existing thread (error if unknown); else
    generate a new `thread_id` and create a new entry.
  - `ThreadRegistry::record_turn(thread_id, q, a)`:
    appends to the thread's history; emits the
    `ask_user_call` metric event with `thread_turn_index`.
  - `ThreadRegistry::close_thread(thread_id, closed_as) ->
    ResolvedThread`: marks closed, returns the resolved
    payload for persistence; emits the
    `ask_user_thread_closed` metric event.
  - `ThreadRegistry::force_close_all_on_subsession_end()`:
    invoked from the sub-session-end hook; closes any open
    threads per Architecture §6.5.5's force-close policy.
  - Persist open-thread state to
    `.sim-flow/<step>/ask-threads/<thread_id>.toml` after
    each turn so a reload mid-thread recovers.
- [ ] Unit tests: suspend then resume cycle; reload from a
      pending checkpoint; multiple-ask serialization (one at
      a time); thread creation, continuation, and close;
      reload mid-thread recovers turn history; force-close
      on sub-session end.

Gate: suspend/resume + thread-management protocol unit tests
pass.

### Milestone 5.6: ask_user -- step-mode flip

- [ ] Implement `flip_step_mode_for_ask_user(state: &mut
      FlowState) -> ModeFlipOutcome`: when
      `state.current_step_mode == auto`, set to `manual`,
      persist `state.toml`, emit `StepModeChanged` event,
      emit `Diagnostic::Info` with the message from Chapter
      6 §6.5.2. When already `manual`, return
      `ModeFlipOutcome::NoChange`.
- [ ] The function is idempotent and safe to call from
      either the tool dispatch or the orchestrator's
      `ask_user` interception path.
- [ ] Unit tests: auto-to-manual flip persists +
      emits event; manual-to-manual is a no-op; reload after
      a flip sees the persisted manual mode.

Gate: mode-flip unit tests pass.

### Milestone 5.7: ask_user -- tool implementation

- [ ] Create `tools/ask_user.rs`.
- [ ] Implement `AskUserTool` matching the existing tool
      trait.
- [ ] Define the JSON schema per Architecture §4.5,
      including `thread_id` as an optional argument.
- [ ] Implement `dispatch(args, ctx)`:
  - Validate args (kind=choice requires choices; an
    explicit `thread_id` must reference an open thread or
    return a structured error).
  - Call `flip_step_mode_for_ask_user` (milestone 5.6) on
    the FIRST turn of a thread (intermediate turns do not
    re-flip; once manual, stay manual).
  - Look up or open the thread via
    `ThreadRegistry::open_or_continue` (milestone 5.5).
  - Construct `PendingUserAsk` with thread fields populated
    and call `WorkSession::suspend_for_user_ask` (milestone
    5.5).
  - Return a special `ToolDispatch::Suspended` variant
    rather than a normal result; the orchestrator's dispatch
    loop recognizes this and exits the current LLM turn
    cleanly after this tool call (discarding any subsequent
    tool calls in the same model response with a
    `tool_calls_after_ask_user` warning per Architecture
    §6.5.1).
- [ ] After the user replies, the orchestrator's
      resume-from-user-ask path:
  - Calls `ThreadRegistry::record_turn` with the Q+A.
  - If `record_as` on the suspended call was NOT `"none"`,
    invokes `ThreadRegistry::close_thread` and triggers
    persistence (milestone 5.8).
  - Builds the `AskUserAnswer` including the generated
    `thread_id` and current `thread_turn_index`.
  - Threads it into the agent's next-turn tool-result
    stream. The tool itself is not re-invoked on resume.
- [ ] Cancellation handling: if the user's reply is the
      `/cancel` or `/cancel-thread` command, populate
      `cancelled` / `thread_cancelled` fields and (for
      thread cancel) close the thread with `closed_as =
      "thread-cancelled"` and persist as unresolved.
- [ ] Turn-cap behavior: when `thread_turn_index >= 5` on
      a new turn in an open thread, emit
      `Diagnostic::Warning` per Architecture §4.5 chaining
      section.
- [ ] Unit tests: dispatch flow with a scripted host;
      validation errors; auto-mode flip side effect;
      fresh-thread creation; thread continuation;
      intermediate vs closing call persistence behavior;
      cancel and cancel-thread paths; turn-cap warning.

Gate: ask_user tool unit tests pass.

### Milestone 5.8: ask_user -- persistence (thread-aware)

- [ ] Implement
      `persist_resolved_thread(spec_md_path: &Path,
      resolved: &ResolvedThread) -> Result<String>` that
      writes a SINGLE entry to spec.md from a closed thread
      (intermediate turns are NOT individually persisted —
      they live only in the chat panel and metrics):
  - `record_as = "open-question"`: append one resolved-or-
    unresolved Open Question entry. Body = the original
    question (turn 0); resolution = the final answer (or
    "unresolved" if the thread was cancelled).
  - `record_as = "auto-decision"`: append one Auto-decision
    row. `decision` = synthesized final answer;
    `rationale` = user's wording at thread close, plus an
    annotation `(arrived at through N rounds of
    clarification)` when N > 1.
  - `record_as = "none"`: should not be invoked on closed
    threads (closing requires a non-none record_as);
    defensive panic / debug-assert.
- [ ] Implement multi-turn coalescing: for threads with >1
      turn, write only the resolved form (original question
      + final answer), NOT the intermediate turns.
      Intermediate turns are preserved in the metrics log
      (`metrics.jsonl`) for audit.
- [ ] When `spec.md` does not exist yet (DM0 in progress),
      append the resolved entry to a session buffer at
      `.sim-flow/spec-ingest/qa-buffer.toml`. Each entry in
      the buffer is one resolved thread.
- [ ] Returns the anchor string for the `recorded_at` field
      of the `AskUserAnswer` returned at thread close.
- [ ] Unit tests:
  - Single-turn thread with each `record_as` value
    persists correctly.
  - Multi-turn thread (3 turns) coalesces to ONE entry
    with the annotation.
  - Cancelled thread persists as unresolved Open Question
    with the "user cancelled clarification" body.
  - "spec.md doesn't exist" branch writes to the buffer.

Gate: persistence unit tests pass.

### Milestone 5.9: Tool registration in universal catalog

- [ ] Edit `src/__internal/session/tools/mod.rs`:
  - Add `mod api_semantic_search;`,
    `mod spec_semantic_search;`, `mod signal_table_query;`,
    `mod ask_user;`.
  - Re-export `ApiSemanticSearchTool`,
    `SpecSemanticSearchTool`, `SignalTableQueryTool`,
    `AskUserTool`.
  - Add the four to the universal tool catalog list (the
    array used by `for_step`) per Architecture §4.9.
- [ ] Edit `src/__internal/steps/mod.rs`:
  - Add the four names to the `UNIVERSAL_TOOL_NAMES`
    constant (or equivalent) so the prompt-side catalog
    advertisement includes them.
- [ ] Unit test: enumerate the universal tool catalog and
      assert all four new tools are present.

Gate: catalog test passes.

### Milestone 5.10: Native function-call schema export

- [ ] For each new tool (all four), export its JSON-schema
      in the shape the existing `ToolAdvertise`
      infrastructure expects.
- [ ] Add a unit test that constructs the tool catalog,
      serializes each tool's advertise shape to JSON, and
      asserts the JSON conforms to the per-tool schema in
      Architecture §4.2 / §4.3 / §4.4 / §4.5.

Gate: advertise-shape test passes.

### Milestone 5.11: Observability metrics

- [ ] In each retrieval tool's dispatch, emit a
      `tracing::info!` event with `target =
      "sim_flow::metrics"`, `event = "retrieval_call"`, plus
      the fields per Architecture §4.10.
- [ ] In `ask_user`'s dispatch, emit `event =
      "ask_user_call"` per Architecture §4.10, including
      `mode_before`, `mode_after`, `record_as`,
      `user_wait_ms`, `answer_length`, `cancelled`.
- [ ] Verify the metrics event fields land in the existing
      `metrics.jsonl` capture flow (sanity-test by setting
      `SIM_FLOW_CAPTURE_METRICS=<tmp>` and asserting the
      file is written).

Gate: metrics-emission unit tests pass for both event kinds.

### Milestone 5.12: Cold-start UX surfacing

- [ ] On the first retrieval-tool call within an orchestrator
      session, emit a `Diagnostic::Info`
      with message `"warming retrieval index (first call may
      take 5-15s on cold embedder)"` BEFORE the dispatch
      begins. Subsequent calls do not emit this.
- [ ] Unit test: first call emits the diagnostic; second
      call does not.

Gate: cold-start unit test passes.

### Milestone 5.13: Retrieval integration test with synthetic fixtures

- [ ] Create `tests/retrieval_integration.rs` using the
      synthetic fixtures from Phase 4 (milestone 4.15).
- [ ] Build the framework + spec indexes against the
      fixtures.
- [ ] Construct a `RetrievalService` and invoke each tool
      directly:
  - `api_semantic_search` with a known-good query that
    should hit a specific fixture symbol.
  - `spec_semantic_search` with a known-good query that
    should hit a specific fixture chunk.
  - `signal_table_query` with a filter that should return
    exactly one row.
  - `signal_table_query` with `conflicts_only=true` against
    a fixture with a deliberate spec-md vs source-spec
    conflict; assert the conflict is detected.
- [ ] Run under both mock embedder (`cargo test`) and live
      Ollama (`SIM_FLOW_E2E_LIVE=1 cargo test`).

Gate: retrieval integration tests pass.

### Milestone 5.14: ask_user integration test

- [ ] Create `tests/ask_user_integration.rs` using a mock
      host (`AutoPresenter`) that scripts user replies.
- [ ] Test 1 (manual mode, single-turn thread): a mock work
      session emits an `ask_user` call (no `thread_id`); the
      test asserts the `RequestUserInput` event is emitted,
      the work session suspends, the scripted reply
      arrives, the tool result includes a generated
      `thread_id` with `thread_turn_index = 0`, and
      `record_as = "open-question"` results in the Q+A
      being persisted to a fixture spec.md as a single
      Open Question entry.
- [ ] Test 2 (auto mode, single-turn thread): same flow but
      starting in `step_mode = auto`; assert the
      `StepModeChanged { mode: manual }` event fires before
      the user-input is surfaced, and
      `state.toml.current_step_mode` is now `manual`.
- [ ] Test 3 (reload mid-suspend): simulate an orchestrator
      shutdown after `ask_user` has suspended but before
      the user replies; restart the session; verify the
      pending-ask state and the thread state are recovered
      from `.sim-flow/<step>/pending-ask.toml` and
      `.sim-flow/<step>/ask-threads/<thread_id>.toml`, and
      that the session resumes correctly when the reply
      arrives.
- [ ] Test 4 (tool calls after ask_user): an LLM response
      emitting `ask_user` followed by another tool call;
      verify the second call is discarded and a
      `tool_calls_after_ask_user` warning is emitted.
- [ ] Test 5 (chained thread, 3 turns, auto-decision close):
      first call is fresh (no thread_id); reply is
      ambiguous ("probably 4"); second call passes the
      returned `thread_id` with `record_as = "none"` and
      a clarifying question; user replies "yes, 4";
      third call passes the same `thread_id` with
      `record_as = "auto-decision"` and a confirmation
      question; user accepts. Assert:
  - All three `ask_user_call` metric events fire with
    monotonic `thread_turn_index` (0, 1, 2).
  - Only one row is written to spec.md's Auto-decisions
    section after the third call.
  - The Auto-decision row includes the "(arrived at
    through 3 rounds of clarification)" annotation.
  - An `ask_user_thread_closed` metric event fires with
    `turn_count = 3`, `closed_as = "auto-decision"`.
- [ ] Test 6 (mid-thread cancel via `/cancel-thread`):
      first two calls succeed; on the third, the user
      types `/cancel-thread`. Assert:
  - The tool result has `thread_cancelled = true,
    cancelled = true, answer = ""`.
  - spec.md gets one unresolved Open Question with the
    "User cancelled clarification after N exchanges" body.
  - The thread is removed from the open-thread registry.
- [ ] Test 7 (force-close on sub-session end): an open
      thread exists when the sub-session terminates.
      Assert the orchestrator's sub-session-end hook
      force-closes the thread per Architecture §6.5.5 (one
      resolved Open Question if any answer recorded; drop
      silently if no answer recorded).
- [ ] Test 8 (turn-cap warning at 5): emit 5 ask_user
      calls in the same thread without closing. Assert
      `Diagnostic::Warning` fires on the 5th turn.
- [ ] Test 9 (interleaved threads): the agent emits
      `ask_user` calls with two DIFFERENT `thread_id`s in
      adjacent turns. Assert both threads remain open
      independently, the chat panel rendering distinguishes
      them, and closing one doesn't affect the other.

Gate: ask_user integration tests pass.

### Milestone 5.15: Live end-to-end smoke

- [ ] Manual: build framework index against real
      `crates/framework`; build a spec index against a real
      ingested RV12 project; issue tool calls from a small
      driver script that exercises each tool.
- [ ] Confirm latency: each call returns in under 2 seconds
      on M5 Max with Ollama (after the cold start has
      completed). Record latencies in
      `tests/fixtures/retrieval-snapshots/README.md`.

Gate: manual verification; recorded.

## Out of Scope (deferred to later phases)

- **Step-prompt nudges referencing the new tools.** Phases 6
  and 7 own this.
- **DM0 / DMx automatic gating on tool usage** (e.g. "the
  agent MUST call api_semantic_search before writing a
  framework symbol"). v1 leaves this to prompt nudges; gate
  enforcement is deferred.
- **Per-step tool gating** (re-introducing the per-step
  catalog gating that was previously removed). Out of scope;
  universal catalog stays universal.
- **Query result caching across tool calls.** Out of scope
  per Architecture §4.7.
- **Cross-session prompt-cache for retrieval responses.** Out
  of scope.
