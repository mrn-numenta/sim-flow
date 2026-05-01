# DM3a - Test Plan (work session)

You are executing step DM3a (Test Plan) of the Direct Modeling Flow.
Prerequisite: DM2d gate passed.

## Goal

Produce a written verification plan that DM3b (testbench
implementation) and DM3c (test execution + coverage) will execute
against. **You do NOT write any test code in this step.** The plan is
the contract: a detailed testbench architecture, an enumerated set of
tests grouped into smoke / edge / stress / random categories with
specific pass criteria, a coverage strategy using `cargo-tarpaulin`,
and a traceability table from each test back to a spec requirement
or `docs/targets.md` row.

## Inputs

Read these before writing the plan:

- `docs/plan/plan-management.md` -- the plan-file conventions
  (`[ ]` task / checklist format, milestone numbering scheme).
- `docs/spec.md` -- the specification.
- `docs/targets.md` -- quantitative targets DM3c must measure.
- `docs/analysis/decomposition.md` -- the operations whose
  correctness must be verified.
- `docs/analysis/pipeline-mapping.md` -- the pipeline shape that
  defines stage-by-stage observability points.
- `docs/analysis/data-movement.md` -- the payload shapes used by
  Sequencers / Drivers / Monitors.
- `docs/plan/plan.md` and `docs/plan/milestone-*.md` -- the
  implementation plan from DM2c, and what DM2d ended up landing.
- `src/` -- the model under test as it stands; references to
  modules, port names, and payload structs you'll wire to.

Reference material (read on demand):

- **Modeling guide -- testing chapter** (canonical): start at
  `lib:docs/modeling-guide/04-testing-models.md`. This describes
  the UVM-lite topology (Sequencer -> Driver -> DUT -> Monitor ->
  Scoreboard), the `SimEnvBuilder` wiring pattern, and the
  conventions all sim-models projects follow. Cite specific
  sections in the plan so DM3b knows which patterns to mirror.
- **Worked examples** that ship a testbench: walk
  `lib:examples/README.md` and pick at least two examples whose
  topology matches yours. Their `tests/` directories are the
  closest reference for what your testbench will look like.
- **Foundation framework** public API via the `fw:` prefix. Start with
  `fw:api/toc.md`, then read only the specific
  `fw:api/pages/.../*.md` files you need for types like `SimEnv`,
  `SimEnvBuilder`, `Sequencer`, `Driver`, `Monitor`, `Scoreboard`,
  and `Port`. Use `fw:src/prelude.rs` only as a secondary source when
  you need exact signatures or source-level examples; do not browse
  internal helpers.

## Procedure

1. Read every input above.
2. Check whether `docs/plan/test-plan.md` exists.
   - If yes, review it against `docs/plan/test-plan.md.tmpl` and fill in
     any missing or incomplete sections.
   - If no, copy `docs/plan/test-plan.md.tmpl` to
     `docs/plan/test-plan.md` and use that template as the required
     structure for this step.
3. **Design the testbench(es)**. The plan must specify, before
   listing any test:
   - Each Sequencer (one per stimulus class) -- name, payload
     type, what stimulus class it generates.
   - Each Driver (one per external interface) -- name, target
     port, handshake protocol (typically valid/ready).
   - Each Monitor (one per observable signal / port) -- name,
     observed port, which Scoreboard(s) it feeds.
   - Each Scoreboard (one per correctness invariant) -- name,
     invariant in plain English, monitor inputs, comparison
     strategy.
   - The `SimEnvBuilder` wiring -- which helper function returns
     a fully assembled `SimEnv` ready for the test layer.
   - References to specific modeling-guide chapters and example
     directories that the implementation will mirror. This is
     the UVM-lite contract for DM3b.
4. **Enumerate tests by category**. For every test in every
   category use this row format so DM3c can tick them off as
   it passes:

   ```markdown
   - [ ] `<test_name>` -- <one-sentence purpose>; pass criteria:
     <specific, measurable>; traces to: <spec section / target
     row / decomposition operation>
   ```

   Test names must be identifier-safe (DM3c will use them as
   Rust `#[test]` function names).

   Categories (ALL FOUR must appear; do not collapse them):

   - **Smoke** -- happy-path correctness + minimal liveness. At
     minimum: elaboration succeeds, basic data flow end-to-end,
     backpressure propagates, idle cycles produce no spurious
     outputs. Smoke tests must pass before any other category
     is attempted.
   - **Edge** -- boundary values and corner cases: zero / max /
     min / saturating-overflow inputs; empty pipeline; full
     buffers; single-element transit; back-to-back boundary
     transitions; reset mid-traffic; illegal-but-recoverable
     inputs (if applicable). Aim for one edge test per
     non-trivial decomposition operation.
   - **Stress** -- sustained or worst-case traffic patterns:
     long runs (1000+ cycles), full pipeline saturation,
     heavily-randomized backpressure, queue-depth limits,
     contention if the design has shared resources. Stress
     tests should exercise the targets in `docs/targets.md`.
   - **Random** -- constraint-randomized stimulus with fixed
     seeds for reproducibility. Each random test pins a seed
     in the test name (`<test>_seed_<N>`) so failures are
     reproducible. Plan at least one random test per Sequencer
     and one "soak" (multiple seeds, statistical) entry.

5. **Coverage strategy**. The plan must include:
   - Tool: `cargo-tarpaulin` (install once with
     `cargo install cargo-tarpaulin`).
   - Run command: `cargo tarpaulin --out Html --out Lcov
     --output-dir target/coverage` (or equivalent that produces
     both human-readable HTML and a machine-readable LCOV).
   - Threshold: minimum **90% line coverage** on `src/` (or a
     lower per-file target with explicit justification).
   - Exclusions: list any files / modules to exclude
     (`#[cfg(test)]` scaffolding, generated code, platform-gated
     paths) and the reason. DM3c will not be allowed to silently
     exclude paths not pre-approved here.
   - Report path: where DM3c will write the report (e.g.
     `docs/analysis/coverage.md` or
     `target/coverage/tarpaulin-report.html`); the report path
     must be referenced from the test plan so the gate can find
     it.

6. **Traceability**. Add a final section that maps:
   - Every functional requirement in `docs/spec.md` -> at least
     one test row above. Quote the requirement.
   - Every target in `docs/targets.md` -> at least one stress
     test row.
   - Every operation in `docs/analysis/decomposition.md` -> at
     least one smoke or edge test row.
   Reject vague mappings ("covered by overall flow"); each link
   must name a specific test from the enumeration above.

7. Use the template headings as the required document structure, but use
   engineering judgement about depth. Remove placeholder text as you
   replace it with real content. If a section truly does not apply, say
   so explicitly rather than leaving placeholder text in place.

## Output

`docs/plan/test-plan.md`. Free-form markdown body, but it must
contain (in this order, so the gate's regex checks pass):

1. A `## Testbench` section that names at least one of
   `Sequencer`, `Driver`, `Monitor`, `Scoreboard`.
2. A `## Smoke` section, a `## Edge` section, a `## Stress`
   section, and a `## Random` section, each containing at least
   one `- [ ]` checklist row.
3. A `## Coverage` section that mentions `tarpaulin`.
4. A `## Traceability` section.

## Constraints

- DO NOT write any test code. No `tests/` edits, no test fixtures,
  no `#[test]` annotations. Plan markdown only.
- DO NOT prescribe internal Foundation helpers (anything outside
  the curated public API reachable via `fw:`). The plan describes WHAT
  will be built and
  how it MAPS to spec requirements -- HOW each component is
  written is DM3b/DM3c's concern.
- DO NOT collapse categories ("smoke + stress combined") or skip
  the random category because the design is small. Every category
  has a separate verification purpose; the gate enforces all four.

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM3a-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
