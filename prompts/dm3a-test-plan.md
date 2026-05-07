# DM3a - Test Plan (work session)

You are executing step DM3a (Test Plan) of the Direct Modeling Flow.
Prerequisite: DM2d gate passed.

## Goal

Produce a written verification plan that DM3b (testbench
implementation) and DM3c (test execution + coverage) will execute
against. **You do NOT write any test code in this step.** The plan
is the contract: testbench architecture, two parallel milestone
sequences (one for DM3b, one for DM3c) that walk the work in
small reviewable chunks, a coverage strategy using
`cargo-tarpaulin`, and a traceability table from each test back
to a spec requirement or `docs/targets.md` row.

DM3b and DM3c each consume their own milestone slice and stop for
a paired critique after every milestone. Keeping the milestones
small and focused is the whole point: each one corresponds to ~one
review-sized chunk of work, not "the whole testbench" or "every
edge test."

## Inputs

Read these before writing the plan:

- `docs/impl-plan/plan-management.md` -- the plan-file conventions
  (`[ ]` task / checklist format, milestone numbering, the
  10-task-per-milestone cap).
- `docs/spec.md` -- the specification.
- `docs/targets.md` -- quantitative targets DM3c must measure.
- `docs/testbench.md` -- DM1's verification strategy and the
  named `lib:examples/<NN-name>/test/` baseline DM3b will mirror.
- `docs/analysis/decomposition.md` -- the operations whose
  correctness must be verified.
- `docs/analysis/pipeline-mapping.md` -- the pipeline shape that
  defines stage-by-stage observability points.
- `docs/analysis/data-movement.md` -- the payload shapes used by
  Sequencers / Drivers / Monitors.
- `docs/impl-plan/plan.md` and `docs/impl-plan/milestone-*.md` -- the
  implementation plan from DM2c, and what DM2d ended up landing.
- `src/` -- the model under test as it stands; references to
  modules, port names, and payload structs you'll wire to.

Reference material (read on demand):

- **Modeling guide -- testing chapter** (canonical):
  `lib:docs/modeling-guide/04-testing-models.md`. UVM-lite
  topology, `SimEnvBuilder` patterns, conventions all sim-models
  projects follow. Cite specific sections in the plan so DM3b
  knows which patterns to mirror.
- **Worked examples** with testbenches: prefer the named baseline
  from `docs/testbench.md`'s `## Implementation Baseline` section
  if present; that's the canonical reference DM3b will mirror.
- **Foundation framework** public API via the `fw:` prefix.
  `fw:api/toc.md` -> the specific `fw:api/pages/.../*.md` files
  for exact types like `SimEnv`, `SimEnvBuilder`, `Sequencer`,
  `Driver`, `Monitor`, `Scoreboard`, `Port`. Use `fw:src/prelude.rs`
  only as a secondary source when you need exact source-level
  signatures or examples.

## Procedure

1. Read every input above.

2. The output is a directory at `docs/test-plan/` with two parallel
   milestone sequences plus an index and a coverage strategy file:

   - `test-plan.md` -- the index. One brief overview, the testbench
     architecture summary, and TWO tables of contents: one pointing
     at `tb-milestone-NN-<name>.md` files (DM3b's milestones) and
     one pointing at `test-milestone-NN-<name>.md` files (DM3c's
     milestones).
   - `tb-milestone-NN-<name>.md` -- one file per testbench-impl
     milestone DM3b will work through.
   - `test-milestone-NN-<name>.md` -- one file per test-execution
     milestone DM3c will work through.
   - `coverage.md` -- coverage strategy (tool, threshold,
     exclusions, run command, report path).

   Re-running this step against an existing directory is allowed --
   read each existing file and fill in / refine missing sections;
   do not silently delete milestone files the prior pass produced.

3. **Design the testbench**. The plan must specify (in
   `test-plan.md`'s testbench section, before listing milestones):

   - Each Sequencer (one per stimulus class) -- name, payload type,
     stimulus class.
   - Each Driver (one per external interface) -- name, target port,
     handshake protocol.
   - Each Monitor (one per observable signal / port) -- name,
     observed port, which Scoreboard(s) it feeds.
   - Each Scoreboard (one per correctness invariant) -- name,
     invariant in plain English, monitor inputs, comparison
     strategy.
   - The `SimEnvBuilder` wiring -- helper function name returning a
     fully assembled `SimEnv`.

   This is the UVM-lite contract for DM3b.

4. **Decompose DM3b's work into testbench milestones** named
   `tb-milestone-NN-<name>.md`. Each file MUST hold no more than
   10 `- [ ]` task rows (`plan-management.md` enforces the cap).
   DM3b writes its scaffolding under `tests/testbench/<file>.rs`
   (a SUBDIRECTORY, one file per concern; never one big
   `tests/testbench.rs` or `tests/tests.rs`). Typical breakdown
   for a small-to-medium design:

   - `tb-milestone-01-payloads-and-sequencers.md` -- payload
     structs + one payload-aware Sequencer per stimulus class.
     Tasks point at `tests/testbench/payloads.rs` and
     `tests/testbench/sequencers.rs`.
   - `tb-milestone-02-drivers.md` -- one Driver per external
     interface. Tasks point at `tests/testbench/drivers.rs`.
   - `tb-milestone-03-monitors.md` -- one Monitor per observable
     port. Tasks point at `tests/testbench/monitors.rs`.
   - `tb-milestone-04-scoreboards.md` -- one Scoreboard per
     invariant + reference-model helpers. Tasks point at
     `tests/testbench/scoreboards.rs` (split into
     `tests/testbench/scoreboards/<name>.rs` if the file would
     exceed 400 lines; the per-file 400-line cap from the work
     prompt's Coding Requirements is a hard rule).
   - `tb-milestone-05-simenv-and-smoke.md` -- the
     `SimEnvBuilder` helper at `tests/testbench/env.rs` (with
     `tests/testbench/mod.rs` re-exporting siblings) + the basic
     data-flow smoke test at `tests/smoke/basic_data_flow.rs`
     (one file per test; lives in `tests/smoke/`, not under
     `tests/testbench/`).

   Combine, split, or rename these as the design demands -- the
   important rules are: each file ≤10 tasks, the numbers are
   contiguous (`01`, `02`, ...), each milestone is reviewable in
   isolation, and the file-layout convention above is honored.
   For a tiny design with no Scoreboard invariants, merging
   milestones 04 and 05 is fine; document why.

   Each task names a concrete artifact path under
   `tests/testbench/` (or `tests/smoke/` for the smoke test):

   ```markdown
   - [ ] `tests/testbench/<file>.rs::<symbol>` -- <one-sentence
     purpose>; mirrors: `lib:examples/<NN-name>/test/<file>`;
     traces to: `<spec section / target row / decomposition op>`
   ```

5. **Decompose DM3c's work into test-execution milestones** named
   `test-milestone-NN-<name>.md`. Each file ≤10 tasks. The four
   canonical categories (smoke, edge, stress, random) are each
   required and each maps to **one or more consecutive
   milestones** -- a category is never collapsed away, and a
   single milestone file never mixes categories. Plus a coverage
   milestone closes DM3c.

   **Order is fixed**: the categories MUST appear in the
   numbering as `smoke (01) -> edge (02) -> stress (03) ->
   random (04) -> coverage (05)`. DM3c walks them in lexicographic
   filename order, which IS this order. Smoke comes first because
   nothing else is meaningful if basic data flow is broken; edge
   comes second because corner-case fixes often surface from
   smoke pass; stress and random come after because they consume
   the most cycles and benefit most from a stable design; coverage
   is last because it measures the cumulative test suite. Do NOT
   reorder, swap, or interleave categories.

   Mandatory category mapping:

   - **Smoke** -> `test-milestone-01-smoke*.md`. Happy-path
     correctness + minimal liveness. At minimum: elaboration
     succeeds, basic data flow end-to-end, backpressure
     propagates, idle cycles produce no spurious outputs. Smoke
     must pass before any other category runs. For combinational
     designs with no flow-control surface, write a one-line
     `RESOLVED: design has no flow-control surface, backpressure
     / idle entries do not apply` inside the smoke milestone --
     don't omit the file.
   - **Edge** -> `test-milestone-02-edge*.md` (one or more
     files). Boundary values and corner cases: zero / max / min
     / saturating-overflow inputs; empty pipeline; full buffers;
     single-element transit; back-to-back boundary transitions;
     reset mid-traffic; illegal-but-recoverable inputs (if
     applicable). Aim for one edge test per non-trivial
     decomposition operation.
   - **Stress** -> `test-milestone-03-stress*.md` (one or more
     files). Sustained or worst-case traffic patterns: long
     runs (1000+ cycles), full pipeline saturation,
     heavily-randomized backpressure, queue-depth limits,
     contention if the design has shared resources. Stress
     milestones MUST exercise the targets in `docs/targets.md`.
   - **Random** -> `test-milestone-04-random*.md` (one or more
     files). Constraint-randomized stimulus with fixed seeds for
     reproducibility. Each random test pins a seed in the test
     name (`<test>_seed_<N>`) so failures are reproducible. Plan
     at least one random test per Sequencer plus one "soak"
     (multiple seeds, statistical) entry.
   - **Coverage** -> `test-milestone-05-coverage.md`. Run
     `cargo-tarpaulin` per `coverage.md`; record the measured
     percentage and report path in `test-plan.md`'s
     `## Coverage` section; address any uncovered lines (add
     tests or extend exclusions list with justification).

   **Splitting rules when a category exceeds the 10-task cap**:

   - Add a letter suffix to the NN field and an axis tag to the
     name: `test-milestone-02a-edge-arithmetic.md`,
     `test-milestone-02b-edge-flow-control.md`. Letters are
     contiguous (`a`, `b`, `c`, ...) and the axis tag names what
     each split covers.
   - All split files for a category run before the next category
     begins (DM3c walks them in lexicographic order:
     `02a` before `02b` before `03`).
   - Document the split axis in `test-plan.md`'s milestone TOC so
     the reviewer can see why the category was split that way.

   **Forbidden**: a single milestone file that mixes categories
   (e.g. some smoke and some edge rows in the same file). The
   per-milestone critique pattern depends on each milestone
   reviewing one slice of one category.

   Each task row format names the test AND its destination file
   (DM3c writes one test per file under `tests/<category>/`):

   ```markdown
   - [ ] `tests/<category>/<test_name>.rs::<test_name>` --
     <one-sentence purpose>; pass criteria: <specific,
     measurable>; traces to: <spec section / target row /
     decomposition operation>
   ```

   Test names must be identifier-safe (DM3c uses them as Rust
   `#[test]` function names AND as filenames). Random tests pin
   a seed in the name AND filename:
   `tests/random/<test>_seed_<N>.rs::<test>_seed_<N>`.

   For purely combinational designs with no flow-control surface,
   write a one-line `RESOLVED: design has no flow-control surface,
   backpressure / idle entries do not apply` note inside
   `test-milestone-01-smoke.md` instead of forcing those entries --
   the critique recognizes the RESOLVED line.

6. **Coverage strategy** lives in `docs/test-plan/coverage.md`.
   The project ships a template at
   `docs/test-plan/coverage.md.tmpl` with the required section
   structure (`## Tool`, `## Threshold`, `## Exclusions`,
   `## Run Command`, `## Report Output`). **Use the
   copy-then-fill pattern from DM1**:

   - If `docs/test-plan/coverage.md` does not exist (fresh DM3a
     run), copy the template body verbatim into the live file
     first using `write_file`, THEN edit the live file to fill
     in placeholders. Do NOT write `coverage.md` from scratch
     prose -- the template's section structure is the contract
     DM3c reads, and earlier runs failed when the agent
     produced minimal hand-rolled coverage.md files missing
     Threshold / Exclusions-with-prose / Report-Path sections.
   - If the live file already exists from a prior pass, treat
     it like the existing-file path in DM1 (`docs/targets.md`):
     read both the live file and the `.tmpl`, fill in any
     section that's missing from the live file, do not delete
     existing content unless it's a placeholder.

   Required content per section (replace the template's
   placeholder text with concrete values):

   - **Tool**: `cargo-tarpaulin` (install with
     `cargo install cargo-tarpaulin`).
   - **Threshold**: minimum **90% line coverage** on
     `src/model/` (or a different target with explicit
     justification IN PROSE -- do not silently lower it).
   - **Exclusions**: list any files / modules to exclude
     (`#[cfg(test)]` scaffolding, generated code, platform-
     gated paths) and a one-sentence prose reason per entry.
     DM3c will not be allowed to silently exclude paths not
     pre-approved here.
   - **Run Command**: typically `cargo tarpaulin --out Html
     --out Lcov --output-dir target/coverage` plus
     `--exclude-files` flags for the entries above. The
     command MUST sit inside a CLOSED triple-backtick code
     fence -- earlier runs produced unclosed fences that
     broke downstream parsing.
   - **Report Output**: name the specific report file DM3c
     will write to (typically `target/coverage/lcov.info`).
     Do NOT just name a directory.

7. **Traceability** lives in the index `test-plan.md`. Add a
   `## Traceability` section that maps:

   - Every functional requirement in `docs/spec.md` -> at least
     one task row in some milestone file. Quote the requirement
     and name the task as `<milestone-file>::<test_name>`.
   - Every target in `docs/targets.md` -> at least one row in a
     `test-milestone-03-stress*.md`.
   - Every operation in `docs/analysis/decomposition.md` -> at
     least one row across smoke / edge milestones.

   Reject vague mappings ("covered by overall flow"); each link
   must name a specific task in a specific milestone file.

8. Use engineering judgement about depth. Each milestone file is
   focused -- don't pad with prose that belongs in the index.
   Remove placeholder text as you replace it with real content.
   If a milestone truly does not apply (e.g. no random tests
   needed), write an explicit RESOLVED line inside the file
   instead of leaving it empty or deleting the file.

## Output

A directory at `docs/test-plan/` containing:

- `test-plan.md` -- index. Testbench architecture (Sequencer /
  Driver / Monitor / Scoreboard) + traceability table back to
  `docs/spec.md` and `docs/targets.md` + TOCs pointing at the
  milestone files.
- `tb-milestone-NN-<name>.md` -- one or more testbench-impl
  milestone files (DM3b's slices). Numbers contiguous.
- `test-milestone-NN-<name>.md` -- one or more test-execution
  milestone files (DM3c's slices). Numbers contiguous.
- `coverage.md` -- coverage strategy (must mention `tarpaulin`).

Gate-significant content rules (the orchestrator's structural
checks pass when):

- `test-plan.md` mentions at least one of `Sequencer`, `Driver`,
  `Monitor`, `Scoreboard` AND references `spec.md` or `targets.md`
  (the traceability link).
- At least one `tb-milestone-NN-*.md` file exists.
- At least one `test-milestone-NN-*.md` file exists.
- At least one `- [ ]` row exists across the milestone files.
- `coverage.md` mentions `tarpaulin`.
- No milestone file has more than 10 `- [ ]` rows
  (the critique enforces this).

## Constraints

- DO NOT write any test code. No `tests/` edits, no test fixtures,
  no `#[test]` annotations. Plan markdown only.
- DO NOT prescribe internal Foundation helpers (anything outside
  the curated public API reachable via `fw:`). The plan describes
  WHAT will be built and how it MAPS to spec requirements -- HOW
  each component is written is DM3b/DM3c's concern.
- DO NOT collapse categories ("smoke + stress combined") or skip
  random because the design is small. Every category has a
  separate verification purpose; the critique enforces all four.
- DO NOT exceed 10 `- [ ]` rows in any milestone file. Split
  oversized categories into per-axis files (e.g.
  `test-milestone-02a-edge-arithmetic.md`,
  `test-milestone-02b-edge-flow-control.md`).

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM3a-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
