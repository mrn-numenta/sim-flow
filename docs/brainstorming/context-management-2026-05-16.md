# Context-management brainstorm — 2026-05-16

How sim-flow should keep prompt-stacks below the model's context
window without losing fidelity. Original problem: zero pre-flight
budgeting, no compaction, no per-tool caps beyond `read_file`'s
16 KB. Overflow → server 4xx → orchestrator parks with Retry /
Cancel; `/retry` re-sends the same too-large stack and fails
identically. Recovery required `/end-session`. See
`orchestrator-bug-audit-2026-05-16.md` for the bug-side picture.

**Implementation status (2026-05-16):** Phases 1a (steps 1-5a) +
1b + 3 are shipped (`10 commits`, sequenced below). Phase 2 was
reassessed mid-implementation and deferred — the duplication it
targeted turned out to be smaller than the brainstorm assumed; see
`context-management-phase-2-reassessment-2026-05-16.md` for the
evidence + alternative directions. Phase 1a step 5b
(phase-boundary cleanup) is also deferred.

The rest of this doc is the original design discussion. Skip to
[Implementation status](#implementation-status) at the bottom for
what actually shipped.

---

## Techniques on the table

The classic levers, in roughly increasing cost:

- **Truncation** — drop oldest turns until the stack fits. Cheap,
  but loses early decisions.
- **Middle elision** — keep the first few turns (system prompt /
  step descriptor) + the most recent N, drop the middle.
- **Drop newest** — almost never the right call; included for
  completeness.
- **Summarization** — replace a range of old turns with an
  LLM-generated summary. Adds an extra round trip; lossy.
- **Partitioning** — split context into critical (always kept) +
  disposable buckets, compact only the disposable side.
- **Stale-context removal** — the headline of this doc. Identify
  context that is *no longer relevant* (not just *old*) and drop
  it without information loss.

Phasing-wise, the cheapest deterministic levers should run before
any LLM-assisted ones.

---

## Staleness detection signals

Without an LLM round-trip, the orchestrator can already infer most
staleness:

- **Path-keyed dedup.** `read_file("docs/spec.md")` followed by
  another `read_file("docs/spec.md")` → drop the first result.
  Same for `list_directory`, `search`, `grep`. The result is just
  a function of the args; keeping more than one copy is waste.
- **Mutation invalidation.** Track every `write_file` / `edit_file`
  the agent makes within the session. Any prior `read_file` result
  for a now-mutated path is **actively misleading** (it shows
  pre-edit content the agent might re-edit). Drop it.
- **Per-tool TTL.** `list_directory` from 20 turns ago is rarely
  cited again. `read_file` for the file being authored is needed
  every turn. Declare a TTL per tool kind (turns, or
  references-since).
- **Reference counting.** Walk later tool-call args and assistant
  prose for path / symbol citations. If no later turn refers to a
  prior result, it's a drop candidate.
- **Phase boundaries.** sim-flow has natural compaction points:
  - end of a sub-session
  - between milestones in a walk
  - on critique commit
  Use them. Don't compact across a critique-resolution turn
  before the gate clears.

---

## Partitioning for sim-flow

Two buckets map cleanly onto what's already on disk:

### Critical (refreshed each turn from disk)
- Step's system prompt
- Current `state.toml`
- Step descriptor (work artifacts, predecessor inputs, gate checks)
- Critique findings (unresolved + blocker only — resolved findings
  don't drive behaviour)
- The milestone-in-flight's task list

### Disposable (subject to compaction)
- All tool results
- Back-and-forth dialogue
- Background reads / exploration

The critical bucket comes "for free" — the orchestrator already
has the canonical source on disk. Re-prepending each turn from disk
sheds duplicate weight without any clever algorithm. A lot of
current prompt-stack volume is duplicate critical content.

---

## Active (agent-declared) discards

Designs worth considering, in increasing agent burden:

- **`forget(refs)`** — agent explicitly drops listed turn ids /
  paths. High agency; requires agent discipline.
- **`note_to_self(text)`** — fire-and-forget; orchestrator stores
  the body in a small (~2 KB) scratchpad that's always prepended.
  No turn lands in the stack at all. Pairs well with the critical
  bucket pattern.
- **Side-channel scratch file** — agent writes to
  `generated/scratchpad.md` via the existing tools; orchestrator
  pins that file into the critical bucket. Reuses FS plumbing; no
  new tool.

The `note_to_self` design is cheap to implement once the critical
bucket exists.

---

## Footguns

- **Loop hazard.** Agent reads `foo.rs`, gets answer, content
  dropped, agent re-reads `foo.rs`. Mitigation: replace dropped
  results with a *metadata stub* — "we read this at turn 12, 240
  lines, hash abc…, content compacted" — so the agent knows the
  content existed and was here.
- **Summarization drift.** Line numbers, exact identifiers, error
  messages all degrade. Anything the agent is supposed to cite
  verbatim shouldn't go through summarization. Critique findings
  are the canonical example.
- **Critique recursion.** Critique findings live in
  `critique.json`; redundantly injecting them into the message
  stack inflates without value. But the *dialogue* resolving them
  belongs in the stack. Don't compact past the resolution point
  before the gate clears.
- **Premature TTL.** Aggressively short TTLs cause re-reads. Tune
  per tool, instrument hit/miss rates, adjust.
- **Visibility drift.** If the chat panel transcript shows
  something the agent no longer has in context, the user assumes
  the agent remembers it. The panel needs a visual signal
  (greyed-out, "context evicted" badge, …) so the user
  understands what the agent currently has.

---

## Concrete phasing

| Phase | What | LLM round-trips? | Protocol change? |
|-|-|-|-|
| **1** | Per-tool output caps (not just `read_file`), path-keyed dedup, mutation invalidation, end-of-sub-session cleanup. | none | none |
| **2** | Re-architect critical bucket: pull state.toml, step descriptor, critique findings out of the message stack; re-prepend each turn from disk. | none | none |
| **3** | Query each backend for the real context window (Anthropic `/v1/models`, vLLM/LM Studio /v1/models + native, Ollama /api/show). Surface it to the chat panel pie. Trigger compaction at 90 %. | one (preflight) | yes (state.toml carries window; protocol carries it to panel) |
| **4** | Agent-driven discards: `forget(refs)`, `note_to_self(text)` tools. | none additional | yes (new tools) |
| **5** | Summarization fallback: when phases 1–4 still overflow, run a side LLM call to summarize a turn range and replace the range with the summary. Gated per-step opt-in. | one per compaction | none beyond a config flag |

Phases 1 + 2 are the highest-leverage low-cost work — they likely
eliminate the overflow class for typical sim-flow projects. Phases
3–5 escalate gradually.

---

## Chat panel vs. context (separate decision)

If we compact context, the chat panel's transcript shouldn't also
shrink — losing transcript fidelity hurts debuggability and
operator audit. The panel should retain the full history and mark
turns that have been *evicted from context* with a visual signal
(struck-through, a red ✗ icon in the corner, dimmed opacity).

Design choice for the protocol: orchestrator emits a
`ContextEviction { turn_ids: [...] }` event each time it compacts.
The chat panel toggles a class on the corresponding rows. The
transcript on disk stays untouched.

This decouples "what the agent remembers" from "what the user can
see" — which is the correct asymmetry.

---

## Implementation status

Snapshot at end of the 2026-05-16 push. Each commit landed green
on `cargo test -p sim-flow --lib` (modulo the pre-existing flaky
`__internal::prompts::tests::all_dm_prompts_render_in_both_modes`)
and `npm test` (525 passed / 1 skipped, 41 test files).

### Shipped

| Commit | Phase | What |
|-|-|-|
| `8ec0c63` | preflight | Setting `sim-flow.chatPanel.showContextState` + popover toggle (default off). |
| `34bc2ae` | 3 | Real context-window query per backend (vLLM `max_model_len`, LM Studio `loaded_context_length`, Ollama `<arch>.context_length`, OpenAI-compat). Replaces the cosmetic 128 k constant in the toolbar pie. |
| `310d0c5` | 1a / 1 | Protocol surface for `Event::ContextEvicted { ids, reason }` + `ContextEvictionReason` enum + bus translator + transport subscriber. |
| `e1c1b33` | 1a / 2 | Pure `run_path_keyed_dedup` rule. 9 unit tests covering supersession, chains, distinct paths/tools, malformed args. |
| `5ec01f2` | 1a / 3 | Wire dedup into `run_session` dispatch loop. Position-based ids (`msg-N`) correlate stack slots to wire-event payloads. |
| `e82b9c6` | 1a / 4 | `run_mutation_invalidation` rule + wiring. 6 additional unit tests. Compaction module total: 15 tests. |
| `5335bae` | 1a / 5a | Universal 16 KB tool-output cap at the central `invoke_tool` dispatch site. Defence in depth above per-tool caps. |
| `5c8c78c` | 1b | End-to-end correlation: per-message id on `Event::LlmRequest`, transcript entries carry the id, chat panel renders evicted bubbles dimmed + grayscale with a red ✗ overlay + native-title eviction tooltip. CSS gated on `body[data-show-context-state="on"]`. |
| `4ede7a9` | — | [Phase 2 reassessment](./context-management-phase-2-reassessment-2026-05-16.md): the original "pull critical content out of the stack" plan no longer pencils out given the actual prompt-assembly layout. Deferred. |

### Deferred

- **Phase 1a step 5b** — phase-boundary cleanup at sub-session
  end. Requires citation analysis (which paths / symbols later
  turns reference). False-negative risk is real; weight shed is
  modest over what the existing dedup + mutation rules already
  catch.
- **Phase 2** — re-architect critical bucket. See the
  reassessment doc; the duplication this targeted isn't actually
  in the prompt stack today, so the payoff doesn't match the
  invasiveness.
- **Phase 4** — agent-driven discards (`forget(refs)` /
  `note_to_self(text)`). Not started; pre-requisites land first.
- **Phase 5** — summarization fallback. Not started; reserved as
  last-resort lever.

### Visible runtime behaviour after these commits

1. Chat panel toolbar pie reads from the real backend context
   window for vLLM / LM Studio / Ollama / OpenAI-compat sources
   (falls back to the cosmetic 128 k constant for Anthropic and
   `vscode` source since those need API-key plumbing).
2. When the agent re-reads a path that was previously read, the
   orchestrator silently swaps the older `Tool` body for a
   `<superseded: a later turn re-invoked …>` stub. The slot
   stays in the stack as a placeholder so the agent knows the
   read existed.
3. When the agent writes / edits / deletes a path that was
   previously read, the prior `read_file` result is swapped for
   an `<invalidated: a later turn wrote / edited …>` stub.
4. Any tool result whose body exceeds 16 KB is truncated at a
   UTF-8 char boundary with an "orchestrator output cap" marker.
5. The chat panel transcript always keeps the full history; with
   "Show context state" turned on in the gear popover, evicted
   rows render dimmed with a red ✗ + a hover tooltip explaining
   the eviction reason.

### Validation suggestion

Run a real session with `RUST_LOG=sim_flow::session=info`
(or check the debug-log file) and observe the `ContextEvicted`
events. Frequent evictions → the deterministic rules are doing
work. Few evictions → tune (or reconsider whether overflow
remains a real problem before adding more compaction).
