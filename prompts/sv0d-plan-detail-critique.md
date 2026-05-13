# SV0d Critique - SV Conversion Plan Detail (critique session)

You are critiquing one milestone stub that SV0d's work session
just detailed. The orchestrator has scoped you to a single
milestone file under `generated/plan/`.

## Inputs

- The CURRENT milestone file (orchestrator inlines its TOC).
- `generated/plan.md` -- the index, for cross-referencing scope /
  trace.
- `src/model/<file>.rs` -- Foundation module the stub maps to
  (rtl-milestone).
- `tests/testbench/<file>.rs` -- Rust testbench role (uvm-milestone).
- `docs/test-plan/test-plan.md` -- referenced test rows.

## Evaluation criteria

Emit findings as `RESOLVED:`, `UNRESOLVED:`, or `BLOCKER:`.

1. **Placeholder removed**: the file no longer contains
   `<!-- detail-pending`. Missing = `BLOCKER:`.
2. **`## Tasks` is non-empty**: at least one `- [ ]` row.
   Empty = `BLOCKER:`.
3. **Task count <= 10**: hard cap enforced by
   `plan-management.md`. Exceeded = `BLOCKER:`.
4. **Concrete artifact names**: each task names a specific `.sv`
   path under `generated/rtl/` or `generated/test/`. A row that
   says "implement the driver" without a filename = `BLOCKER:`.
5. **Pass criteria**: each task has a measurable pass criterion
   (e.g. "RTL compiles with `verilator --lint-only`",
   "scoreboard predicts expected output for spec worked example").
   Vague "works correctly" = `UNRESOLVED:`.
6. **Trace links**: each task has a `derives from:` sub-bullet
   pointing at a Foundation module / Rust testbench symbol / test
   row. Missing = `UNRESOLVED:`.
7. **Mandatory categorical content**:
   - `rtl-milestone-01-payloads.md`: lists every packed struct /
     typedef. Missing common payload (e.g. the model's main
     `Pixel`-like struct) = `BLOCKER:`.
   - Final rtl-milestone: includes `generated/rtl/top.sv`. Missing
     = `BLOCKER:`.
   - `uvm-milestone-01-types-and-interfaces.md`: includes
     `uvm_types_pkg.sv` + virtual interfaces. Missing = `BLOCKER:`.
   - Final uvm-milestone: includes `tb_top.sv` + per-test
     `uvm_test_*.sv` files. Missing = `BLOCKER:`.
8. **Scope agreement**: tasks stay within the stub's Scope /
   Components-Files list. A task naming a file outside this
   milestone's contract = `BLOCKER:` (cross-milestone leak).
9. **Auto-decisions**: non-trivial choices (merging tasks, deferring
   a stretch goal, splitting a struct into two payload files) are
   recorded in `## Auto-decisions`. Missing rationale on a
   non-obvious decision = `UNRESOLVED:`.

## Output

Write `docs/critiques/SV0d-critique.json` per the standard critique
JSON schema. The orchestrator renders the `.md` sibling.

Stop after writing the JSON. Do not `/exit`.
