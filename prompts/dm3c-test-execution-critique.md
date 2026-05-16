# DM3c - Test Execution and Coverage (critique session)

You are reviewing the DM3c test-execution and coverage results.
{{ third_party_reviewer_note }} Do not modify the test
artifacts; evaluate them and write the critique file.

This critique runs more than once:

- after each `test-milestone-NN-*.md` is checked off, to validate
  the just-landed slice before the next milestone starts
- once after the FINAL test-milestone (typically the coverage
  milestone), as the lighter end-to-end regression / coverage
  review

Determine which milestone was just completed by walking
`docs/test-plan/test-milestone-NN-*.md` files in numeric order
and finding the highest-numbered one whose rows are all resolved
(`- [x]` or `- [-]` with a defer-reason). Review that milestone
in detail, and also sanity-check that the new tests didn't
regress earlier milestones.

## Inputs

- `docs/impl-plan/plan-management.md` -- plan-file conventions
  including the 10-task cap.
- `docs/test-plan/test-plan.md` -- index. Verify the
  `## Coverage` section names a measured percentage + report
  path once DM3c reaches the coverage milestone.
- `docs/test-plan/test-milestone-NN-<name>.md` -- per-milestone
  task lists. Verify every `- [ ]` in the just-completed
  milestone is now `- [x]` or `- [-]` with a `- defer reason:`
  sub-bullet.
- `docs/test-plan/coverage.md` -- coverage strategy (threshold,
  exclusions, run command, report path). Verify any new
  exclusions DM3c added are justified.
- `docs/testbench.md` -- testbench architecture; useful when
  judging whether a test exercises the behaviors DM1 said the
  testbench must verify.
- `tests/` source tree.
- Coverage report (path recorded in
  `docs/test-plan/test-plan.md`'s `## Coverage` section once the
  coverage milestone has run).

## Evaluation

{{ critique_kinds }}

1. **Milestone completeness**. Identify the just-completed
   milestone. Is every row in that file `- [x]` or `- [-]` with
   a specific `- defer reason:` sub-bullet? Reject
   silently-skipped rows. On the FINAL critique, all
   test-milestone files must be fully resolved.

2. **Task fidelity**. For each `- [x]` row in the just-completed
   milestone, does a matching `#[test]`-annotated function exist
   in `tests/` with the name the task said it would? Quote the
   row and the implementing source location. Reject rows
   ticked off without the test landing.

3. **Test pass state**. Does `cargo test` succeed end-to-end at
   the milestone-complete checkpoint? If any failure is
   documented as known, is the rationale concrete (a specific
   design limitation tracked elsewhere) rather than vague
   ("flaky")?

4. **Random reproducibility** (random milestones only). Does
   every random test pin a specific seed in its name? A failure
   of `foo_seed_42` should be re-runnable as `cargo test
   foo_seed_42` and reproduce deterministically.

5. **Coverage threshold** (coverage milestone only). Is
   `cargo-llvm-cov` line coverage at or above `coverage.md`'s
   declared threshold (90% line coverage on `src/model/` is the
   default)? Is the measured percentage written into
   `test-plan.md`'s `## Coverage` section?

6. **Coverage exclusions** (coverage milestone only). Are any
   new exclusions DM3c added to `coverage.md` justified -- each
   names a specific file / module and a concrete reason (dead
   code, platform-gated, generated)? Reject vague exclusions
   ("unimportant", "we'll get to it"); they must either be
   tested or have a real reason.

7. **Bug-fix discipline**. When a design bug in `src/` was
   discovered during testing, was the fix re-verified by
   re-running the failing test? Reject "test was wrong"
   rationales that turn out to mask real bugs.

8. **Scaffolding integrity**. Did DM3c add tests using DM3b's
   testbench helpers, or did it modify or grow the scaffolding?
   Scaffolding changes here are a `BLOCKER:` -- the testbench
   architecture is owned by DM3b's gate.

9. **Deferred-row discipline**. Is the just-completed milestone
   one where every planned row was deferred? That is a
   `BLOCKER:` -- a fully deferred milestone means the flow has
   no meaningful execution signal for that class of behavior.
   Mostly-deferred (>25%) milestones are also a `BLOCKER:`
   even when individual defer reasons look fine in isolation.

10. **Milestone composability**. If this is a milestone
    checkpoint critique rather than the final DM3c review, is
    the just-completed milestone solid enough that the next
    test-milestone can safely build on it? If this is the final
    review (typically after the coverage milestone), do the
    milestone-local test additions compose into a clean
    end-to-end suite + coverage result without regression?

11. **Coding Requirements (per the work prompt)**. Inspect every
    Rust source file landed or modified in this milestone:
    - **Idiomatic Rust**: manual loops where iterators fit,
      `unwrap()` in non-test code paths, nested `if let` where
      `match` would read better, `Box<dyn _>` where a concrete
      type fits -> `BLOCKER:` with the file/line.
    - **Magic numbers / strings**: any inlined port name,
      payload value, threshold, seed (other than the
      milestone-pinned seed in random tests), or invariant
      constant -> `BLOCKER:`.
    - **Emojis**: any non-ASCII decorative glyph in code,
      comments, doc strings, error messages, or string literals
      -> `BLOCKER:`.
    - **File size cap**: line-count every Rust file authored
      or modified this milestone. Any file at or above 400
      lines -> `BLOCKER:` with the count and a suggested split
      (typically: extract a helper into its own file, or split
      a multi-test file into the per-test-per-file layout).

12. **File Layout (per the work prompt)**. Verify the per-
    category subdirectory split:
    - Smoke tests at `tests/smoke/<test_name>.rs`.
    - Edge tests at `tests/edge/<test_name>.rs`.
    - Stress tests at `tests/stress/<test_name>.rs`.
    - Random tests at `tests/random/<test_name>.rs` with the
      seed pinned in the filename (`<test>_seed_<N>.rs`).
    - DM3b's testbench scaffolding stays under
      `tests/testbench/`; DM3c does not modify it.
    - Multiple `#[test]` functions packed into one file ->
      `BLOCKER:`. One file per test is the contract.
    - The basic-data-flow smoke test from DM3b
      (`tests/smoke/basic_data_flow.rs`) survives intact
      (`UNRESOLVED:` if DM3c modified it; `BLOCKER:` if it was
      removed or replaced).

## Output

{{ output_intro }}

Write the critique as JSON to
`docs/critiques/DM3c-critique.json`. The orchestrator renders a
human-readable `docs/critiques/DM3c-critique.md` from that JSON
automatically; do NOT write the markdown yourself.

{{ critique_json_schema }}