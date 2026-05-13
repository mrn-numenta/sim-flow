# SV0 Critique - SystemVerilog Conversion Plan (critique session)

You are critiquing step SV0's plan + stubs. The plan must give SV1
(RTL emission) and SV2 (UVM emission) a complete, traceable
contract before either step starts emitting code.

## Inputs

The orchestrator has inlined the latest critique file (if this is
a retry) plus the TOC of plan artifacts. Read on demand:

- `generated/plan.md` -- the index.
- `generated/plan/rtl-milestone-NN-*.md` -- each RTL stub.
- `generated/plan/uvm-milestone-NN-*.md` -- each UVM stub.
- `src/model/` -- to verify the module inventory is complete.
- `docs/test-plan/test-plan.md` -- to verify every planned test
  row is traced.
- `docs/analysis/decomposition.md`,
  `docs/analysis/pipeline-mapping.md`,
  `docs/analysis/data-movement.md` -- to verify pattern
  classifications are defensible.

## Evaluation criteria

Emit a finding line per criterion. Lines are `RESOLVED:`,
`UNRESOLVED:`, or `BLOCKER:`. RESOLVED is the default for things
that look correct; UNRESOLVED is for editorial / non-blocking
gaps; BLOCKER is for structural defects SV1 / SV2 cannot work
around.

1. **Module coverage**: every Foundation module under
   `src/model/` appears in plan.md's module table AND in some
   rtl-milestone stub. Missing module = `BLOCKER:`.
2. **Pattern classification**: each module is classified into a
   hardware pattern from the design-pattern guide. A bare
   "transform stage" label without justification when the
   Foundation module has feedback / kill / sideband semantics
   that the data-movement doc flags is `UNRESOLVED:`. Mis-classified
   (e.g. labeling a FIFO as a simple pipeline) is `BLOCKER:`.
3. **RTL file mapping**: each rtl-milestone names the concrete
   `.sv` file paths it will produce under `generated/rtl/`. A
   stub that says "writes the RTL" without naming files is a
   `BLOCKER:`.
4. **`rtl-milestone-01-payloads.md`** exists and produces
   `generated/rtl/payloads.sv` (shared packed structs). Missing =
   `BLOCKER:`.
5. **Final RTL milestone produces `top.sv`**. Missing = `BLOCKER:`.
6. **UVM coverage**: every testbench role from the Rust side
   (Sequencer, Driver, Monitor, Scoreboard, SimEnvBuilder helper)
   has a corresponding UVM milestone or is explicitly justified
   in plan.md's auto-decisions (e.g. "no scoreboard -- Rust uses
   direct assertions"). A silently-dropped role is `BLOCKER:`.
7. **Test-plan traceability**: every row in
   `docs/test-plan/test-plan.md`'s milestone tables traces to a
   UVM milestone in plan.md's traceability table. Untraced rows
   are `BLOCKER:`.
8. **Stub placeholder**: every milestone stub still carries the
   `<!-- detail-pending -->` marker (SV0d's gate keys on its
   absence). A stub missing the marker = `BLOCKER:` (SV0 should
   not pre-empt SV0d).
9. **Stub-content scope**: stubs describe WHAT (files, role,
   trace), not HOW (no SV code, no concrete struct field lists --
   that's SV0d's job). A stub leaking detail-step content is
   `UNRESOLVED:`.
10. **Naming convention**: milestone files use
    `rtl-milestone-NN-<slug>.md` / `uvm-milestone-NN-<slug>.md`
    with two-digit zero-padded NN. Mismatch = `BLOCKER:` (the
    orchestrator's walk-order depends on lexicographic NN
    sorting).
11. **Auto-decisions**: any non-obvious classification or
    grouping choice is recorded in plan.md's auto-decisions.
    Missing rationale on a non-trivial choice is `UNRESOLVED:`.

## Output

Write `docs/critiques/SV0-critique.json` per the standard critique
JSON schema (`step`, `summary`, `findings[]`, `notes`). The
orchestrator renders the `.md` sibling automatically.

Stop after writing the JSON. Do not `/exit`.
