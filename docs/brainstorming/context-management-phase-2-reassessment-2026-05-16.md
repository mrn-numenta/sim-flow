# Phase 2 reassessment — 2026-05-16

The original [context-management brainstorm](./context-management-2026-05-16.md)
called for Phase 2 to:

> Re-architect critical bucket: pull state.toml, step descriptor,
> critique findings out of the message stack; re-prepend each turn
> from disk.

A focused survey of `orchestrator.rs::build_initial_messages` +
the dispatch loop's message-mutation patterns turned up evidence
that the duplication Phase 2 targets is **smaller than the
brainstorm assumed**. This note captures what we found and what
to do about it.

---

## What's actually in the prompt stack today

`build_initial_messages` (`orchestrator.rs:2699-3063`) produces
6-8 system messages + 1 user message at session start. The
content breakdown:

| Item | Embedded in stack? | Source / size |
|-|-|-|
| Step's system prompt | YES | `instruction_slug.md` + conventions (~8-15 KB) |
| Current `state.toml` | NO | Agent fetches via tools at runtime |
| Step descriptor | TOC only (paths listed) | ~2-5 KB stable system message |
| Critique findings | CONDITIONAL | Inlined only on Work-after-blocker / Critique-retry |
| Milestone task list | TOC only (path listed) | Agent reads body via tools on demand |

The orchestrator deliberately keeps "stable" content (system
prompt, conventions, predecessor TOCs) at the head of the stack
and "volatile" content (critique findings on retry, milestone
scope, cargo report) at the tail — see the comment block at
`orchestrator.rs:2995-3001` that explicitly preserves the vLLM
prefix-cache assumption.

## What changes between turns

Within a single `run_session` call:

- Messages are **append-only**. The compaction module (Phase 1a)
  is the only mid-session mutation; it replaces evicted bodies
  with stubs in place but never removes indices.
- **state.toml is not re-read.** Loaded once at session start
  (`auto.rs:213`); even if the agent's tools mutate it, the
  orchestrator's in-memory view stays at the load-time snapshot.
- **Critique findings are not re-read.** Pulled once during
  `build_session_inputs` (`orchestrator.rs:3364`).
- **Milestone files are not re-read.** Their path is resolved once;
  the agent reads bodies via `read_file` on demand.

## What that means for Phase 2

The "critical bucket re-prepend from disk" design assumed all
five items above were inlined into the stack on every turn.
Three of the five (state.toml, step descriptor, milestone task
list) aren't actually in the stack at all — they're referenced
by path, with the agent fetching bodies via `read_file`. The
two that ARE inlined (step's system prompt, conditional critique
findings) appear **once** in the stack and ride along for every
turn — but they're not "duplicated"; they're "carried."

The savings from Phase 2 as originally framed are therefore:

- **None** for state.toml / step descriptor / milestone task list
  (they were never duplicated).
- **One-turn delay reduction** for critique findings: if the agent
  resolves a blocker by editing `critique.json` mid-session, the
  next dispatch could re-read the file and reflect the resolution
  instead of carrying the stale findings forward. But this only
  helps the per-step critique sub-session, and only when the agent
  edits the critique file (which the gate-check requires anyway).
- **Cache invalidation risk** if implemented naively: re-prepending
  fresh content each turn would shrink the vLLM prefix cache hit
  window. The comment at `orchestrator.rs:2995-3001` explicitly
  warns against this.

## Recommendation

**Defer Phase 2 as written.** The disposable / Phase-1a side of
compaction (dedup, mutation invalidation, per-tool caps) is doing
the work the brainstorm hoped Phase 2 would do.

Three alternative directions worth considering, ordered by ratio
of value to risk:

### Option α — Add the deferred phase-boundary cleanup (Phase 1a step 5b)

The remaining unbuilt piece of Phase 1a. Drops tool results not
cited by the final assistant text of a sub-session, at
`SubSessionEnded` time. Lower risk than Phase 2, similar weight
shed for chatty sub-sessions. Citation analysis is the
implementation challenge.

### Option β — Mid-session critique re-read

Surgical: when the agent invokes a tool that writes the critique
file (only `write_file` against `*-critique.json`), trigger a
re-read of the findings and stub the prior critique findings
message. Tiny scope (~150 LOC + tests). Real but narrow benefit
(faster resolution loop during critique sub-sessions).

### Option γ — Universal in-stack content dedup

Even more conservative: scan the persistent stack for
byte-identical message bodies and evict all but the latest.
Today's path-keyed dedup only catches `read_file` / `list_directory`
results. Universal dedup would also catch e.g. duplicate
system-prompt fragments or repeated tool-error messages. Risk:
false positives where the duplication is intentional (e.g. the
agent re-quoted a section to make a point).

## Suggested next move

If the goal is to keep shipping deterministic-compaction wins,
Option α (phase-boundary cleanup) is the cleanest follow-up to
the existing Phase 1a work. Option β is a surgical strike with a
clear use case but limited reach. Option γ is opportunistic but
needs a sanity check on false positives.

Phase 2 as originally framed is no longer the highest-leverage
next step.
