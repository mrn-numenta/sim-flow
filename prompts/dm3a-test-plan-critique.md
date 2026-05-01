# DM3a - Test Plan (critique session)

You are reviewing the DM3a test plan. Treat it as work produced by
a third party even if you produced it yourself earlier in this
conversation -- the independent-review property depends on you
bracketing any prior reasoning rather than leaning on it. The plan
is the contract DM3b (testbench implementation) and DM3c (test
execution + coverage) will execute against; gaps here propagate as
missing tests or insufficient coverage downstream. Do not modify
the plan; evaluate it and write the critique file.

## Inputs

- `docs/plan/plan-management.md` -- plan-file conventions.
- `docs/plan/test-plan.md` -- the plan under review.
- `docs/spec.md`
- `docs/targets.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/pipeline-mapping.md`
- `docs/analysis/data-movement.md`
- `src/` -- the model under test.

## Evaluation

Prefix gate-blocking issues with `BLOCKER:` (DM3b cannot proceed
until fixed). Prefix informational notes with `UNRESOLVED:`. The
orchestrator fails the DM3a gate on `BLOCKER:` lines only.

1. **Testbench design**. Does the plan describe at least one
   Sequencer, Driver, Monitor, and Scoreboard? Are payload types
   and target ports named explicitly? Is the `SimEnvBuilder`
   wiring described concretely enough that DM3b can implement it
   without making architectural decisions?
2. **UVM-lite topology**. Does the testbench architecture honor
   Sequencer -> Driver -> DUT -> Monitor -> Scoreboard? Does the
   plan explicitly cite chapters of `lib:docs/modeling-guide/`
   (especially `04-testing-models.md`) and at least one example
   under `lib:examples/` so DM3b knows which patterns to mirror?
   Reject hand-rolled testbench architectures that bypass UVM-lite
   without justification.
3. **Test categories present**. Does the plan have separate
   `## Smoke`, `## Edge`, `## Stress`, and `## Random` sections,
   each with at least one `- [ ]` row? A "small design only needs
   smoke" justification is not acceptable -- the four categories
   exercise different failure modes.
4. **Test row format**. Each row uses the
   `- [ ] <test_name> -- <purpose>; pass criteria: <criteria>;
   traces to: <ref>` shape with an identifier-safe test name and
   measurable pass criteria. Reject vague pass criteria
   ("reasonable", "fast", "looks correct").
5. **Smoke coverage**. Are the four required smoke tests present
   (elaboration, basic data flow, backpressure propagation,
   idle cycles)?
6. **Edge coverage**. Is there at least one edge test per
   non-trivial operation in `decomposition.md`? Are obvious
   boundaries (zero / max / min / saturation, empty / full
   buffers, single-element transit, reset mid-traffic) covered?
7. **Stress coverage**. Does the stress section exercise every
   target in `docs/targets.md`? Are run lengths concrete
   (1000+ cycles, named iteration counts) rather than vague
   ("for a while")?
8. **Random coverage**. Does each random test pin a seed in its
   name (`<test>_seed_<N>`) for reproducibility? Is there at
   least one random test per Sequencer plus a seed-sweep "soak"
   entry?
9. **Coverage strategy**. Does the plan name `cargo-tarpaulin`
   explicitly, give the run command (`cargo tarpaulin --out Html
   --out Lcov --output-dir target/coverage` or equivalent),
   declare a numeric line-coverage threshold (default 90%), list
   any exclusions with reasons, and name the report path DM3c
   will write to? Reject "we'll figure out coverage later".
10. **Traceability**. Does every requirement in `docs/spec.md`
    map to at least one test row (quoting the requirement)? Does
    every target in `docs/targets.md` map to at least one stress
    test? Does every operation in `decomposition.md` map to at
    least one smoke or edge test? Reject vague mappings
    ("covered by overall flow"); each link must name a specific
    test from the enumeration.
11. **Scope**. Does the plan stay out of test-code territory? It
    should describe WHAT and HOW MUCH, not HOW each test is
    implemented. Reject embedded code snippets, `#[test]`
    annotations, or implementation pseudocode.

## Output

Write `docs/critiques/DM3a-critique.md`. Free-form markdown body;
only line-prefix tokens (`BLOCKER:`, `UNRESOLVED:`, `RESOLVED:`)
are inspected by the gate.
