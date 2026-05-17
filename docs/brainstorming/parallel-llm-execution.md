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

## 4. Proposal -- phased rollout

> **History:** the original phases 1+2 collapsed into a single
> "in-place parallel walk over plan-detail steps" feature that
> landed in commits 06e7e13..ab229b3. Post-implementation review
> (`parallel-llm-execution-bug-review.md`) surfaced Bug 1: the
> serial Phase-2 critique loop writes to the same per-step JSON
> path, so only the LAST critique's findings survive. The
> worktree-based design below was triggered by that bug and the
> realization that per-worker filesystem isolation kills it by
> construction. The in-place implementation stays as a fallback
> for non-git projects.

**Phase A: Worktrees for plan-detail walks (DM2cd / DM3ad /
DM4ad).** Each worker runs in its own `git worktree` off the
current HEAD. The worker performs BOTH Work and Critique
back-to-back inside its worktree, so each milestone gets its own
filesystem -- including its own `docs/critiques/<step>-critique.json`.
The coordinator reads each worktree's critique JSON, aggregates
findings across milestones (no overwrite race), and merges each
worktree's milestone file back into the main working tree. Bug 1
disappears by construction. Bug 9 (stale per-step JSON from a
crashed prior run) disappears too because every Phase-A run writes
to fresh worktrees.

Why this is the natural Phase A: none of the plan-detail steps
WRITE to `src/`, and DM3ad / DM4ad don't even READ it -- their
prompts list only `docs/` inputs. DM2cd reads `docs/spec.md` plus
`docs/analysis/*` and writes only to `docs/impl-plan/`. So the
merge is exclusively over disjoint `docs/` files plus an
aggregated critique JSON. No conflict resolution logic needed.

**Phase B: Worktrees + `touches:` for execution walks (DM2d /
DM3b / DM3c / DM4b).** Adds the milestone-frontmatter dependency
metadata so the orchestrator can schedule disjoint milestones
concurrently. Each worker runs in its own worktree (own
`target/` too -- parallel cargo builds for free), and the
coordinator merges via git's three-way merge. Milestones that
declare overlapping `touches:` paths fall back to serial.
DM3c first (smoke/edge/stress/random categories are naturally
disjoint), then DM4b, then DM3b, then DM2d with a tight cap.

```yaml
---
milestone: 04
title: Decode stage
depends-on: [02, 03]       # payload types + module skeletons
touches:   [src/model/decode/, src/model/payloads.rs]
---
```

**Phase C: cross-step speculative execution.** While DM2d
finishes its last milestone, speculatively start DM3a planning
against the in-progress source tree. Risky -- DM3a's outputs
get invalidated if DM2d revises. Almost certainly overkill;
flag for later.

**Fallback path: in-place parallel walk (already landed in V1).**
Non-git projects or projects where `git worktree add` fails
keep the in-place serial-Phase-2 path from commits
06e7e13..ab229b3. Bug 1's fix in that path is to check findings
after EACH Phase-2 critique and halt on the first blocker. The
worktree path is preferred when available.

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

## 6. Filesystem contention -- worktrees are now the primary mechanism

Earlier this section debated `touches:` advisories vs.
worktrees. Phase A's rollout flips that: **worktrees are the
default**, `touches:` becomes a Phase-B scheduling hint that
only kicks in when worktree merges would conflict.

**Per-milestone git worktrees.** Each parallel worker runs in
its own `git worktree` off the current HEAD (typically at
`.sim-flow/worktrees/<step>-<milestone>/`). The worker performs
its full Work + Critique pair inside the worktree -- so every
worker writes to its own filesystem, including its own
`docs/critiques/<step>-critique.json`. Reads from upstream
artifacts (`docs/spec.md`, `docs/analysis/*`, etc.) are
concurrent-safe because they're unmodified during the walk.
Strong isolation; correct by construction. Phase-A plan-detail
walks NEVER conflict at merge time -- each worker contributes
exactly one detailed milestone file plus an aggregated critique
JSON.

**`touches:` advisory (Phase B).** Once execution walks join
the parallel path, milestones that genuinely touch the same
source file (Cargo.toml, src/lib.rs) can't safely fan out even
with worktrees -- git's three-way merge fails on overlapping
edits. The orchestrator computes the intersection of `touches:`
across in-flight milestones and serializes any overlap.
`touches:` becomes the scheduling hint that decides which
milestones can cohabit a parallel batch.

**Phase A's merge logic** is trivial precisely because the
plan-detail steps write to disjoint paths:

- DM2cd writes only to `docs/impl-plan/milestone-NN-*.md`
- DM3ad writes only to `docs/test-plan/{tb,test}-milestone-NN-*.md`
- DM4ad writes only to `docs/perf-plan/perf-milestone-NN-*.md`

Plus each worker writes `docs/critiques/<step>-critique.json`.
The coordinator copies each worker's milestone file back to
main, aggregates the critique JSONs into one main-tree JSON,
then renders the markdown. No merge driver, no conflict
resolution.

**Worktree lifecycle.** Create on Phase-A entry, destroy on
exit. On crash, `git worktree prune` on next startup cleans up
orphans. Disk cost is bounded by `max_parallel_requests` ×
working-tree size; the shared `.git/` object store means it's
cheaper than N independent clones.

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

1. ~~Does the AI client abstraction multiplex?~~ **Answered by
   V1 implementation:** `LlmAdapter` now requires `Send + Sync`
   and the in-place dispatcher shares one adapter across worker
   threads via `RefAdapter`. Carries forward to Phase A
   unchanged.
2. **Worktree path resolution inside the orchestrator.** The
   orchestrator reads `opts.project_dir` for state, prompts,
   artifact writes. For Phase A workers it must point at the
   per-worker worktree path, not the main project. We already
   thread `milestone_name` into `OrchestratorOptions`; the
   worktree path likely lives alongside it (or `opts.project_dir`
   is just overwritten per worker, which is simpler).
3. **`.sim-flow/` propagation into worktrees.** Workers need
   `state.toml`, `config.toml`, prompts, and the project
   templates. Simplest: copy `.sim-flow/state.toml` and
   `.sim-flow/config.toml` into the worktree on create; the
   worker reads them but doesn't write state (state advances
   happen on the main thread post-merge). The rest of `.sim-flow/`
   (logs, critiques, etc.) is per-worktree by design.
4. **Critique JSON aggregation schema.** When the coordinator
   merges N per-worker critique JSONs into one main-tree JSON,
   the aggregation rule needs to compose with the existing
   `Critique::load` gate parser. Probably "concatenate findings
   arrays, tag each with its source milestone." Confirm against
   `src/__internal/critique.rs`.
5. **Dashboard multi-session view.** Today the step-rail tile
   shows one active session. Phase A workers all emit
   `SubSessionStarted` / `SubSessionEnded` through the shared
   coordinator-side mpsc; the dashboard currently expects
   nested singletons. May render fine (just rapid in/out
   bracketing); may need a per-worker swimlane. Worth verifying
   before declaring Phase A user-facing.
6. **Cost cap interaction.** `docs/api-cost-management.md`
   tracks per-run spend. Concurrent calls don't change total
   spend but they spike instantaneous rate; the cap should
   measure cumulative tokens, not parallelism.

---

## 10. Summary

The DMF has substantial unused LLM parallelism: plan-detail
steps with N independent stubs, multi-artifact non-walk steps,
and milestone-level execution within walk steps once
dependencies are declared. V1 landed an in-place parallel
walker (commits 06e7e13..ab229b3) and surfaced one critical
correctness bug (per-step critique JSON overwrite, see
`parallel-llm-execution-bug-review.md` Bug 1).

The revised rollout uses git worktrees as the primary
isolation mechanism:

- **Phase A**: worktrees for plan-detail walks (DM2cd /
  DM3ad / DM4ad). Each worker runs Work + Critique inside
  its own worktree, eliminating the critique-JSON race by
  construction. Merge is trivial because plan-detail steps
  write to disjoint `docs/` files.
- **Phase B**: worktrees + `touches:` for execution walks
  (DM2d / DM3b / DM3c / DM4b). `touches:` schedules disjoint
  milestones into parallel batches; worktrees handle the
  filesystem isolation. Per-worktree `target/` directories
  parallelize cargo builds for free.
- **Phase C**: cross-step speculative execution. Flagged for
  later; almost certainly overkill.

V1's in-place implementation stays as a fallback for non-git
projects and as the supported `max_parallel_requests = 1`
behavior. Each phase is independently shippable and
reversible.
