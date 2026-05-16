# Parallel LLM execution in the DMF (scoping doc)

**Status:** draft / scoping. No code changes yet.
**Created:** 2026-05-15
**Owner:** mneilly@numenta.com
**Motivation:** the Direct Modeling Flow is a single linear
chain of 14 sequential gates with work + critique per step
and one-milestone-at-a-time walks inside the heavy steps. Every
LLM call is dispatched on a critical path; modern LLM APIs
are happy to take concurrent requests, and many of our
sessions are reading the same source documents and writing
to disjoint output files. There is a large unused parallelism
budget. This doc scopes how to claim it.

This doc does **not** propose code changes -- the goal is
to align on (a) which parallelism opportunities exist, (b)
which require plan-format changes vs. orchestrator-only
changes, and (c) a phased rollout that keeps the existing
serial path as the default until each tier is proven.

---

## 1. What's serial today and why

The DMF runs 14 gated steps in a fixed order
(`tools/sim-flow/src/__internal/steps/dm.rs`):

```
DM0 -> DM1 -> DM2a -> DM2b -> DM2c -> DM2cd -> DM2d
    -> DM3a -> DM3ad -> DM3b -> DM3c
    -> DM4a -> DM4ad -> DM4b
```

`StepDescriptor::prerequisite: Option<&str>` carries a single
parent name. The state machine has one `current_step`.
There is no DAG, no fan-out, no per-milestone state.

Inside a step:

- **Work then Critique** is fundamentally sequential -- the
  critique reads the work output. This is by design (the
  critique session was split out specifically so it could
  review work as a fresh independent session) and is **not**
  what we want to parallelize.
- **Milestone walks** (DM2cd, DM2d, DM3ad, DM3b, DM3c, DM4ad,
  DM4b) iterate `milestone-NN-*.md` stubs one at a time in
  `session/auto.rs`. The walker picks the first file with
  pending rows, runs a Work + Critique pair, advances, repeats.
  This is the largest pool of trivially-parallelizable LLM
  calls in the flow.

Critique sessions across different steps already run
sequentially in the gate chain. They could overlap any time
the underlying artifacts they read are disjoint.

## 2. Where parallelism is actually available

Three tiers, in increasing order of effort.

### Tier 1 -- already-independent siblings (no plan-format change)

These cases are parallel today by construction; only the
orchestrator's serial dispatch makes them sequential.

**Plan-detail steps** (DM2cd, DM3ad, DM4ad). Each one walks N
milestone stubs and replaces `<!-- detail-pending` with a full
task list. Each stub reads the **same** outline + source docs
and writes to its **own** `milestone-NN-*.md` file. Zero
cross-stub dependencies, zero filesystem conflict. A 20-
milestone design currently makes 20 serial LLM round trips;
all 20 can run concurrently. The gate is unchanged: it just
waits until every stub is filled.

**Multi-artifact non-walk steps**:

| Step | Artifacts emitted |
|---|---|
| DM1 | `docs/targets.md`, `docs/testbench.md` |
| DM2a | `docs/analysis/decomposition.md`, `docs/analysis/data-movement.md` |

Each artifact can be authored by a separate sub-session reading
the same predecessors. The critique still happens once per step
after both artifacts land.

**Critique-with-next-work pipelining.** Within a walk, while
milestone N's **critique** is running, milestone N+1's **work**
can start. The critique only reads milestone N's output; the
next work session reads the same predecessors N's work session
did. Free 2x on every walk step with no plan-format change and
only one extra concurrent session slot.

### Tier 2 -- single-prompt, multi-question fan-out

Several individual prompts ask the agent for multiple
sub-deliverables that don't need to share a chain of thought.
A DM2a session today produces both `decomposition.md` and
`data-movement.md` in one conversation. Splitting that into two
sibling LLM calls (one prompt per artifact, both reading the
same predecessors) gives a 2x reduction in wall time on top of
Tier 1, since the dependency is structural, not conversational.

Same pattern shows up in DM1, DM3a, DM4a -- any step that
emits multiple top-level artifacts from the same predecessor
set.

This tier needs **prompt-template work**, not state-machine
work: the new prompts must be self-contained (don't reference
"the other artifact you wrote earlier"), and the post-condition
becomes "both files exist" instead of "the agent stopped."

### Tier 3 -- milestone-level DAG (needs plan-format change)

DM2d, DM3b, DM3c, DM4b execute real code/tests/sweeps where
milestones can genuinely interact. This is where the user's
core observation applies: **the plan-creation step must
identify dependencies** so the walker can run independent
milestones concurrently.

Today the milestone files are an ordered list with no declared
dependencies. The proposal is to extend the milestone-NN-*.md
template with two frontmatter fields:

```yaml
---
milestone: 04
title: Decode stage
depends-on: [02, 03]       # payload types + module skeletons
touches:   [src/model/decode/, src/model/payloads.rs]
---
```

`depends-on:` -- the data dependency graph. A milestone may
start once every dep has passed its per-milestone gate.

`touches:` -- the write-path advisory. The orchestrator does
not co-schedule milestones whose `touches:` overlap, because
real LLM agents will clobber each other on shared files
(`Cargo.toml`, `src/lib.rs`, `src/model/top.rs`, etc.).
Missing `touches:` defaults to "touches everything" --
forces serialization, preserving today's behavior.

The plan-detail step (DM2cd / DM3ad / DM4ad) is what populates
these fields. The outline step (DM2c / DM3a / DM4a) can also
emit them, but the detail step is the natural authority since
it's already analyzing per-milestone scope.

#### Which Tier-3 step is the easiest first win?

| Step | Concurrency potential | Reason |
|---|---|---|
| DM3c (test execution) | High | Smoke/Edge/Stress/Random categories live in separate test files; only the testbench scaffolding is shared |
| DM4b (perf analysis) | High | Different workloads / sweeps write to different `docs/analysis/<topic>.md` reports |
| DM3b (testbench impl) | Medium | Sequencer / Driver / Monitor / Scoreboard live in separate files but share `SimEnvBuilder` wiring |
| DM2d (model impl) | Low-medium | Milestones often share `src/lib.rs`, `Cargo.toml`, `src/model/top.rs` -- the connectivity scaffolding by construction |

Order of rollout: DM3c first, then DM4b, then DM3b, then DM2d
with a small concurrency cap.

## 3. What blocks each tier

| Blocker | Tier 1 | Tier 2 | Tier 3 |
|---|---|---|---|
| Orchestrator must support N concurrent sessions | yes | yes | yes |
| `state.toml` per-milestone status | yes (for walks) | -- | yes |
| Prompt-template changes | -- | yes | optional |
| Plan-format change (`depends-on:`, `touches:`) | -- | -- | yes |
| Filesystem conflict avoidance | (none) | (none) | `touches:` advisory, or per-milestone git worktrees |
| Single-session AI-client policy (`single` in the mode matrix) | breaks | breaks | breaks |

The `single` session policy from `docs/flow/02-direct-modeling-flow.md`
section "Mode x session-policy matrix" injects all prompts
into one long-lived agent process. That model is fundamentally
incompatible with concurrent LLM calls. Parallel execution
only applies to `per-step` session policy. The matrix gains a
new dimension or the `single` cell stays single-threaded.

## 4. Proposal -- four-phase rollout

**Phase 1: Pipeline Work / Critique within a walk.** One extra
concurrent session slot. No plan-format change. No DAG. Smallest
diff. Roughly 2x on every walk step. Establishes the concurrency
primitive (a session-slot pool plus a way to launch a Work
session while a Critique is in flight) that later phases reuse.

**Phase 2: Parallelize plan-detail steps (DM2cd / DM3ad /
DM4ad).** Each stub is an independent LLM call writing to its
own file. Concurrency bounded by `--max-parallel` flag (default
4, raisable). Largest absolute speedup because these steps
often have 10-25 stubs and each is a separate round trip today.

**Phase 3: Add `depends-on:` and `touches:` to milestone
frontmatter.** Teach the plan-author prompts (DM2c, DM3a, DM4a)
and detail prompts to populate them. The walker reads them but
does not yet schedule concurrently -- this phase only proves the
metadata is being filled in correctly and gates pass under the
old serial walker.

**Phase 4: DAG-aware walker.** Consume the metadata; schedule
independent milestones concurrently. Roll out per step in the
order DM3c -> DM4b -> DM3b -> DM2d. Each rollout is a flag flip
plus a milestone-cap increase, not a code change.

## 5. State-model changes

`state.toml`'s per-step gate row becomes per-milestone where the
step has a walk:

```toml
[gates.DM2d]
passed = false                # step gate

[gates.DM2d.milestones]
"milestone-02-payloads" = { passed = true, timestamp = "..." }
"milestone-03-skeletons" = { passed = true, timestamp = "..." }
"milestone-04-decode" = { passed = false }
"milestone-05-execute" = { passed = false }
```

The step gate passes when every milestone gate passes. That
contract already exists today via `milestones_all_implemented`
in `dm.rs` -- this phase just makes the per-milestone state
explicit so the scheduler can reason about it.

## 6. Filesystem contention -- the open question

Tier 3's biggest risk is parallel LLM agents writing to the
same source file. `touches:` is an advisory list the plan
author has to predict accurately. Two strategies, not
mutually exclusive:

**A. Strict `touches:` enforcement.** The orchestrator computes
the intersection of `touches:` across in-flight milestones. If
a candidate milestone's `touches:` overlaps any active set, it
waits. Cheap, but only as good as the plan author's prediction.
False negatives (the agent wrote to a file it didn't declare)
produce silent conflicts.

**B. Per-milestone git worktrees.** Each parallel milestone runs
in its own worktree off the same base commit. The orchestrator
merges results after all in-flight milestones at the current
DAG level land. Strong isolation; correct by construction. The
cost is merge complexity for milestones that genuinely do
touch the same file (and merge conflicts that an agent has to
resolve).

Recommendation: ship `touches:` enforcement first (Phase 4) with
DM3c and DM4b -- both have naturally disjoint outputs. Defer
worktrees to a follow-up if DM3b / DM2d need them. They may
not, if `touches:` predictions are accurate enough.

## 7. What we explicitly don't parallelize

- Work + Critique within a single milestone. Critique reads
  work output.
- Cross-step gates. DM3a still needs DM2d. DM4a still needs
  DM3c. The chain between steps stays linear; only inside a
  walk does parallelism happen.
- The `single` session-policy mode. Concurrent prompts into one
  long-lived agent process is not a thing.
- Per-step critique iterations on the same milestone. If the
  critique fires a BLOCKER, the next work session for that
  milestone has to read the critique. That stays serial.

## 8. Concrete metrics to track

To know whether each phase is paying off:

- **Wall-clock per gate-passing run** (overall headline).
- **LLM round trips per step** (should stay constant; we are
  parallelizing the same calls, not adding new ones).
- **LLM token spend per step** (should stay constant -- this is
  a sanity check that we're not duplicating prompts).
- **Per-milestone gate failure rate** (should NOT rise with
  concurrency; if it does, a `touches:` advisory is wrong or a
  `depends-on:` is missing).
- **Cargo-build serialization wait** in DM2d / DM3b / DM3c
  (parallel agents will queue on the build lock; this is the
  real chokepoint after LLM time).

## 9. Open questions

1. **Does the AI client abstraction multiplex?** Each client
   currently spawns a subprocess (`session/clients/claude.rs`,
   `codex.rs`, `gh_copilot.rs`). For Tier 1 parallelism we
   need to spawn N at once. Concurrent stdio capture, control
   socket routing, and event-tap channel keying all assume one
   active session -- audit needed.
2. **How does `auto.rs`'s milestone-walk loop become a scheduler?**
   It is currently a sequential `while pending > 0` loop. The
   conversion to a level-by-level DAG scheduler is the largest
   single piece of new code in Phase 4.
3. **Where does the parallelism cap live?** Per-step in
   `StepDescriptor`, per-run on the CLI, or both?
4. **Does the dashboard need a multi-session view?** Today the
   step-rail tile shows one active session. Phase 4 implies N
   active tiles at once.
5. **Cost cap interaction.** `docs/api-cost-management.md`
   tracks per-run spend. Concurrent calls don't change total
   spend but they spike instantaneous rate; the cap should
   measure cumulative tokens, not parallelism.

---

## 10. Summary

The DMF has substantial unused LLM parallelism: plan-detail
steps with N independent stubs (Tier 1), multi-artifact
non-walk steps (Tier 2), and milestone-level execution within
walk steps once dependencies are declared (Tier 3). Phases 1
and 2 are orchestrator-only and pay off immediately. Phase 3
introduces a minimal `depends-on:` / `touches:` extension to
the milestone format. Phase 4 turns the milestone-walk loop
into a DAG scheduler. Each phase is independently shippable
and reversible.
