# DM3a - Test Plan, Outline (critique session)

You are reviewing the DM3a test-plan OUTLINE.
{{ third_party_reviewer_note }} The outline is the contract DM3ad
will detail against; gaps here propagate as missing or mis-shaped
milestones.

## Inputs

- `docs/impl-plan/plan-management.md` -- plan-file conventions.
- `docs/test-plan/` -- the outline under review:
  - `test-plan.md` -- index. Testbench architecture +
    traceability + per-milestone scope blurbs + TOCs.
  - `tb-milestone-NN-<name>.md` -- one stub per testbench
    milestone (Scope / Components / Trace +
    `<!-- detail-pending -->` placeholder).
  - `test-milestone-NN-<name>.md` -- one stub per test-execution
    milestone (same shape).
  - `coverage.md` -- coverage strategy (full content, no stub).
- `docs/spec.md`
- `docs/targets.md`
- `docs/testbench.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/pipeline-mapping.md`
- `docs/analysis/data-movement.md`
- `src/`

## Evaluation

{{ critique_kinds }}

This critique reviews the OUTLINE, not per-milestone task
lists. Resist reviewing content that lives in
`<!-- detail-pending -->` placeholders; flag the ABSENCE of
one as a `BLOCKER:`, but don't critique missing tasks
themselves.

1. **Directory layout**. Does `docs/test-plan/` exist with
   `test-plan.md`, `coverage.md`, plus at least one
   `tb-milestone-NN-*.md` stub and one `test-milestone-NN-*.md`
   stub?
2. **Index file** (`test-plan.md`):
   - Has a verification-strategy overview.
   - Has a testbench architecture summary naming Sequencers /
     Drivers / Monitors / Scoreboards.
   - Has TWO TOCs: one for `tb-milestone-*.md` files, one for
     `test-milestone-*.md` files. Each TOC entry has a 1-2
     sentence scope blurb.
   - Has a `## Traceability` section linking each spec
     requirement / target / decomposition operation to a
     SPECIFIC milestone file (not a specific test name -- that's
     DM3ad's level of detail). Vague mappings ("covered by
     overall flow") = `BLOCKER:`.
3. **Stub structure** (every `tb-milestone-*.md` and
   `test-milestone-*.md`):
   - Contains the literal `<!-- detail-pending -->` marker.
     Missing marker = `BLOCKER:` (DM3ad's gate keys on it).
   - Has Scope, Components/Tests, Trace sections.
   - Scope blurb names the milestone's slice and acceptance
     criterion (NOT a task list -- that's the detail step's
     job).
   - Trace section points at SPECIFIC entries in spec.md /
     targets.md / decomposition.md / testbench.md. Vague trace
     = `UNRESOLVED:`; missing trace entirely = `BLOCKER:`.
4. **Milestone breakdown -- testbench (`tb-milestone-*.md`)**:
   - Numbers contiguous (`01`, `02`, ...).
   - Covers payloads, Sequencers, Drivers, Monitors,
     Scoreboards, SimEnvBuilder, smoke test (in some
     reasonable grouping).
5. **Milestone breakdown -- test execution
   (`test-milestone-*.md`)**:
   - Order is fixed: `smoke (01) -> edge (02) -> stress (03) ->
     random (04) -> coverage (05)`. Lexicographic filename
     order MUST match this order. Reordering or interleaving =
     `BLOCKER:`.
   - All four mandatory categories present (smoke / edge /
     stress / random) plus coverage. Skipping a category =
     `BLOCKER:` even if the design is small. For combinational
     designs without flow-control, the smoke milestone may
     include a one-line `RESOLVED: design has no flow-control
     surface, ...` note in its Scope.
   - Split-category files use letter suffix (`02a-edge-x`,
     `02b-edge-y`) and an axis tag in the name. Documented
     splits = OK; un-documented = `UNRESOLVED:`.
   - One milestone file never mixes categories. Mixing =
     `BLOCKER:`.
6. **Coverage strategy** (`coverage.md`):
   - Names `cargo-llvm-cov` as the tool.
   - States a numeric line-coverage threshold (90% line coverage
     on `src/model/` is the default; other values need prose
     justification).
   - Has Exclusions / Run Command / Report Output sections.
   - Run Command is in a CLOSED triple-backtick code fence.
7. **Stub leakage**: does ANY stub leak per-task content
   (concrete `- [ ]` rows, specific test bodies, algorithm
   details) into its Scope or Trace? That's the detail step's
   job; stubs describe WHAT, not specific HOW. Flag offending
   lines as `BLOCKER:`.
8. **Pre-empting DM3b/DM3c**: does any stub or the index
   prescribe internal Foundation helpers / specific framework
   APIs the implementer should use? The plan describes WHAT
   will be tested and how it MAPS to spec; HOW each test is
   implemented is DM3b/DM3c's concern. Mark `BLOCKER:` for
   prescription.

## Output

{{ output_intro }}

{{ critique_output_block }}