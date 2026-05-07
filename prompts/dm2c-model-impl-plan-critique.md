# DM2c - Implementation Plan (critique session)

You are reviewing the DM2c implementation plan. Treat it as work
produced by a third party even if you produced it yourself earlier
in this conversation -- the independent-review property depends on
you bracketing any prior reasoning rather than leaning on it. The
plan is the contract DM2d will execute against; gaps here
propagate forward as missing code or thrash during implementation.
Do not modify the plan; evaluate it and write the critique file.

## Inputs

- `docs/impl-plan/plan-management.md` -- plan-file conventions.
- `docs/impl-plan/plan.md` -- plan index + TOC.
- `docs/impl-plan/milestone-*.md` -- per-milestone task lists.
- `docs/spec.md`
- `docs/targets.md`
- `docs/testbench.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/pipeline-mapping.md`
- `docs/analysis/data-movement.md`

## Evaluation

Prefix gate-blocking issues with `BLOCKER:` (DM2d cannot proceed
until fixed). Prefix informational notes -- nits, follow-up
questions, things DM2d can work around -- with `UNRESOLVED:`. The
orchestrator fails the DM2c gate on `BLOCKER:` lines only.

**Finding-marker grammar.** The gate parses lines starting with
`BLOCKER:` / `RESOLVED:` / `UNRESOLVED:` (case-insensitive,
plural OK) optionally preceded by list markers (`-`, `*`, `+`,
`>`), heading markers (`#`+), bold/underline (`**` / `__`), and
one decoration glyph (e.g. `❌` `✅`). Headings DO match
(`### BLOCKER: ...`); section titles describing a blocker
without a colon-after-keyword (e.g. `### BLOCKER 1 - title`)
do NOT match -- they're prose. Mid-sentence mentions do NOT
match. ONLY the keyword-colon shape is a finding; pick the form
deliberately.

1. Does `docs/impl-plan/plan.md` follow the conventions in
   `plan-management.md`? Is there an overview and a TOC pointing at
   each milestone file?
2. Are milestones named `Milestone NN: <description>` and saved as
   `milestone-NN-<name>.md`? Are the numbers contiguous?
3. Is every task a `[ ]`-prefixed bullet that names a concrete
   artifact (file path, module name, payload struct name)? Reject
   vague tasks like "implement the pipeline" or "write tests".
4. Does every operation in `decomposition.md` map to at least one
   task? Quote the operation name and the task that covers it.
5. Does every payload in `data-movement.md` map to at least one
   task that produces or consumes it?
6. Is the milestone ordering correct -- payload types before
   modules, skeletons + connectivity before per-stage logic, logic
   before its tests? Flag tasks whose dependencies live in later
   milestones.
7. Does the plan cover the elaboration smoke test, the basic
   data-flow smoke test, AND any flow-control / idle-cycle tests
   **explicitly required by `docs/testbench.md`** for this design?
   When the design is purely combinational with no ready/valid or
   stall semantics, the absence of backpressure / idle-cycle tests
   is **RESOLVED**, not BLOCKER, provided the plan contains a
   one-line note acknowledging the choice (e.g. "design has no
   flow-control surface, backpressure / idle tests do not apply").
   Per-module unit tests should cover at least one representative
   input per non-trivial module.
8. Does the plan account for target-sensitive and verification-sensitive
   implementation concerns from `docs/targets.md` and `docs/testbench.md`
   where they materially affect DM2d structure, without turning DM2c
   into a full DM3 verification-plan step? When `docs/testbench.md`
   names internal signals to observe, does the plan list the signals
   AND the modules that expose them (without prescribing the
   framework mechanism -- naming `SignalTrace` / `test-only port` /
   etc. is a NIT, not a BLOCKER)?
9. Does the plan stay within DM2d scope? Reject tasks that pre-empt
   DM3 (directed verification suites, coverage targets,
   scoreboards, randomized stimulus).
10. Does the plan describe WHAT will be built and IN WHAT ORDER,
   without prescribing the algorithm inside each module's
   `evaluate()`? Tasks may name the function and the module file;
   they MUST NOT include shift-and-mask recipes, intermediate
   variable names, packing-format choices, or loop-vs-vectorized
   decisions. Acceptable: `Implement evaluate() in
   src/model/avg_stage.rs (consumes pixel_in_a + pixel_in_b,
   produces averaged_pixel).` Unacceptable: `Compute per-channel
   average: (a.r + b.r) >> 1, similarly for g, b, a / Pack the
   result into a u32...`. Flag the latter as BLOCKER and quote the
   offending lines.
11. Are open decisions (data types, buffer depths, fanouts not
    pinned by analysis) surfaced as explicit `DECIDE:` (or `OPEN:`
    for DM3-bound items) tasks with the format
    `- [ ] DECIDE: <question> -- options: <A | B>; default: <pick>;
    rationale: <one line>`? Decisions buried as parenthetical
    asides inside other tasks (e.g. `(u32 or PixelRGBA?)`) are
    BLOCKER -- name the offending lines.

## Output

Write `docs/critiques/DM2c-critique.md`. Free-form markdown body;
only line-prefix tokens (`BLOCKER:`, `UNRESOLVED:`, `RESOLVED:`)
are inspected by the gate.
