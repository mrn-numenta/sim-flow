# DM4b - Performance Analysis (work session)

You are executing step DM4b (Performance Analysis) of the Direct
Modeling Flow. Prerequisite: DM4a gate passed.

Every run you reference in a perf report MUST be recorded in
`.sim-flow/experiments.db`. Use `run_cargo({command: "run",
binary_args: ["--run-id", "<id>"]})` to invoke the project's main
binary, then `record_run({description: "<id>", workload: "...",
candidate: "...", study: "..."})` to log the run into the
experiments index. The index is created on first call; you don't
need to bootstrap it.

## Goal

Execute the performance-analysis plan written in DM4a -- run the
canonical workloads + sweeps, analyze bottlenecks, verify targets,
and produce per-topic reports under `docs/analysis/`. Every claim
in your reports must trace to a recorded experiment in
`.sim-flow/experiments.db`.

## Inputs

- `docs/perf-plan/perf-plan.md` -- the milestone index. Read this first
  to orient.
- `docs/perf-plan/perf-milestone-*.md` -- per-milestone task lists.
  Walk them in order.
- `docs/targets.md` -- the targets the plan traces back to.
- `docs/analysis/decomposition.md`,
  `docs/analysis/pipeline-mapping.md` -- module / stage names for
  bottleneck reporting.
- `docs/spec.md` -- workload assumptions.
- `src/`, `tests/` -- the model and testbench.

Reference material (read on demand):

- **Modeling guide -- observability + analysis chapter**:
  `lib:docs/modeling-guide/05-observability-and-analysis.md` for
  Foundation's metrics conventions and the
  `ObservabilityRunWriter` pattern.
- **Worked examples** with measurement: pick one whose targets
  shape matches yours from `lib:examples/`.

## Procedure

1. Read `docs/perf-plan/perf-plan.md` to orient. The orchestrator
   scopes each work session to ONE
   `docs/perf-plan/perf-milestone-NN-*.md` file at a time -- only
   the current milestone appears in your inputs. Do that milestone
   and stop (see step 2 below); the auto-driver re-launches you
   for the next milestone after the paired critique.
2. **For each milestone**:
   - Read its `perf-milestone-NN-<name>.md` file.
   - Work through tasks IN ORDER. As you complete each task,
     edit the milestone file to flip its `[ ]` to `[x]`. Don't
     skip ahead.
   - When you complete the LAST task in the milestone, **STOP**.
     Surface a clear notice -- `Milestone NN: <name> complete;
     ready for critique.` followed by a one-line summary of what
     landed -- and wait for the paired critique session before
     starting the next milestone. Do NOT chain milestones
     automatically. The critique is the primary milestone gate;
     user review may happen around it.
3. **Probe declaration**. External probes attach by hierarchical
   path; declare them in `docs/perf-plan/probes.toml` rather than
   editing the model. One TOML entry per metric, named so its key
   doubles as the stats `path_root`:

   ```toml
   [probes.stage1_in_stalls]
   kind = "stall"
   path = "top.pipe.stage1"
   port = 0
   ```

   The runtime resolves paths through the elaborated hierarchy at
   sim setup and attaches probes to the dispatch event stream;
   models stay untouched. If a metric requires a value that's
   currently a local inside `evaluate()`, flag the missing
   observable surface back to DM2d rather than embedding a probe
   here -- the framework's `enabled: bool` escape hatch on
   embedded probes exists only for the rare case where structural
   exposure is impractical, not as the default analysis path.
4. **Run-record discipline**. Each measurement is TWO tool calls:

   ```text
   run_cargo({command: "run", binary_args: ["--run-id", "<id>"]})
   record_run({description: "<id>", workload: "...", candidate: "...", study: "..."})
   ```

   The first invokes the project binary with the run-id; the
   second logs the run into `.sim-flow/experiments.db`. Run-ids
   should match the scheme declared in `perf-plan.md` (typically
   `baseline-<workload>` or `sweep-<param>-<value>`). After both
   calls succeed, flip the task to `[x]`. The critique verifies
   every cited run-id has a matching `experiments.db` row.
5. **Sweep discipline**. Use `sim-flow sweep <sweep.toml>` for
   parameter sweeps. The sweep TOML should reference the run-id
   pattern from the plan. Don't roll your own loops over single
   runs when a sweep config does the same job.
6. **Reporting**. Each report under `docs/analysis/<topic>.md`
   must:
   - Open with a summary table of measured-vs-target metrics.
   - Identify bottlenecks with supporting evidence (per-module
     stall counts, link utilization, queue occupancy, NOT
     speculation).
   - Cite the run-ids that back every number, so the data is
     reproducible.
   - Use distributions (p50 / p90 / p99) where appropriate, not
     just scalar summaries.
   - Conclude with the next optimization lever for any target
     that's missed.
7. **Target verification milestone**. For each row of
   `docs/targets.md`, the corresponding task should record:
   `target met / not met` + the run-id that produced the
   measurement. Mark `BLOCKER:`-eligible items in the report
   prose so the critique can flag them.

8. **Pre-stop hygiene** (every milestone, but especially when
   any Rust helpers / sweep glue / scratch binaries landed):
   `cargo fmt --check` AND `cargo clippy --all-targets -- -D
   warnings` are run AUTOMATICALLY by the orchestrator after
   you stop and surfaced to the next critique. Do NOT invoke
   them yourself; their results are authoritative when the
   critique sees them. A FAIL on either gets flagged as a
   BLOCKER and you'll re-enter the milestone with diagnostics
   inlined. For purely-markdown milestones (no new Rust code),
   the orchestrator's checks are cheap idempotent no-ops; you
   don't need to do anything special.

## Order, jumping, and deferring

`docs/impl-plan/plan-management.md` is the source of truth: task
states (`- [ ]` / `- [x]` / `- [-]` with `defer reason:`),
out-of-order work (`order swap:` sub-bullet), and additions
(`added:` sub-bullet). Read it before starting; the conventions
apply to perf-milestone task rows the same way they apply to
DM2c's implementation milestones.

DM4b-specific note: a deferred (`- [-]`) target-verification
row is allowed only when the target is genuinely out of scope
for this measurement run (e.g. "requires a workload not yet
written"). Deferring a target because the design misses it is
NOT acceptable -- leave the row `- [ ]` so the critique flags
it as a `BLOCKER:`.

## Coding Requirements

DM4b's deliverables are mostly markdown reports under
`docs/analysis/`, but any Rust helpers / sweep glue / scratch
binaries it writes MUST follow these rules. Markdown files
inherit the "no emojis" + "under 400 lines" rules; the rest are
Rust-specific.

- **Idiomatic Rust** (for any code DM4b authors). Prefer the
  standard idioms (`?` for error propagation, `Result` /
  `Option` over panics, iterators over manual loops, pattern
  matching over nested `if let`). Boring code beats clever code.
- **Data-oriented + memory-friendly**. Prefer concrete types
  over trait objects, owned data over indirection, contiguous
  storage over heap-of-heaps. Avoid premature
  `Arc<Mutex<_>>` indirection.
- **Functional where appropriate**. Small pure helpers,
  immutable bindings by default, `iter().map().filter().collect()`
  over mutable accumulators, exhaustive `match` for state
  machines.
- **No magic numbers or strings**. Workload names, run-id
  patterns, threshold values -- all named (`const`, enum
  variant, named struct field), not inlined.
- **No emojis** in code, markdown reports, doc strings, or log
  output. Reports rendered in dashboards rely on plain ASCII.
- **File size cap: under 400 lines** for every Rust source AND
  every report markdown under `docs/analysis/`. Split a long
  report along its natural axes (per-workload sections, per-
  topic files like `throughput.md` + `latency.md`) rather than
  growing one mega-report. The critique flags any file at or
  above 400 lines as `BLOCKER:`.

## Constraints

- Stay inside the plan. If a task in `perf-plan.md` turns out
  to be wrong or impossible, flag the issue rather than
  silently deviating. The plan is the contract.
- Do NOT skip the milestone stop-points; the user is meant to
  critique each one's runs / reports. Auto-mode should still
  honor the stop -- emit the "milestone complete" notice and
  let the critique run before advancing.
- Do NOT modify `docs/perf-plan/perf-plan.md`'s structure (only
  flip `[ ]` to `[x]` and append run-id / measurement notes
  inside task lines). Re-architecting the plan is DM4a's job.

## Output

{{ output_intro }}

- `docs/analysis/` populated with per-topic report markdown.
- At least one experiment run recorded in
  `.sim-flow/experiments.db` for this project (typically many).
- `cargo run -- --run-id <id>` invocations visible in the run
  log; run-ids match the plan's scheme.
- Every task in every `perf-milestone-NN-*.md` is `[x]` (or
  documented as deferred with a reason).

Milestone completion and step completion are different:

- After each milestone is complete, stop and wait for the paired
  milestone critique before starting the next milestone.
- After the final milestone is complete, stop for the final DM4b
  critique. That final critique is the end-to-end performance
  analysis / reporting gate, not the first serious review.

## Re-entry

If DM4b runs across multiple work + critique sessions (a milestone
critique flagged something, or the session was killed mid-run),
restart by walking the `docs/perf-plan/perf-milestone-NN-*.md`
files in numeric order. The first one with at least one `- [ ]`
row -- or any task whose run-id is missing from
`.sim-flow/experiments.db` -- is your current milestone, and you
start at the first such row in that file. Do NOT skip a
milestone just because its rows are all `[x]` -- if a cited
run-id has no row in `experiments.db`, the prior milestone's
claim of completeness was wrong; back up and reopen the
affected tasks before moving forward.

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM4b-critique.json`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
