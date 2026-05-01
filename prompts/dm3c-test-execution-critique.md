# DM3c - Test Execution and Coverage (critique session)

You are reviewing the DM3c test-execution and coverage results.
Treat them as work produced by a third party even if you produced
them yourself earlier in this conversation -- the independent-review
property depends on you bracketing any prior reasoning rather than
leaning on it. Do not modify the test artifacts; evaluate them and
write the critique file.

This critique runs more than once:

- after each category-complete checkpoint (`Smoke`, `Edge`,
  `Stress`, `Random`) before the next category begins
- once after the final category and coverage pass, as the final
  DM3c integration/regression review

Determine which category was just completed from the plan state,
review that category in detail, and also sanity-check that the new
tests did not regress earlier categories. On the final review,
evaluate the full plan, full suite, and coverage outcome.

## Inputs

- `docs/plan/test-plan.md` -- the plan; check that every `- [ ]`
  is now `- [x]` or `- [-]` with a `defer reason:`, and that the
  `## Coverage` section names a measured percentage + report path.
- `tests/` source tree.
- Coverage report (path recorded in `docs/plan/test-plan.md`'s
  `## Coverage` section).

## Evaluation

Prefix gate-blocking issues with `BLOCKER:` (DM4 cannot proceed
until fixed). Prefix informational notes with `UNRESOLVED:`. The
orchestrator fails the DM3c gate on `BLOCKER:` lines only.

1. **Plan completion**. Is every row in
   `docs/plan/test-plan.md` either `- [x]` or `- [-]` with a
   specific `defer reason:`? Reject silently-skipped rows.
2. **Category coverage**. Are all four categories (smoke, edge,
   stress, random) represented in the implemented suite, with at
   least the test counts the plan declared? The four categories
   exercise different failure modes; missing categories means
   missing coverage.
3. **Test pass state**. Does `cargo test` succeed end-to-end? If
   any failure is documented as known, is the rationale concrete
   (a specific design limitation tracked elsewhere) rather than
   vague ("flaky")?
4. **Random reproducibility**. Does every random test pin a
   specific seed in its name? A failure of `foo_seed_42` should
   be re-runnable as `cargo test foo_seed_42` and reproduce
   deterministically.
5. **Coverage threshold**. Is `cargo-tarpaulin` line coverage at
   or above the plan's declared threshold (default 90% on
   `src/`)? Is the measured percentage written into the plan's
   `## Coverage` section?
6. **Coverage exclusions**. Are uncovered lines justified --
   each exclusion names a specific file / module and a concrete
   reason (dead code, platform-gated, generated)? Reject vague
   exclusions ("unimportant", "we'll get to it"); they must
   either be tested or have a real reason.
7. **Bug-fix discipline**. When a design bug in `src/` was
   discovered during testing, was the fix re-verified by re-
   running the failing test? Reject "test was wrong" rationales
   that turn out to mask real bugs.
8. **Scaffolding integrity**. Did DM3c add tests using DM3b's
   testbench helpers, or did it modify or grow the scaffolding?
   Scaffolding changes here are a `BLOCKER:` -- the testbench
   architecture is owned by DM3b's gate.
9. **Deferred-row discipline**. Is there any category where every
   planned row was deferred? That is a `BLOCKER:` -- a fully
   deferred category means the flow has no meaningful execution
   signal for that class of behavior.
10. **Checkpoint discipline**. If this is a category checkpoint
    rather than the final DM3c review, is the just-completed
    category solid enough that the next category can build on it?
    If this is the final review, do the category-local test
    additions compose into a clean end-to-end suite and coverage
    result without regression?

## Output

Write `docs/critiques/DM3c-critique.md`. Free-form markdown body;
only line-prefix tokens (`BLOCKER:`, `UNRESOLVED:`, `RESOLVED:`)
are inspected by the gate.
