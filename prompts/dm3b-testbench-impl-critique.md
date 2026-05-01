# DM3b - Testbench Implementation (critique session)

You are reviewing the DM3b testbench scaffolding. Treat it as work
produced by a third party even if you produced it yourself earlier
in this conversation -- the independent-review property depends on
you bracketing any prior reasoning rather than leaning on it. The
testbench is the substrate DM3c will fill with edge / stress /
random tests; gaps here force DM3c to either work around them or
kick the work back. Do not modify the testbench; evaluate it and
write the critique file.

## Inputs

- `docs/plan/test-plan.md` -- the contract DM3b was implementing;
  specifically its `## Testbench` and `## Smoke` sections.
- `tests/` source tree (or the test module the work session used).
- `src/` -- the model under test, for confirming Monitors observe
  the right ports.
- Reference material on demand:
  - `lib:docs/modeling-guide/04-testing-models.md` for canonical
    UVM-lite structure and `SimEnvBuilder` patterns.
  - The example directories cited by `docs/plan/test-plan.md`.
  - `fw:api/toc.md`, then only the specific `fw:api/pages/...` files
    needed to confirm public API usage. Use `fw:src/prelude.rs` only as
    a secondary source if you need an exact source-level signature.

## Evaluation

Prefix gate-blocking issues with `BLOCKER:` (DM3c cannot proceed
until fixed). Prefix informational notes with `UNRESOLVED:`. The
orchestrator fails the DM3b gate on `BLOCKER:` lines only.

1. **Component coverage**. Is every component named in
   `docs/plan/test-plan.md`'s `## Testbench` section implemented?
   Quote the plan row and the implementing source location for
   each.
2. **UVM-lite topology**. Sequencer -> Driver -> DUT -> Monitor ->
   Scoreboard intact? Do any components reach into internal model
   state they should observe via Monitors?
3. **`SimEnvBuilder` wiring**. Is there a helper function (named
   per the plan) that returns a fully assembled `SimEnv`? Does it
   connect every external port?
4. **Scoreboard quality**. Are Scoreboard checks meaningful
   (value, ordering, invariants stated in the plan), not trivial
   (non-crash, "did anything come out")?
5. **Smoke test**. Is the basic data-flow smoke test from the
   plan implemented and passing? Does it exercise the wiring
   end-to-end (stimulus -> DUT -> monitor -> scoreboard) rather
   than testing components in isolation?
6. **Build state**. Does `cargo build` succeed? Does
   `cargo test` succeed for the smoke test? (Confirm via the
   `run_cargo` tool; don't infer.)
7. **Public API discipline**. Does the testbench stay within the
   public framework surface reachable from `fw:`? Reject reliance on
   internal helper modules or non-curated framework internals when a
   public API page does not justify it.
8. **Payload / port fidelity**. Do Drivers and Monitors use payload
   types and port names that match `src/`, `docs/spec.md`, and
   `docs/analysis/data-movement.md`? Flag mismatches explicitly.
9. **Scope discipline**. Does DM3b stay out of DM3c territory?
   Reject edge / stress / random tests authored at this step --
   only the basic data-flow smoke is allowed.
10. **Plan fidelity**. If the implementation deviated from
   `docs/plan/test-plan.md` (renamed components, added ones not
   in the plan, skipped ones), flag every deviation. The plan
   is the contract; deviations belong as a `BLOCKER:` so DM3a
   can be revisited.

## Output

Write `docs/critiques/DM3b-critique.md`. Free-form markdown body;
only line-prefix tokens (`BLOCKER:`, `UNRESOLVED:`, `RESOLVED:`)
are inspected by the gate.
