# SV2 - UVM Testbench Emission (work session)

You are executing step SV2 (UVM Emission) of the SystemVerilog
Conversion Flow. Prerequisite: SV1 gate passed.

## Goal

Emit a full SystemVerilog UVM testbench under `generated/test/`
that mirrors the Rust-side UVM-lite testbench (`tests/testbench/`,
`tests/<category>/`) and exercises the RTL from `generated/rtl/`.
SV2 is milestone-driven: walk each `generated/plan/uvm-milestone-NN-*.md`
in order, emit the listed files, mark each task `- [x]`, stop for
the paired critique. Do NOT chain milestones.

## Inputs

- `generated/plan.md` -- conversion plan with UVM milestone table.
- `generated/plan/uvm-milestone-NN-<name>.md` -- current milestone.
- `generated/rtl/` -- DUT SV (produced by SV1).
- `tests/testbench/` -- the Rust UVM-lite scaffolding (Sequencer,
  Driver, Monitor, Scoreboard, SimEnvBuilder helper).
- `tests/<category>/` -- the directed / random / stress / smoke /
  edge tests DM3c wrote. Each maps to a UVM test.
- `docs/test-plan/test-plan.md` -- the verification contract.
- `lib:examples/02-multiple-ports/test/` -- baseline UVM
  structural reference.
- `fw:api/toc.md` then specific `fw:api/pages/...` for framework
  API parity.

## Procedure

1. **Pick the current milestone**: lexicographic walk of
   `uvm-milestone-NN-*.md` under `generated/plan/`. First file
   with an open `- [ ]` row.

2. **Walk tasks in order**. For each `- [ ]` row:
   - Read the referenced Rust testbench symbol (Sequencer
     impl, Driver run_phase analog, Monitor sample logic,
     Scoreboard predict logic) AND/OR the referenced test row
     from `docs/test-plan/test-plan.md`.
   - Emit the named `.sv` file under `generated/test/`.
   - Source-trace comment at the top:
     `// generated from tests/testbench/<file>.rs::<symbol>` or
     `// generated from docs/test-plan/<file>.md::<row>`.
   - Mark the row `- [x]`.

3. **UVM structural conventions**:
   - `uvm_types_pkg.sv`: sequence items, shared typedefs, the
     `uvm_pkg::*` import. Sequence items inherit
     `uvm_sequence_item`; use `uvm_object_utils_begin/end` macros.
   - Virtual interface per port group: `<name>_if.sv` defines
     `interface <name>_if` with clocking blocks the driver /
     monitor consume via `virtual <name>_if`.
   - Driver: `uvm_<name>_drv.sv` extends `uvm_driver #(<txn>)`
     with `run_phase` that calls
     `seq_item_port.get_next_item / item_done`. Mirror Rust
     Driver's per-cycle protocol (when to accept / when to back-
     pressure).
   - Monitor: `uvm_<name>_mon.sv` extends `uvm_monitor`, samples
     the virtual interface every cycle, fires
     `uvm_analysis_port::write(<txn>)`.
   - Scoreboard: `uvm_<name>_sb.sv` extends `uvm_scoreboard`,
     subscribes to monitor's analysis port, predicts expected
     vs observed. Use direct assertions when the Rust side does
     (don't force a scoreboard abstraction).
   - Env: `uvm_env.sv` extends `uvm_env`, builds + connects
     driver / monitor / scoreboard / sequencer.
   - Base test: `uvm_test_base.sv` extends `uvm_test`, builds the
     env, raises objections in `run_phase`.
   - Per-test: `uvm_test_<name>.sv` extends `uvm_test_base`, runs
     a specific sequence. One per planned test row or per
     coherent test family. Random tests pin a seed in the name
     AND set `+UVM_SEED=<N>` documentation in the test's
     `class new()` comment.
   - `tb_top.sv`: top module instantiates the DUT, the virtual
     interface(s), clock + reset, and calls
     `run_test("<default_test>")` in an `initial` block.

4. **Reproducibility**: random tests must be re-runnable via
   `make sim TEST=<name>` and the simulator's seed control
   (`+UVM_SEED=<N>` or per-simulator equivalent).

5. **Surface the milestone-complete notice**:

   > `<milestone-name> complete; ready for critique.`
   > `<one-line summary: N files added, M tasks resolved, K
   > deferred items, any ambiguity flagged>`

   Do NOT proceed to the next milestone.

## Output

{{ output_intro }}

Files this step adds under `generated/test/` (per milestone task
list).

## Constraints

- **No DUT modifications**. Do not touch anything under
  `generated/rtl/`, `src/`, `tests/`, or `docs/`.
- **No `sim.f` / `Makefile` yet** -- those belong to SV3.
- **Preserve verification intent**, not just component names. The
  UVM env must check what the Rust testbench checks (ordering,
  latency, backpressure, scoreboard intent or direct assertions).
- **Don't reduce to category smoke tests**: every planned test
  row needs a corresponding UVM test/sequence or an explicit
  parameterized grouping recorded in the milestone's
  `## Auto-decisions`.
- **Source-trace comment** at the top of every generated `.sv`.

When the current milestone is fully emitted, stop. Do not write
`docs/critiques/SV2-critique.md`. Do not `/exit`.
