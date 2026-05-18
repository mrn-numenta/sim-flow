# Chapter 6: DMF Flow Integration

This chapter specifies how the DMF flow (DM0 through DM5) and
its critique passes integrate with the new components: the
spec ingest pipeline (Chapter 1), the structured spec.md schema
(Chapter 2), the LanceDB index (Chapter 3), the three new
agent tools (Chapter 4 — three retrieval tools plus `ask_user`),
and the rig-backed embedder
(Chapter 5).

The orchestrator core does not change (Axiom 1 of
[architecture.md](architecture.md)). What changes is the
inputs the steps consume, the artifacts they produce, the
tools they reach for, and the validation the gates run.

## 6.1 End-to-End Flow Overview

The new lifecycle of a project:

```
1. sim-flow new model <project>
2. (optional) sim-flow ingest --source <path-to-source-spec> --project <project>
       -> populates .sim-flow/spec-ingest/
3. (optional) sim-flow build-spec-index --project <project>
       -> populates .sim-flow/lance-index/
4. sim-flow auto --project <project>
       -> DMF runs: DM0 (author spec.md) -> DM1 -> ... -> DM5
```

Steps 2 and 3 are optional only because of the no-source-spec
case (§6.4). When a source spec is provided, ingest + index are
prerequisites for DM0; the orchestrator runs them
automatically if the user invoked `auto` without them.

## 6.2 DM0 (Specification) — Changes

### 6.2.1 Inputs

DM0 receives, in addition to today's inputs:

- The ingest manifest at
  `.sim-flow/spec-ingest/manifest.toml` (or `source_kind = "none"`
  marker).
- The corpus of chunked, classified source-spec content under
  `.sim-flow/spec-ingest/`.
- The lance index at `.sim-flow/lance-index/` (or empty in the
  no-source case).
- The new template at
  `templates/model-project/docs/spec.md.tmpl`.

### 6.2.2 Two Modes

DM0 detects which mode to enter from
`manifest.toml.source_kind`:

- **`pdf | markdown | text`**: Source-driven mode (§6.3).
- **`none`**: Interactive authoring mode (§6.4).

### 6.2.3 Outputs

DM0 produces:

- `docs/spec.md` conforming to Chapter 2's schema.
- Updated `Open Questions` and `Auto-decisions` sections.
- (Side effect) Updated `.sim-flow/lance-index/` after every
  spec.md write (incremental refresh, see Chapter 3 §3.10
  case 2).

### 6.2.4 Gate Check

The DM0 gate now validates spec.md against Chapter 2's schema
rather than the old regex pattern:

1. **Parse cleanly**. Errors abort the gate with the parser's
   line/column.
2. **REQUIRED sections present** (Chapter 2 §2.2).
3. **Required quantitative rows present** in `Assumptions and
   Constraints`: `Clock frequency` matching `\d+\s*(MHz|GHz)`,
   `Gate budget per cycle` matching `\d+`. Today's regex
   pattern, applied against the structured table cell.
4. **Cross-references resolve** (Chapter 2 §2.6).
5. **Auto-decisions populated** in automated mode.
6. **Source-spec anchors resolve** to chunks in the manifest
   (or are explicitly marked "no anchor; user-provided" for
   the no-source case).

Warnings (don't block):

- Use of alias column names.
- Empty `Behavior summary` on a block.
- Empty `Caption` on a figure.

## 6.3 DM0 Source-Driven Mode

The flow when a source spec is registered.

### Step A: Auto-populate from structured ingest output

The orchestrator pre-fills spec.md from deterministic ingest
artifacts, BEFORE the LLM dispatch starts:

- `Metadata.source_documents` ← `manifest.toml.peers[]` plus
  the primary source.
- `Assumptions and Constraints` ← any quantitative rows the
  pipeline detected (clock frequency, technology node, etc.,
  surfaced via §2.3.3's source-anchor entries).
- `Parameters` ← `tables/parameters/*.toml`.
- `External Interfaces` and `Blocks` ← seeded from
  `tables/signals/*.toml`, one block per signal table with
  `stage` as the block name. Behavior summaries are empty;
  source anchors are populated.
- `Encodings` ← `tables/encodings/*.toml`.
- `Error Handling` ← `tables/errors/*.toml`.
- `State Machines` ← `tables/fsms/*.toml`.
- `Figures` ← one entry per figure raster, with the source
  page reference; captions empty.
- `Source-Spec Anchors` ← built from the above.
- `Open Questions` ← every `tbds.toml` entry becomes a
  candidate open question, formatted with breadcrumb context.

After auto-population the LLM sees a spec.md that is
**structurally complete** (all REQUIRED sections present) but
**semantically incomplete** (prose subsections empty, behavior
summaries empty, some Open Questions not yet decided).

### Step B: LLM-driven completion

The work-session prompt for DM0 directs the agent to:

1. Validate the auto-populated structured tables against the
   source spec (using `spec_semantic_search` to retrieve the
   relevant chunks, and `read_file` on each chunk's
   `chunk_path` to confirm).
2. Fill empty prose subsections (`Purpose`, `Scope`,
   `Non-goals`, per-block `Behavior summary`, `Functional
   Behavior > End-to-end behavior`, etc.) by reading the
   source-spec chunks and writing concise normalizations.
   Anchors in the table cells direct the agent to which chunks
   to read.
3. Resolve `Open Questions` either by answering from the
   source spec (when a TBD has been answered elsewhere) or by
   recording an Auto-decision (automated mode) or asking the
   user (manual mode).
4. Emit a single `write_file` call to `docs/spec.md` with the
   completed text.

The agent uses `signal_table_query` to spot-check that the
spec.md signal tables it just wrote match what's in the
source spec. The critique pass also does this; this is a
belt-and-suspenders check.

### Step C: Critique

The DM0 critique pass runs (today's existing critique
infrastructure) against the new structured spec.md.
Specifically:

- Parse spec.md via the Chapter 2 parser.
- Verify every signal-table row in spec.md has a corresponding
  source-spec row (or an Auto-decision explaining a
  deliberate divergence).
- Verify every Block in spec.md has a `Behavior summary` of at
  least N characters (configurable; default 50).
- Verify Open Questions have either been answered (moved to
  Auto-decisions) or remain genuinely open.
- Use `spec_semantic_search` to surface anchors the agent may
  have missed.

Findings are written to `docs/critiques/DM0-critique.json` in
the existing format; the gate engine reads it as today.

## 6.4 DM0 Interactive Authoring Mode (No Source Spec)

The flow when `manifest.toml.source_kind = "none"`.

### Step A: Required-Field Traversal

The orchestrator computes the list of `MissingField` entries
by walking Chapter 2 §2.7's traversal against the empty
template. Output is an ordered list:

```
MissingField {
  section_path: "Metadata.design_name",
  prompt_template: "What is the design's name?",
  kind: Scalar,
}
MissingField {
  section_path: "Metadata.version",
  prompt_template: "What is the design version?",
  kind: Scalar,
}
MissingField {
  section_path: "Purpose",
  prompt_template: "Describe what the design does in one or two short paragraphs.",
  kind: Prose,
}
MissingField {
  section_path: "Assumptions.Quantitative.clock_frequency",
  prompt_template: "What is the target clock frequency? (e.g. 1 GHz, 500 MHz)",
  kind: ConstrainedScalar(regex = "\\d+\\s*(MHz|GHz)"),
}
...
```

Required fields are flagged required; optional sections start
with an "is this section applicable?" question before drilling
in.

### Step B: Q&A Loop

DM0 in this mode operates as a structured interview built on
the `ask_user` primitive (Chapter 4 §4.5 and §6.5.3). The DM0
loop driver does NOT implement its own user-prompting
machinery; every question goes through `ask_user`.

For each `MissingField` in traversal order:

1. The agent constructs an `ask_user` invocation:
   - `question` = the field's `prompt_template`.
   - `context` = which section is being filled and why.
   - `kind` = mapped from `MissingFieldKind` (`Scalar` →
     `free-form`, `ConstrainedScalar` → `value` with default
     suggestion, `TableRow` → `free-form` with row schema in
     context, etc.).
   - `choices` populated when applicable.
   - `record_as` = `"auto-decision"` (DM0 is authoring spec
     from scratch; every answer is by definition an
     architectural decision worth persisting).
2. The orchestrator's `ask_user` dispatch surfaces the
   question and waits for the user.
3. The user's answer arrives as the tool result.
4. The agent validates the answer against the field's `kind`
   (regex match for `ConstrainedScalar`, non-empty for
   `Scalar`, etc.). If invalid, the agent emits another
   `ask_user` call with a clarifying re-prompt.
5. On valid input, the agent commits the answer to the
   in-memory `SpecMd` struct and advances to the next field.

After every N fields (configurable; default 5), the agent
performs a `write_file` of the current `SpecMd` state to
`docs/spec.md` so progress is durable across reloads.

In automated mode this loop cannot run as-is: the first
`ask_user` call flips step-mode to manual (per §6.5.2), at
which point the loop continues in manual mode for the rest of
DM0. This is the desired behavior — DM0 with no source spec
fundamentally requires human input, so "automated mode for
spec authoring" is an inapplicable combination that the flip
makes explicit.

For OPTIONAL sections, the agent asks "Does this design have
[Memory Map / FSMs / Encodings / Connectivity / Error Handling]?
(yes/no/skip)" — yes drills in, no records "not applicable" in
that section's anchor map, skip defers to a later pass.

### Step C: Worked Examples Are Special

Worked Examples (§2.3.18) are too free-form for one-question-
at-a-time. The agent treats them as a single conversational
turn: "Walk me through what should happen for one or two
representative input scenarios. I'll record your description as
the worked example." User free-types; agent normalizes to
Chapter 2 §2.3.18 format.

### Step D: Critique

DM0 critique in this mode runs as today, but the spec.md it
reviews was authored from user dictation rather than from
source-spec chunks. The critique looks for:

- Internal consistency (signal tables agree across Blocks).
- Quantitative claims with no anchor (acceptable in this mode;
  recorded as such).
- Open Questions that should be auto-decisions (or vice
  versa).

## 6.5 ask_user Integration

The `ask_user` tool (Chapter 4 §4.5) is available to the agent
on every step. Three integration concerns specific to the DMF
flow:

### 6.5.1 Turn-Boundary Discipline

`ask_user` is a turn-boundary tool. The orchestrator's
contract:

- The agent emits `ask_user` only when forward progress
  requires the answer. If the agent can complete other useful
  tool calls (partial-artifact writes, hypothesis declarations,
  context reads) in the same turn, it MUST do those first and
  emit `ask_user` as the LAST tool call of the turn.
- The orchestrator's dispatcher processes tool calls in the
  model's emitted order. When it encounters `ask_user`, it
  suspends the work session AFTER executing earlier calls in
  the same turn and BEFORE dispatching any subsequent calls.
  Subsequent calls in the same model response are discarded;
  the orchestrator records a `tool_calls_after_ask_user`
  warning so prompt-side regressions surface.
- The next LLM turn begins with the `ask_user` tool result
  (the user's answer) threaded into the tool-result stream as
  if it were a synchronous return. The agent resumes with the
  answer in working memory.

### 6.5.2 Automated-Mode Flip

When `step_mode = auto` and the agent emits `ask_user`:

1. The orchestrator persists `state.toml.current_step_mode =
   manual`.
2. Emits a `StepModeChanged { mode: manual }` event so the
   chat panel updates its mode indicator.
3. Emits a `Diagnostic::Info` with message
   `"ask_user invoked during auto run; flipping to manual
   mode. Re-enable auto via the chat panel toggle when
   ready."` so the user understands why the mode changed.
4. Surfaces the question via `RequestUserInput` per Chapter 4
   §4.5 step 3.

After the user answers, the work session resumes in manual
mode. Subsequent steps STAY in manual mode until the user
explicitly flips back via the chat panel's mode toggle. The
orchestrator does NOT auto-revert to `auto` when the question
is resolved; the policy is that once a human has been pulled
in, they decide when to step back out.

The `mode_changed: "auto-to-manual"` field on the
`AskUserAnswer` (Chapter 4 §4.5) is the agent-visible signal
of the flip; the agent's prompt nudge tells it to update its
working assumption ("you are now in manual mode; the user
will see subsequent questions and may issue commands at any
park").

### 6.5.3 DM0 Q&A Loop as an ask_user Consumer

DM0's no-source authoring loop (§6.4) does NOT implement its
own user-prompting machinery. It is a thin driver that:

1. Computes `MissingField` list via Chapter 2 §2.7's
   traversal.
2. For each field, constructs an `ask_user` invocation with
   `question`, `context`, `kind`, `choices`, `default`, and
   `record_as = "auto-decision"` (since DM0 is authoring the
   spec from scratch, every answer is by definition an
   architectural decision worth persisting). The first call
   for a field omits `thread_id` (fresh thread); the
   orchestrator returns the generated `thread_id` in the
   answer.
3. Awaits the tool result.
4. **Validates the answer.** If the field's `kind` validation
   fails OR the agent judges the reply incomplete /
   ambiguous, it emits a follow-up `ask_user` call with the
   SAME `thread_id` (chained per Chapter 4 §4.5 chaining
   semantics) and `record_as = "none"` on the intermediate
   call.
5. On a closing call (`record_as = "auto-decision"`) the
   orchestrator persists ONE auto-decision row to spec.md
   covering the full thread.
6. Applies the resolved answer to the in-memory `SpecMd`.
7. Commits to `docs/spec.md` per the existing checkpoint
   policy.

The orchestrator's `ask_user` dispatch is the single
implementation of "surface a question, wait for an answer,
record it." DM0's loop driver, ad-hoc DM2d invocations, and
critique-pass clarifications all use the same primitive. There
is no second user-prompting code path.

### 6.5.4 Persistence and Recovery

When `ask_user` lands an answer (per `record_as`):

- `"open-question"`: persists at thread close. For a single-
  turn thread, the Q+A pair is appended to spec.md's Open
  Questions as a resolved entry (with the answer rendered
  as the resolution). For a multi-turn thread, the thread is
  coalesced into ONE entry whose body is the original
  question and whose resolution is the synthesized final
  answer.
- `"auto-decision"`: persists at thread close. Same coalescing:
  one Auto-decision row whose `decision` is the resolved
  final answer and whose `rationale` references the
  clarification count when the thread was multi-turn.
- `"none"`: the call is ephemeral by intent (used on
  intermediate clarification turns within a thread). The
  orchestrator records the call in `metrics.jsonl` for
  observability but does not touch spec.md.

Intermediate Q+A turns are NOT written to spec.md. They live
in the orchestrator's session log and the chat panel
transcript for audit; spec.md sees only the resolved thread.

If spec.md does not yet exist (DM0 hasn't completed),
resolved-thread entries accumulate in
`.sim-flow/spec-ingest/qa-buffer.toml`. DM0's Step B (LLM-
driven completion) pulls them in when writing the initial
spec.md.

Open threads (intermediate turns recorded but no closing
call yet) are buffered separately at
`.sim-flow/<step>/ask-threads/<thread_id>.toml`. A reload
mid-thread recovers from these files: the orchestrator
restores the pending state and waits for the next user reply
(or, if the sub-session ended, force-closes per §6.5.5's
thread-lifecycle rule).

### 6.5.5 Chained Asks Across the DMF Flow

The `ask_user` chaining mechanism (Chapter 4 §4.5 — `thread_id`,
deferred persistence, intermediate `record_as = "none"`) is
not a DM0-only feature. Every DM step that can call
`ask_user` may also chain. Three flow-level concerns:

**Thread lifecycle vs sub-session lifecycle.** A thread can
span multiple LLM turns within a single sub-session (the
natural case — the agent asks, the user replies, the agent
clarifies, repeat). A thread MUST NOT span sub-sessions:
when a sub-session ends (e.g. DM2d work session completes,
or is cancelled), any open thread is force-closed by the
orchestrator. Force-close behavior:

- If the thread has at least one recorded answer, persist as
  a resolved Open Question with body "Resolved through N
  exchanges; final answer: `{last_reply}`".
- If the thread has no answers (rare; only happens if the
  sub-session ends before the user replies to the first
  call), drop the thread silently and log a metric event.

**Cross-step thread continuity.** Threads do NOT carry across
step boundaries. A clarification thread opened in DM2c is
closed by the time DM2d starts; if DM2d needs to revisit the
same topic, it starts a fresh thread (with its own
`thread_id`). The persisted Auto-decision / Open Question
from the earlier step is the carry-over.

**Thread visibility in the chat panel.** Each thread renders
as a connected group of chat-panel bubbles with shared
visual treatment (indentation, thread-label badge). The
chat panel surfaces a `/cancel-thread <id>` shortcut next
to each open thread. The architecture does not prescribe
the exact UI; the contract is "the user can tell which
turns belong to the same conversation and can cancel a
thread without dropping into raw command syntax."

**Critique passes can use chaining too.** When a critique
pass needs human input (e.g. "this milestone has two
possible interpretations of the spec — which did you
intend?"), it uses `ask_user` with chaining like any work
session. Critique-side asks default to
`record_as = "open-question"` since the critique itself is
not authoring decisions.

### 6.5.6 What ask_user Replaces

The DMF flow had two legacy patterns the new tool replaces:

- **Mid-session "use your best judgment" prose in step
  prompts.** Previously the agent was told to "make your best
  educated guess and record the assumption in
  Auto-decisions." With `ask_user`, the prompt becomes "use
  `ask_user` for blocking unknowns; record auto-decisions
  only for inferences you have evidence for in the spec."
- **The manual-mode-only Q&A turn at `wait_for_command`.**
  That mechanism (a separate sub-session triggered by a
  user-typed prompt) is preserved for user-initiated
  questions, but the agent-initiated flow goes through
  `ask_user` rather than parking and waiting for the user to
  ask first.

## 6.6 DM1, DM2a, DM2b, DM2c — Changes

These steps consume the structured spec.md.

### Inputs

- `docs/spec.md` parsed via the Chapter 2 parser into
  `SpecMd`.
- The full lance index (for retrieval).
- (When source spec exists) the spec-ingest corpus.

### New tool usage in prompts

Step prompts gain a "Tools you should use" subsection:

- For DM1 / DM2a / DM2b (analysis steps):
  - `signal_table_query` to enumerate I/O per stage / block.
  - `spec_semantic_search` to expand on prose where spec.md
    is brief.
  - `api_semantic_search` rarely (these steps are
    architectural, not implementation).

- For DM2c (impl plan):
  - All of the above.
  - `api_semantic_search` becomes more relevant; the plan
    references framework symbols.

### Gate-check changes

Where today's gates regex spec.md for specific operation
names, the new gates query the parsed `SpecMd`:

- DM2a `decomposition` gate: verify every operation in
  decomposition.md exists in
  `SpecMd.functional_behavior.operations[].id`.
- DM2b `pipeline-mapping` gate: verify every pipeline-mapping
  stage references a Block in `SpecMd.blocks[]`.
- DM2c `impl-plan` gate: verify each milestone names exactly
  one Block or Operation.

The gate engine gains a thin layer that takes a parsed
`SpecMd` plus the step's specific gate logic. Today's regex-
on-prose path remains for backward compatibility while
projects migrate.

## 6.7 DM2d (Model Implementation) — Changes

The step where invented-API and signal-mismatch failures fire
most often. The most substantive changes.

### Inputs

- All of DM2c's outputs (impl plan + per-milestone files).
- `SpecMd` parsed from `docs/spec.md`.
- Lance index (for retrieval).
- LSP (for live framework discovery).

### Prompt changes

The DM2d work prompt receives the strongest tool nudge:

> Before writing code that uses a framework symbol you do not
> already know:
>
> 1. Call `api_semantic_search` with a natural-language
>    description of the operation you need.
> 2. For each candidate, call `api_hover` with the symbol name
>    to verify the live signature.
> 3. Do not write against a signature you have not
>    `api_hover`ed. `api_semantic_search` results are
>    approximate; `api_hover` is the truth.
>
> When you need to know a block's inputs / outputs, call
> `signal_table_query` with `stage = "<block name>"`. The
> rows are authoritative; the prose in spec.md is
> supporting context.
>
> When you encounter a TBD, an unresolvable ambiguity, or a
> design choice the spec doesn't make for you, call
> `ask_user` as the LAST tool call of the turn. Do as much
> other useful work as you can in the same turn first
> (partial milestone artifacts, hypothesis declarations,
> additional context reads). Do NOT call `ask_user` for
> framework-symbol questions — that's what
> `api_semantic_search` and `api_hover` are for.

### Gate-check additions

DM2d gate gains:

- **Signal-table consistency** (advisory diagnostic, not a
  hard fail): query `signal_table_query` with
  `conflicts_only=true`; any conflicts produce
  `DiagnosticLevel::Warning` so the critique pass and the
  human reviewer see them.
- **Compile / test gates** (existing): unchanged.

### Critique pass

The DM2d critique reads the implemented code against:

- spec.md's Blocks (the agent should have implemented every
  block; missing blocks are findings).
- spec.md's per-block signal tables (the agent's port
  declarations should match every row's direction and width).
- The framework patterns retrieved via `api_*` tools (the
  agent's usage should match the live signatures, not
  invented ones).

Critique findings continue to use the existing
`docs/critiques/DM2d-critique.json` format.

## 6.8 DM3a, DM3b, DM3c — Changes

Verification steps. Similar pattern: consume `SpecMd`,
reach for `spec_semantic_search` for source-spec detail
beyond what spec.md normalizes, reach for
`signal_table_query` to confirm test stimulus shapes match
the design's I/O contract.

### Worked-example consumption

DM3a (test plan) consumes `SpecMd.worked_examples[]` as the
basis for smoke and edge tests. Each worked example becomes
at least one test scenario.

DM3b (smoke / edge / random / coverage test impl) consumes
the test plan plus `signal_table_query` to confirm test
drivers / monitors match block I/O.

## 6.9 DM4 (Performance Analysis) — No Changes

DM4 steps do not consume spec.md structure (they work against
runtime artifacts and code). They see the new tools in the
catalog but rarely call them. No prompt changes.

## 6.10 Critique Passes — General Pattern

Each step's critique pass gains read access to:

- The parsed `SpecMd` for the project.
- The lance index (so the critique can call retrieval tools
  too).

Critique passes always run in a fresh sub-session per current
policy. The retrieval-tool calls a critique pass makes are
metered separately in the metrics stream so we can measure
critique-side tool usage.

## 6.11 Prompt Engineering Summary

The minimum set of prompt edits:

- **DM0 work prompt** rewritten for the new template
  (auto-populate + LLM-completion in source-driven mode;
  Q&A loop in no-source mode).
- **DM2d work prompt** gains the `api_semantic_search` →
  `api_hover` nudge.
- **DM2d critique prompt** gains the signal-table-consistency
  check directive.
- **DM3a / DM3b work prompts** gain the
  `signal_table_query` and `spec_semantic_search` nudges.
- **Universal-tools system message** (the standing prompt
  injected on every step) gains a one-paragraph description
  of each new tool plus a heuristic for when to reach for
  each.
- **Every step's work prompt** gains the `ask_user`
  turn-boundary nudge (Chapter 4 §4.5 prompt-nudge text). The
  agent learns to use the tool for blocking unknowns rather
  than for casual checking or for things retrievable via
  `spec_semantic_search` / `api_semantic_search`.

Exact wording is an implementation-plan concern; the
contract is "the agent knows the new tools exist and what
they're for."

## 6.12 Migration of Existing Projects

Projects with a `docs/spec.md` written against the old
template need migration. Three paths:

1. **Re-author from source spec.** Project's source spec is
   re-ingested, `docs/spec.md` is deleted, DM0 reruns. Loses
   any agent / human edits to the old spec.md.
2. **Auto-migrate via DM0.** A DM0 sub-mode "migrate" reads
   the old spec.md as best it can (best-effort heuristics
   matching the old section headings), populates a new spec.md
   structured template, and presents diffs to the user. Risky
   on subtle content but cheap.
3. **Manual port.** User hand-edits a new spec.md following
   the new template, copying content over.

Path 1 is recommended for active projects; path 2 is
provided for projects without a current source spec on disk.
Path 3 is the fallback.

The migration implementation is in the implementation plan,
not the architecture.

## 6.13 Backward Compatibility

The old spec.md template at
`templates/model-project/docs/spec.md.tmpl` is replaced
in-place. No dual-template support; existing projects either
migrate (§6.11) or stay on a pinned older sim-flow version.

This is acceptable because:

- Projects are small (one human user; one team).
- The migration paths are well-defined.
- The new template is a strict improvement, not a sidegrade.

## 6.14 Observability Additions

The metrics stream gains:

```
tracing::info!(
    target = "sim_flow::metrics",
    event = "dm0_field_filled",
    field_path = "Blocks.Instruction Fetch.signals",
    method = "auto-populated-from-source",   -- or "llm-completed" or "user-dictated"
    source_anchors = ["primary:p12-13"],
);

tracing::info!(
    target = "sim_flow::metrics",
    event = "signal_table_conflict",
    stage = "Instruction Fetch (IF)",
    signal_name = "if_nxt_pc",
    differs_on = ["direction"],
);

tracing::info!(
    target = "sim_flow::metrics",
    event = "invented_api_caught",
    step = "DM2d",
    api_name = "take_input",
    detected_via = "api_semantic_search returned 0 hits before api_hover",
);

tracing::info!(
    target = "sim_flow::metrics",
    event = "ask_user_in_dm",
    step = "DM2d",
    question_kind = "value",
    mode_flipped = true,
    record_as = "auto-decision",
    user_wait_ms = ...,
);
```

These plug into the existing `metrics.jsonl` capture from the
robustness study so the value of the retrieval tools, the
structured spec.md, and the auto→manual flip rate from
`ask_user` can be measured directly.

## 6.15 What This Chapter Does Not Specify

- The exact prompt text for every modified step prompt.
  Implementation-plan concern; this chapter specifies the
  contract (which tools each step should advertise / use,
  what gates check, what critiques look for) but not the
  literal wording.
- The agent-side authoring loop driver code (the structured
  Q&A loop implementation). Implementation concern;
  contract is the `MissingField` traversal in Chapter 2 §2.7.
- The migration tool's implementation. Owned by the
  implementation plan.
- New flow shapes (DSF, SVF). Out of scope.
- Changes to subprocess CLI agents (Claude Code, codex,
  copilot). They see the new tools in the catalog (Chapter 4
  §4.8) and the new spec.md content; no additional changes
  required.
