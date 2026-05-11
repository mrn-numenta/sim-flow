# DM3a - Test Plan, Outline (work session)

You are executing step DM3a (Test Plan, OUTLINE) of the Direct
Modeling Flow. Prerequisite: DM2d gate passed.

## Goal

Produce the TEST PLAN OUTLINE for DM3b (testbench impl) and DM3c
(test execution + coverage). **You write the index
(`test-plan.md`), the coverage strategy (`coverage.md`), and
STUB files for every milestone.** You do NOT yet write the
per-milestone task lists -- that's DM3ad's job, walking each
stub one at a time.

The split exists so a non-trivial design (RISC-V core, pipelined
SoC) can name its testbench milestones + test categories in one
session even when the per-milestone task lists are too large to
fit alongside the spec / decomposition / targets in one prompt.

## Inputs

Read these before writing the outline:

- `docs/impl-plan/plan-management.md` -- plan-file conventions
  (milestone numbering, the 10-task-per-milestone cap).
- `docs/spec.md` -- the specification.
- `docs/targets.md` -- quantitative targets DM3c must measure.
- `docs/testbench.md` -- DM1's verification strategy and the
  named `lib:examples/<NN-name>/test/` baseline DM3b will mirror.
- `docs/analysis/decomposition.md` -- operations whose
  correctness must be verified.
- `docs/analysis/pipeline-mapping.md` -- pipeline shape and
  observability points.
- `docs/analysis/data-movement.md` -- payload shapes for
  Sequencers / Drivers / Monitors.
- `docs/impl-plan/plan.md` -- the implementation plan.
- `src/` -- the model under test as it stands.

Reference material (read on demand):

- `lib:docs/modeling-guide/04-testing-models.md` -- UVM-lite
  topology, `SimEnvBuilder` patterns.
- The named baseline from `docs/testbench.md`'s
  `## Implementation Baseline` -- the canonical reference DM3b
  will mirror.
- `fw:api/toc.md` and specific `fw:api/pages/...` files for
  exact types (`SimEnv`, `SimEnvBuilder`, `Sequencer`,
  `Driver`, `Monitor`, `Scoreboard`, `Port`).

## Procedure

1. Read each input above.

2. **Design the testbench** (lives in `test-plan.md`'s testbench
   section, before the milestone TOCs):

   - Each Sequencer (one per stimulus class) -- name, payload
     type, stimulus class.
   - Each Driver (one per external interface) -- name, target
     port, handshake protocol.
   - Each Monitor (one per observable signal / port) -- name,
     observed port, which Scoreboard(s) it feeds.
   - Each Scoreboard (one per correctness invariant) -- name,
     invariant in plain English, monitor inputs, comparison
     strategy.
   - The `SimEnvBuilder` wiring -- helper function name returning
     a fully assembled `SimEnv`.

   This is the UVM-lite contract for DM3b.

3. **Decide the testbench milestone breakdown** (DM3b's slices,
   `tb-milestone-NN-*.md`). Typical shape for a small-to-medium
   design:

   - `tb-milestone-01-payloads-and-sequencers.md`
   - `tb-milestone-02-drivers.md`
   - `tb-milestone-03-monitors.md`
   - `tb-milestone-04-scoreboards.md`
   - `tb-milestone-05-simenv-and-smoke.md`

   Combine, split, or rename as the design demands. The numbers
   are contiguous (`01`, `02`, ...) and each milestone is
   reviewable in isolation.

4. **Decide the test-execution milestone breakdown** (DM3c's
   slices, `test-milestone-NN-*.md`). The four canonical
   categories are required and each maps to one or more
   milestones:

   - **Smoke** -> `test-milestone-01-smoke*.md` -- happy-path +
     liveness; elaboration, basic data flow, backpressure (when
     applicable), idle cycles produce no spurious outputs.
   - **Edge** -> `test-milestone-02-edge*.md` -- boundary values,
     corner cases.
   - **Stress** -> `test-milestone-03-stress*.md` -- sustained
     traffic; MUST exercise targets in `docs/targets.md`.
   - **Random** -> `test-milestone-04-random*.md` -- constraint-
     randomized stimulus with seeded test names.
   - **Coverage** -> `test-milestone-05-coverage.md` -- runs
     `cargo-tarpaulin`, records measurement, addresses gaps.

   **Order is fixed**: smoke (01) -> edge (02) -> stress (03) ->
   random (04) -> coverage (05). DM3c walks them in
   lexicographic filename order. When a category exceeds the
   ~10-task cap a milestone normally holds, split with a letter
   suffix (`02a`, `02b`) and an axis tag in the name. Don't
   collapse categories or skip random because the design is
   small.

5. **Write `docs/test-plan/test-plan.md`** (the index):

   - Brief overview of the verification strategy.
   - Testbench architecture summary (Sequencers / Drivers /
     Monitors / Scoreboards as per step 2).
   - TWO TOCs: one for `tb-milestone-NN-*.md` files, one for
     `test-milestone-NN-*.md` files. Each TOC entry has a
     1-2 sentence scope blurb (NOT a task list -- that's the
     stub's job; the index just gives an overview).
   - **Traceability** section mapping:
     - Every functional requirement in `docs/spec.md` -> a
       SPECIFIC milestone (by file name). DM3ad later refines
       to specific task names.
     - Every target in `docs/targets.md` -> a stress milestone.
     - Every operation in `docs/analysis/decomposition.md` -> a
       smoke or edge milestone.

6. **Write `docs/test-plan/coverage.md`** by copying the
   project's `docs/test-plan/coverage.md.tmpl` template body
   verbatim into the live file with `write_file`, then editing
   the live file to fill in placeholders. Required content:

   - **Tool**: `cargo-tarpaulin`.
   - **Threshold**: minimum **90% line coverage** on
     `src/model/` (or a different target with explicit
     justification in prose).
   - **Exclusions**: list any files / modules to exclude with a
     one-sentence prose reason per entry.
   - **Run Command**: typically `cargo tarpaulin --out Html
     --out Lcov --output-dir target/coverage` plus relevant
     `--exclude-files` flags. CLOSED triple-backtick code fence.
   - **Report Output**: specific file path
     (`target/coverage/lcov.info`).

7. **Write one stub per milestone** at
   `docs/test-plan/tb-milestone-NN-<name>.md` AND
   `docs/test-plan/test-milestone-NN-<name>.md`. Each stub uses
   this template:

   ```markdown
   # <category-prefix>-Milestone NN: <Name>

   ## Scope

   <1-paragraph description of what this milestone covers --
   which testbench components / which tests / which spec area,
   ending with the acceptance criterion.>

   ## Components / Tests

   <For tb-milestone: list the testbench components this
   milestone delivers (e.g. "1 Sequencer + 2 Drivers + the
   payload structs they share"). Files: name the
   `tests/testbench/<file>.rs` paths.

   For test-milestone: list the test categories / classes (e.g.
   "edge cases for the averaging stage: zero / max / min on
   each channel"). Files: name the `tests/<category>/<test>.rs`
   paths.>

   ## Trace

   - spec.md: <which requirements>
   - targets.md: <which target rows, if any>
   - decomposition.md: <which operations>
   - testbench.md: <observability hooks, if any>

   ## Tasks

   <!-- detail-pending: DM3ad replaces this section with the full
   task list per `docs/impl-plan/plan-management.md`. The scope +
   components + trace above are the contract this milestone
   delivers; expand into concrete `- [ ]` rows naming files,
   symbols, pass criteria. -->
   ```

   The literal comment `<!-- detail-pending -->` is load-
   bearing: the orchestrator's gate fails until every stub has
   been detailed (placeholder removed). Keep it verbatim. Do
   NOT add task rows to the stub.

   Two-digit zero-padded numbers; letter suffixes for
   split-category files (`test-milestone-02a-edge-*.md`).

## Output

**Use the path as the fence info-string, verbatim.** Opening
the fence with a language tag (`markdown`, `json`, `toml`, `rust`,
`yaml`, `text`, `md`, `rs`, `yml`, `txt`) means the body is
**silently dropped** -- the file never lands on disk, the gate
fails, and the work session burns its retry budget. See
`_conventions/fenced-blocks.md` ("Language-tag info-strings are
SILENTLY DROPPED") for the failure mode in detail. If you don't
remember the exact path, run `tool:read_file` / `tool:list_dir`
to discover it -- never guess `\`\`\`markdown` as a fallback.

A directory at `docs/test-plan/` containing:

- `test-plan.md` -- index with testbench architecture +
  per-milestone scope blurbs + traceability section.
- `coverage.md` -- full coverage strategy (no stub).
- `tb-milestone-NN-<name>.md` -- stub per testbench milestone.
- `test-milestone-NN-<name>.md` -- stub per test-execution
  milestone.

Gate-significant rules:

- `test-plan.md` mentions Sequencer / Driver / Monitor /
  Scoreboard AND references `spec.md` or `targets.md`.
- At least one `tb-milestone-NN-*.md` and one
  `test-milestone-NN-*.md` file exists.
- `coverage.md` mentions `tarpaulin`.
- Every stub contains the literal `<!-- detail-pending -->`
  marker (DM3ad's gate keys on it).

## Constraints

- DO NOT write any test code. Plan markdown only.
- DO NOT write the per-milestone task list -- DM3ad does that.
- DO NOT remove the `<!-- detail-pending -->` placeholder from
  the stubs.
- DO NOT collapse categories or skip random because the design
  is small. Every category has a separate verification purpose.
- DO NOT cite specific framework APIs in stubs; the plan
  describes WHAT, not HOW.

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM3a-critique.md`; the critique is a distinct
task. Do not `/exit` on your own -- the user and the
orchestrator control session boundaries.
