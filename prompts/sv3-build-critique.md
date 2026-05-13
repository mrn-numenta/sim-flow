# SV3 Critique - Build and Validate (critique session)

You are critiquing the SV3 step's build + validation output. SV3
should have produced a runnable simulation and recorded its
results.

## Inputs

- `generated/test/sim.f` -- compile-order file list.
- `generated/test/Makefile` -- runnable simulation flow.
- `generated/manifest.md` -- generation summary + fix log.
- `generated/validation.md` -- run record.
- `generated/rtl/`, `generated/test/` -- final SV state.

## Evaluation criteria

Emit findings as `RESOLVED:`, `UNRESOLVED:`, or `BLOCKER:`.

1. **`sim.f` exists and references every emitted `.sv`**: every
   file under `generated/rtl/` and `generated/test/` (other than
   `sim.f` / `Makefile` themselves) appears in `sim.f` in
   compile order. Missing references = `BLOCKER:`.
2. **`Makefile` runs `verilator --binary`** with the
   `tb_top` top module and `+UVM_NO_DPI`. Bare `verilator
   -E` / lint-only would not actually simulate = `BLOCKER:`.
3. **UVM library include**: `sim.f` has a `+incdir+` pointing at
   the Accellera UVM `src/` AND lists `uvm_pkg.sv`. Missing =
   `BLOCKER:`.
4. **`validation.md` records an actual run**: tool, command, pass
   / fail summary per test. A `validation.md` that just says
   "would run if verilator installed" is `UNRESOLVED:` (operator
   needs to seed the tool) BUT becomes `BLOCKER:` if SV3's
   preflight should have installed verilator and didn't.
5. **All planned tests ran**: every UVM test file under
   `generated/test/uvm_test_*.sv` appears in `validation.md`'s
   per-test summary. Missing = `BLOCKER:`.
6. **Pass rate**: `validation.md`'s summary shows all tests
   passing. Failing tests = `BLOCKER:` (SV3's job is to iterate
   until clean).
7. **`manifest.md` complete**: lists source files consulted,
   module-to-RTL mapping, design patterns, assumptions, fix log,
   final validation status. Missing fix log when SV3 had to
   re-touch milestones = `UNRESOLVED:`.
8. **No silent `-Wno-` additions**: every disable flag beyond
   `-Wno-fatal -Wno-lint` (reference example baseline) is
   justified in `manifest.md`. Unjustified disables =
   `UNRESOLVED:`.
9. **Source-side untouched**: no files under `src/`, `tests/`,
   `docs/` modified by SV3. Touched = `BLOCKER:`.
10. **Plan parity**: every milestone listed in
    `generated/plan.md` had its files emitted and validated. A
    plan row with no corresponding artifact on disk =
    `BLOCKER:`. A plan row dropped silently with no
    `generated/validation.md` note = `UNRESOLVED:`.

## Output

Write `docs/critiques/SV3-critique.json` per the standard critique
JSON schema. The orchestrator renders the `.md` sibling.

Stop after writing the JSON. Do not `/exit`.
