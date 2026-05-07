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

- `docs/impl-plan/plan-management.md` -- plan-file conventions
  (including the 10-task-per-milestone cap).
- `docs/test-plan/` -- the plan under review (a directory):
  - `test-plan.md` -- index. Testbench architecture +
    traceability table + TOCs of milestone files.
  - `tb-milestone-NN-<name>.md` -- DM3b's per-milestone task
    lists (testbench impl).
  - `test-milestone-NN-<name>.md` -- DM3c's per-milestone task
    lists (test execution).
  - `coverage.md` -- coverage strategy.
- `docs/spec.md`
- `docs/targets.md`
- `docs/testbench.md` -- DM1's verification strategy + named
  baseline.
- `docs/analysis/decomposition.md`
- `docs/analysis/pipeline-mapping.md`
- `docs/analysis/data-movement.md`
- `src/` -- the model under test.

## Evaluation

Prefix gate-blocking issues with `BLOCKER:` (DM3b cannot proceed
until fixed). Prefix informational notes with `UNRESOLVED:`. The
orchestrator fails the DM3a gate on `BLOCKER:` lines only.

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

1. **Directory layout**. Does the plan exist as a directory at
   `docs/test-plan/` with `test-plan.md` (index), `coverage.md`,
   at least one `tb-milestone-NN-<name>.md`, and at least one
   `test-milestone-NN-<name>.md`? Missing categories of file are
   BLOCKER.
2. **Milestone numbering**. Within each prefix
   (`tb-milestone-` and `test-milestone-`), are numbers
   contiguous starting from `01`? Gaps or duplicates are BLOCKER.
3. **Milestone size cap (10-task rule)**. Does every
   `tb-milestone-NN-*.md` and `test-milestone-NN-*.md` file have
   no more than 10 `- [ ]` rows? A milestone exceeding the cap
   forces the agent to skim or chain reviews; flag any
   over-cap milestone as `BLOCKER:` and require splitting along
   a clear axis (e.g. component-class for testbench, test-axis
   for execution).
4. **Testbench design (in `test-plan.md`)**. Does the index
   describe at least one Sequencer, Driver, Monitor, and
   Scoreboard? Are payload types and target ports named
   explicitly? Is the `SimEnvBuilder` wiring described
   concretely enough that DM3b can implement it without making
   architectural decisions?
5. **UVM-lite topology**. Does the testbench architecture honor
   Sequencer -> Driver -> DUT -> Monitor -> Scoreboard? Does
   `test-plan.md` cite the named baseline from
   `docs/testbench.md`'s `## Implementation Baseline` section
   AND chapters of `lib:docs/modeling-guide/` (especially
   `04-testing-models.md`)? Reject hand-rolled testbench
   architectures that bypass UVM-lite without justification.
6. **Testbench-impl milestones (`tb-milestone-NN-*.md`)**. Do
   the milestones cover the components named in the index
   (Sequencers, Drivers, Monitors, Scoreboards, the
   `SimEnvBuilder` helper, plus the basic data-flow smoke test)?
   Each row format:

   ```markdown
   - [ ] `<file_or_helper_path>::<symbol>` -- <purpose>;
     mirrors: `lib:examples/...`; traces to: <ref>
   ```

   Reject rows that don't name a concrete artifact path or
   don't trace to the test plan's testbench section.
7. **Test-execution milestones
   (`test-milestone-NN-*.md`)**. Do the milestones cover all four
   required categories (smoke, edge, stress, random) PLUS a
   coverage milestone? Each category maps to one or more
   consecutive milestones. The mandatory mapping AND the
   numbering order:

   - `01` -> Smoke -- at least one `test-milestone-01-smoke*.md`
   - `02` -> Edge -- at least one `test-milestone-02-edge*.md`
   - `03` -> Stress -- at least one `test-milestone-03-stress*.md`
   - `04` -> Random -- at least one `test-milestone-04-random*.md`
   - `05` -> Coverage -- `test-milestone-05-coverage.md`

   The numbering IS the execution order: smoke first, then edge,
   then stress, then random, then coverage. A milestone file that
   uses the wrong number for its category (e.g.
   `test-milestone-02-stress.md` or
   `test-milestone-04-edge-extra.md`) is `BLOCKER:` -- it would
   make DM3c walk categories in the wrong order. Quote the
   offending filename(s) and require renaming.

   A category file that's genuinely N/A for this design (e.g.
   smoke for purely combinational designs with no flow-control
   surface) MUST still exist with an explicit `RESOLVED:` line
   inside it explaining what the category would have covered.
   Silent omission of any of the five categories is `BLOCKER:`.

   Each test row format:

   ```markdown
   - [ ] <test_name> -- <purpose>; pass criteria: <criteria>;
     traces to: <ref>
   ```

   Reject vague pass criteria ("reasonable", "fast", "looks
   correct").

   **Category-mixing rule**: a milestone file MUST cover exactly
   one category. A single file holding both smoke and edge rows
   (or any other mixture) is `BLOCKER:` because the
   per-milestone critique pattern depends on each milestone
   slicing one category cleanly.

   **Split-file naming rule**: when a category exceeds the
   10-task cap, the splits use a letter suffix on NN and an axis
   tag on the name (`test-milestone-02a-edge-arithmetic.md`,
   `test-milestone-02b-edge-flow-control.md`). Letters
   contiguous from `a`. A category split into multiple files
   without the letter suffix scheme is `BLOCKER:`; the splits
   must walk lexicographically before the next category starts.
8. **Smoke coverage (smoke milestone)**. Are the required smoke
   tests present (elaboration, basic data flow, plus
   backpressure / idle if the design has flow-control)? Pure
   combinational designs may RESOLVE the missing categories.
9. **Edge coverage (edge milestone(s))**. Is there at least one
   edge test per non-trivial operation in `decomposition.md`?
   Are obvious boundaries (zero / max / min / saturation, empty /
   full buffers, single-element transit, reset mid-traffic)
   covered?
10. **Stress coverage (stress milestone(s))**. Does it exercise
    every target in `docs/targets.md`? Are run lengths concrete
    (1000+ cycles, named iteration counts) rather than vague
    ("for a while")?
11. **Random coverage (random milestone(s))**. Does each random
    test pin a seed in its name (`<test>_seed_<N>`) for
    reproducibility? Is there at least one random test per
    Sequencer plus a seed-sweep "soak" entry?
12. **Coverage strategy (`coverage.md`)**. Did the work session
    use the copy-then-fill template (`coverage.md.tmpl`)? The
    file MUST contain ALL FIVE required headings -- `## Tool`,
    `## Threshold`, `## Exclusions`, `## Run Command`,
    `## Report Output` (or `## Report Path`) -- and each must
    have concrete content, not placeholder prose. A `coverage.md`
    that's structurally missing any of these sections is
    `BLOCKER:` -- the template's structure is the contract DM3c
    reads. Specific checks within each section:
    - `## Tool`: names `cargo-tarpaulin`.
    - `## Threshold`: declares a numeric percentage (default
      90%); a non-default value needs an in-prose rationale.
    - `## Exclusions`: lists each excluded file with a
      one-sentence prose reason. An empty section, or a section
      that just gives a list with no rationales, is `BLOCKER:`.
    - `## Run Command`: contains a `cargo tarpaulin ...`
      command inside a CLOSED triple-backtick code fence. An
      unclosed fence is `BLOCKER:` (it breaks the markdown
      parse and signals a truncated response).
    - `## Report Output` (or `## Report Path`): names a
      specific FILE (e.g. `target/coverage/lcov.info`), not
      just a directory.

    Reject "we'll figure out coverage later".
13. **Coverage milestone (`test-milestone-NN-coverage.md` or
    similar)**. Does the final test-execution milestone walk
    `coverage.md`'s run command, record the measured percentage
    in `test-plan.md`'s `## Coverage` section, and address any
    uncovered lines? This milestone is what closes DM3c.
14. **Traceability (in `test-plan.md`)**. Does every requirement
    in `docs/spec.md` map to at least one task row in some
    milestone (quoting the requirement and naming
    `<milestone-file>::<task>`)? Does every target in
    `docs/targets.md` map to at least one row in a stress
    milestone? Does every operation in `decomposition.md` map to
    at least one row across smoke / edge milestones? Reject vague
    mappings ("covered by overall flow"); each link must name a
    specific task in a specific milestone file.
15. **Template hygiene**. Do any files still contain placeholder
    template text or empty sections that hide missing
    information rather than stating something concrete or
    explicitly saying "not applicable"?
16. **Scope**. Does the plan stay out of test-code territory?
    Each file should describe WHAT and HOW MUCH, not HOW each
    test is implemented. Reject embedded code snippets,
    `#[test]` annotations, or implementation pseudocode.

## Output

Write `docs/critiques/DM3a-critique.md`. Free-form markdown body;
only line-prefix tokens (`BLOCKER:`, `UNRESOLVED:`, `RESOLVED:`)
are inspected by the gate.
