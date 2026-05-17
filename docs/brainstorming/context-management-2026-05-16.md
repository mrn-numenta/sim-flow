# Context-management brainstorm — 2026-05-16

How sim-flow should keep prompt-stacks below the model's context
window without losing fidelity. Current state: zero pre-flight
budgeting, no compaction, no per-tool caps beyond `read_file`'s
16 KB. Overflow → server 4xx → orchestrator parks with Retry /
Cancel; `/retry` re-sends the same too-large stack and fails
identically. Recovery requires `/end-session`. See
`orchestrator-bug-audit-2026-05-16.md` for the bug-side picture.

This doc is the design discussion, not a fix plan. Implementation
phasing at the bottom.

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
