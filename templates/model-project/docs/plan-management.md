# Plan Management

This file describes how plans are written and worked through.
There are three sibling plan directories, one per planning step:

- `docs/impl-plan/` (DM2c) -- implementation plan for DM2d.
  - `plan.md` -- index + brief overview pointing at milestone files.
  - `milestone-NN-<name>.md` -- per-milestone task list executed
    by DM2d.
- `docs/test-plan/` (DM3a) -- verification plan for DM3b/DM3c. Two
  parallel milestone sequences live here so each step (DM3b, DM3c)
  walks its own slice without merging anything across consumers:
  - `test-plan.md` -- index + testbench architecture + traceability
    table back to spec / targets.
  - `tb-milestone-NN-<name>.md` -- per-milestone task lists for
    DM3b. Each milestone covers one slice of testbench scaffolding
    (a Sequencer + Driver + Monitor pair, a Scoreboard family,
    `SimEnvBuilder` wiring, etc.). DM3b walks these in order and
    stops for the paired critique after each.
  - `test-milestone-NN-<name>.md` -- per-milestone task lists for
    DM3c. Each milestone covers one slice of test execution
    (a smoke batch, an edge batch, a stress batch, a random
    batch, the coverage measurement). DM3c walks these in order
    and stops for the paired critique after each.
  - `coverage.md` -- coverage-tooling strategy (cargo-llvm-cov
    threshold, exclusions, run command, report path).
- `docs/perf-plan/` (DM4a) -- performance-analysis plan for DM4b.
  - `perf-plan.md` -- index + brief overview.
  - `perf-milestone-NN-<name>.md` -- per-milestone task list
    executed by DM4b.

`NN` refers to a two-digit integer number (`01`, `02`, ...) so the
files sort in plan order.

The three plan directories are siblings under `docs/` so that each
plan's working set stays small and reviewable, even on large
designs. Tools that consume a plan should walk a single directory
and merge nothing across directories.

## Plan structure

A plan starts with a top-level index file (`plan.md` /
`test-plan.md` / `perf-plan.md`) that gives a brief overview and a
table of contents pointing at the per-section sibling files. The
index lives at the top of its plan directory; per-section files
sit beside it.

A milestone holds a list of tasks. Each task is a bullet line
prefixed with a checkbox so it can be marked off as work
progresses.

**Milestone size cap: 10 tasks maximum.** A milestone with more
than 10 `- [ ]` rows is too large to keep in the LLM's working
memory cleanly and forces the agent to either skim or chain
work-then-critique-then-work loops that defeat the per-milestone
review. If a slice of work needs more than 10 tasks, split it
into multiple milestones with a clear axis (one
component-class per milestone, one test-category-batch per
milestone, etc.) and document the split in the index. Critiques
flag any milestone file that exceeds the cap as a `BLOCKER:`.

## Task states

Tasks are checkboxes with three valid states:

- `- [ ]` -- **pending**. Not yet attempted.
- `- [x]` -- **done**. The artifact the task names is on disk and
  verified (the file exists, the test passes, the run was
  recorded, etc.).
- `- [-]` -- **deferred**. A deliberate choice not to do this task
  in the current run. MUST be paired with a `- defer reason: <one
  sentence>` sub-bullet immediately below the row giving a
  specific justification (e.g. "covered by DM3", "blocked on
  hardware feature X", "will revisit after seeing bottleneck
  data").

A milestone is "complete" for stop-and-review purposes when every
row is resolved -- either `- [x]` or `- [-]` with a `defer
reason:`. Pending `- [ ]` rows must be resolved before the
milestone counts as done.

## Order, jumping, and deferring

The default is to do tasks in the order they appear within a
milestone, and milestones in numeric order. Out-of-order work is
sometimes the right answer; here's how to handle it:

- **Forced reordering** (a real dependency was missed in the
  plan): proceed with the dependency first, but document the swap
  by adding a sub-bullet under the moved task:
  `- order swap: blocked by <other-task>; doing it first.` Then
  return to the original row when its dependency is resolved.

- **Discovered better order**: same as above. Do not silently
  rewrite milestone headers or renumber tasks; document the swap
  inline so reviewers see the original plan plus the deviation.

- **Out-of-milestone jumping**: only when correctness requires
  it (a downstream milestone's task exposes a fix needed for the
  current milestone). Document with `- order swap: jumping to
  M<NN>-T<NN>; reason: <one sentence>.` After the jump,
  return to in-order work.

## Deferring vs failing

If a task can't be done now and won't be done in this run, mark
it `- [-]` with a `defer reason:` sub-bullet. Deferring is
deliberate -- it documents a choice, not a failure. The user
revisits deferred rows by editing the plan and re-running the
step.

If a task is failing in a way that should block the gate (e.g.
the test was supposed to pass and the model has a bug that hasn't
been fixed), do NOT defer. Leave the row `- [ ]` and let the
critique surface it as a `BLOCKER:` so the next iteration
addresses it.

## Adding tasks the plan missed

If you discover work that's needed but isn't in the plan, append a
new row to the most relevant milestone with a `- added: <reason>`
sub-bullet:

```markdown
- [x] new_task_name -- one-line description
  - added: <why this wasn't in the original plan>
```

Don't grow the codebase silently; every artifact should trace to
a checkbox in some milestone.

## Stop-points for critique

Each milestone-completion is a natural stop point. When all rows
in a milestone are resolved, the work session emits
`Milestone NN: <name> complete; ready for critique.` plus a
one-line summary, then waits for the paired critique before
moving on. User review may happen around that checkpoint, but
the critique is the primary gate. Do not chain milestones
automatically -- the workflow is expected to inspect and
critique between them.
