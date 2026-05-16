# Parallel LLM execution — post-implementation bug review

**Status:** review notes; bugs not yet fixed.
**Created:** 2026-05-16
**Reviewer:** mneilly@numenta.com (self-review)
**Scope:** the seven commits landing the parallel plan-detail walk
dispatcher and its `max_parallel_requests` setting.

Commits reviewed:

| sha | summary |
|---|---|
| 0fee513 | brainstorming doc |
| 06e7e13 | `[llm] max_parallel_requests` config + CLI flag |
| d7e226b | `sim-flow.llm.maxParallelRequests` setting (VS Code) |
| 4e7cb1f | `enumerate_pending_milestones` + `find_milestone_by_name` |
| f343360 | `LlmAdapter` Sync bound + `milestone_name` scoping |
| 4b02605 | parallel plan-detail walk dispatcher |
| ab229b3 | unit tests for dispatcher helpers |

---

## Bug 1 (critical, commit 4b02605) — Phase 2 critique findings overwritten silently

`run_plan_detail_walk_parallel` runs N critiques serially, each
writing to the same `docs/critiques/<step>-critique.json` path
(`prompts/dm2cd-impl-plan-detail-critique.md` lines 82-85 hard-code
the single per-step path). The parallel walker only calls
`read_gate_findings` **after the loop**, so only the **last**
critique's findings are visible. If milestone-1's critique fires
`BLOCKER:` but milestone-N's is clean, the gate passes incorrectly
and the step advances.

The serial walker doesn't have this issue because it checks findings
**between** each critique and either retries Work-for-that-milestone
or moves on. The parallel path skips that intermediate check.

**Fix:** read `read_gate_findings` after each Phase-2 iteration and
halt the loop on the first blocker (or accumulate findings into an
aggregate JSON before exiting Phase 2). Smallest viable patch:

```rust
for milestone_rel in &pending {
    ...
    run_subsession_scoped(..., SessionKind::Critique, ..., Some(bare_name))?;
    let findings = read_gate_findings(&opts.project_dir, step_id);
    if !findings.is_empty() {
        // flip to manual the same way the post-loop check does
        return Ok(true);  // caller sees dirty gate, flips to manual
    }
}
```

This keeps the V1 "no auto-retry" contract; it just catches the lost
findings.

---

## Bug 2 (cosmetic, commit 06e7e13) — `probe_verilator_and_warn` doc orphaned

In `tools/sim-flow/src/commands.rs` lines 895-907, the new
`resolve_max_parallel_requests` function got inserted **between**
the existing `probe_verilator_and_warn` doc comment block and the
function it documented. Now both doc blocks attach to
`resolve_max_parallel_requests` and `probe_verilator_and_warn` has
no doc at all.

**Fix:** move the new function's body (and its own doc) below
`probe_verilator_and_warn`, or insert a blank line between the
verilator doc and my function so rustdoc stops attaching them.

---

## Bug 3 (latent, commit 4b02605) — `ChannelPresenter::recv` returns a second synthetic Hello

`AutoPresenter::queue_synthetic_hello` already queues a Hello in
`pending_reads` for the handshake. After it's consumed by the
orchestrator, the next `host.recv()` call falls through to
`ChannelPresenter::recv`, which **also** returns Hello on its first
call (because `hello_queued: false` is initialized fresh per
worker). `AutoPresenter::recv` matches Hello via
`other => return Ok(other)` and propagates it up.

Today this is benign because auto-mode worker sessions never call
`host.recv()` mid-session (only the handshake does). But it's a
footgun: if any future feature adds a `host.recv()` to the auto
path, the orchestrator would receive a phantom Hello mid-session
and break.

**Fix:** change `ChannelPresenter::recv` to always return `Ok(None)`.
The handshake is satisfied by `AutoPresenter::queue_synthetic_hello`;
subsequent recvs returning None correctly signal "host channel
closed" and the orchestrator handles that path cleanly.

---

## Bug 4 (low severity, commit 4b02605) — worker join results dropped on rx-loop error

In Phase 1's scope closure:

```rust
while let Ok(event) = rx.recv() {
    auto_host.send(&event)?;   // ← propagates Err
}
for h in handles { match h.join() { ... } }   // ← never reached on Err
```

If `auto_host.send` fails (transport-closed, etc.), the closure
returns early via `?` and `thread::scope`'s auto-join silently
drops worker results -- including any `Err` a worker returned and
any panic payload. Probably rare in practice, but the failure path
discards diagnostics that would help debug it.

**Fix:** collect handle results in a block that runs unconditionally
before propagating the rx-loop error. Either swap to an explicit
`scope { ... }; handles.into_iter().map(|h| h.join())` shape, or
catch the `?` into a `let first_send_err = ...;` and process
handles afterward.

---

## Bug 5 (subtle, commit f343360) — MockAgent ordering under concurrent use

Swapping `RefCell` → `Mutex` makes `MockAgent` `Sync`, but it also
changes ordering semantics: `seen.lock().unwrap().push(...)`
serializes through the mutex with no guaranteed order across
concurrent dispatches. All existing tests are single-threaded so
they're unaffected, but a future test that drives N parallel
sessions and asserts e.g. `mock.seen[0] == "milestone-01 prompt"`
will be flaky -- whichever worker grabs the mutex first wins
position 0.

**Fix:** either tag MockAgent dispatches with a per-call sequence
number, or document the semantics change in the type's docstring
("`seen` is unordered when dispatch is called from multiple
threads") so future test authors know not to assert positional
ordering.

---

## Bug 6 (minor consistency, commit 4b02605) — `check_post_subsession` skipped per-worker

The serial walker calls `check_post_subsession` after each
`run_subsession` to catch cap-exceeded diagnostics. The parallel
path only calls it once, after Phase 2's last Critique. A Phase-1
worker that emits `max_auto_iters` won't trigger the cap-handling
until after **all** Phase 2 critiques complete -- in the meantime
the other workers keep running past the cap.

**Fix:** drain `cap_exceeded` state out of worker `AutoPresenter`s
into a shared `AtomicBool` the coordinator checks between events,
or hoist the check into the rx-forwarding loop ("if I just forwarded
a max_auto_iters Diagnostic, set the shared flag"). For V1 the
existing post-Phase-2 check is acceptable.

---

## Bug 7 (cosmetic, commit f343360) — `pick_touched` computed when `milestone_name` is Some

In `tools/sim-flow/src/__internal/session/orchestrator.rs` line 296,
`pick_touched` is always computed but only consumed in the `None`
arm of the milestone-name match:

```rust
let pick_touched = match opts.kind { ... };
let resolved = match &opts.milestone_name {
    Some(name) => find_milestone_by_name(...),       // pick_touched ignored
    None => find_current_milestone(..., pick_touched),
};
```

Dead computation, no behavior change. Fix by moving `pick_touched`
into the `None` arm so it's only computed when used.

---

## Bug 8 (semantics, commit 4e7cb1f) — asymmetric directory-missing handling

`enumerate_pending_milestones` returns an empty `Vec` when the
directory is missing; `find_milestone_by_name` returns
`CurrentMilestone::NoMilestonesPresent`. Two different "nothing
found" outcomes for what is structurally the same condition.
Callers must remember which is which.

Currently no caller depends on the distinction in a load-bearing
way, but if the parallel walker grows a "directory missing -> flip
to manual" branch and reuses `enumerate_pending_milestones`, the
absence of the `NoMilestonesPresent` signal could silently hide
a setup error.

**Fix:** align both helpers on the same return shape, or document
the difference and tighten the parallel walker's empty-pending
handling to match.

---

## Bug 9 (potential, commit 4b02605) — partial-resume after crash leaves stale per-step JSON

If the orchestrator crashes between Phase 1 and Phase 2, on resume:
- The crashed run's prior `docs/critiques/<step>-critique.json` is
  still on disk (whatever the prior critique wrote).
- `enumerate_pending_milestones` returns 0 stubs (all detailed by
  Phase 1).
- The parallel walker's early-exit "fewer than 2 pending" returns
  `Ok(false)`, falling through to the serial walker.
- Serial walker's `find_current_milestone` returns `AllResolved`,
  `try_advance_classified` runs the gate, `critique_clean` reads
  the stale JSON.

Whether this advances incorrectly depends on what the prior crash
left behind. Likely safe in practice (the prior critique was
typically clean or the run wouldn't have made it to Phase 2), but
the gate is being judged against a critique nobody re-validated
on this run.

**Fix:** on parallel-walk re-entry, treat a missing pending-stubs
set as "run a clean-up critique pass" rather than "skip Phase 2."
Or: invalidate the prior JSON on Phase 1 entry so an interrupted
run can't reuse it.

---

## Triage

| # | severity | fix complexity | suggested timing |
|---|---|---|---|
| 1 | critical | small | before any production use |
| 2 | cosmetic | trivial | next cleanup commit |
| 3 | latent | trivial | bundle with the Bug-1 fix |
| 4 | low | small | bundle with the Bug-1 fix |
| 5 | subtle | small (doc) | bundle with the Bug-1 fix |
| 6 | consistency | medium | follow-up; V1 acceptable |
| 7 | cosmetic | trivial | bundle with the Bug-1 fix |
| 8 | semantics | small | follow-up |
| 9 | edge case | medium | follow-up |

Recommended landing order: Bug 1 + 2 + 3 + 4 + 5 + 7 in a single
"parallel-walk correctness + cleanup" commit; defer 6, 8, 9 to a
follow-up once real-world use surfaces which of them actually bite.
