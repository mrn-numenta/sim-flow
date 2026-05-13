# SV3 - Build and Validate (work session)

You are executing step SV3 (Build + Validate) of the SystemVerilog
Conversion Flow. Prerequisite: SV2 gate passed. This is the final
SV step: emit the build / run scaffolding, run Verilator, and
iterate on failures until the UVM tests pass under simulation.

## Goal

Make the generated SV simulate cleanly. The orchestrator's gate
runs your validation output (`generated/validation.md`) and checks
that `sim.f` + `Makefile` exist; the critique reviews the run
record.

## Inputs

- `generated/rtl/` -- RTL from SV1.
- `generated/test/` -- UVM testbench from SV2.
- `generated/plan.md` -- conversion plan + traceability.
- `~/Projects/Verilog/uvm-test.py` (when available) -- reference
  Verilator + UVM driver script you can mirror in the generated
  Makefile.
- `1800.2-2017-1.0/src/uvm_pkg.sv` (Accellera UVM library; the
  orchestrator may seed a local copy under
  `.sim-flow/uvm-1800.2-2017/` -- if not present, the prompt's
  procedure tells the agent how to bootstrap).

## Procedure

1. **Locate the UVM library**. Required path is whatever the
   project has -- check (in order):
   - `<project>/.sim-flow/uvm-1800.2-2017/src/` (orchestrator-
     seeded).
   - `<HOME>/Projects/Verilog/1800.2-2017-1.0/src/` (developer
     sibling).
   - Anywhere `+incdir+` already resolves `uvm_pkg.sv` on disk.

   If none is found, record in `generated/validation.md` that the
   UVM library is missing and the operator must seed it under
   `.sim-flow/uvm-1800.2-2017/`. Do NOT auto-download from inside
   this session; the orchestrator owns network IO.

2. **Emit `generated/test/sim.f`** -- a flat compile-order file
   list:

   ```
   +incdir+<uvm_src>
   <uvm_src>/uvm_pkg.sv
   ../rtl/payloads.sv
   ../rtl/<each-module>.sv
   ../rtl/top.sv
   uvm_types_pkg.sv
   <each-interface>.sv
   uvm_<name>_drv.sv
   uvm_<name>_mon.sv
   uvm_<name>_sb.sv
   uvm_env.sv
   uvm_test_base.sv
   <each-uvm_test_*.sv>
   tb_top.sv
   ```

3. **Emit `generated/test/Makefile`** with at minimum two
   targets:

   ```
   sim:
       verilator --binary -j 0 --timing -Wno-fatal -Wno-lint \
         -f sim.f --top-module tb_top \
         +define+UVM_NO_DPI
       ./obj_dir/Vtb_top +UVM_TESTNAME=$(TEST)

   clean:
       rm -rf obj_dir
   ```

   Default `TEST` to the smoke test. Document the
   `make sim TEST=<name>` invocation in `generated/manifest.md`.

4. **Run `make sim`**. Use `run_cargo({"command": "test"})` is NOT
   appropriate here -- this step shells out to verilator. Run via
   `run_cargo`-style tool if a verilator wrapper exists; otherwise
   surface the missing tooling in `generated/validation.md` and
   stop (the orchestrator's preflight should have installed
   verilator on startup).

5. **Iterate on failures**. Typical classes:
   - Verilator complains about syntax / unresolved symbols ->
     fix the offending RTL/UVM file. Surface which milestone
     owned it in `generated/manifest.md` under "Fix log".
   - UVM run reports `UVM_ERROR` / `UVM_FATAL` -> fix the
     scoreboard / sequence / driver until it passes.
   - Don't blindly silence warnings; if a `-Wno-...` flag is
     added, justify it in `generated/manifest.md`.

6. **Write `generated/manifest.md`** (the generation summary):
   - Source files consulted.
   - Module-to-RTL mapping (lifted from `generated/plan.md`).
   - Chosen design pattern per module (lifted from plan).
   - Key assumptions / unresolved ambiguities.
   - Fix log (which milestones were re-touched in SV3 and why).
   - Validation status (final).

7. **Write `generated/validation.md`** (the run record):
   - Tool used (`verilator --version` output).
   - Exact command invoked.
   - Which tests were run (smoke / edge / stress / random
     coverage).
   - Pass/fail summary per test.
   - Any unvalidated gaps (e.g. tests that didn't make it into
     the SV port; should be empty if SV2's plan-mapping was
     complete).

8. **Surface the step-complete notice**:

   > `SV3 build + validate complete; ready for critique.`
   > `<one-line summary: M tests passed, K iterations to converge,
   > any unvalidated rows flagged>`

## Output

{{ output_intro }}

- `generated/test/sim.f`
- `generated/test/Makefile`
- `generated/manifest.md`
- `generated/validation.md`

## Constraints

- **No DUT or testbench rewrites without milestone ownership**:
  every fix must be attributed to a specific milestone in the
  manifest's "Fix log". If a fix doesn't fit any milestone, record
  the gap in `generated/validation.md` -- it suggests SV0/SV0d's
  plan was incomplete.
- **No silent `-Wno-` flags** beyond `-Wno-fatal -Wno-lint` (which
  the reference example uses). Each additional disable is logged
  in `manifest.md`.
- **Do not touch anything outside `generated/`**.
- **Do not modify upstream Rust files**: `src/`, `tests/`, `docs/`
  are read-only at this step.

When the simulation runs clean, stop. Do not write
`docs/critiques/SV3-critique.md`. Do not `/exit`.
