# Generate SystemVerilog (work session)

You are emitting synthesizable SystemVerilog RTL plus a full
SystemVerilog UVM testbench from the cycle-accurate sim-foundation
model in `src/`. The hardware must behave identically to the
Foundation model under the same stimulus, with **one register
stage at every Module input** -- that's the framework's invariant
and the SV must honor it.

This is a one-off generation triggered from the dashboard, not a
flow step. There is no critique session and no gate -- the user
inspects the output by running it through their own simulator.

If the dashboard's "Run and debug generated SystemVerilog after
emission" setting is on, a `## Simulate and iterate` section is
appended below; in that mode you also run the emitted code through
the configured simulator and fix failures until it passes. When the
appendix is absent, stop after emission -- don't invoke any
simulator on your own.

## Inputs

- `src/model/` -- the Foundation model. Modules implement
  `Module + HasLogic + HasInstances`. Read every module to map
  ports, payload types, and the `evaluate()` body to SV.
- `src/sim.rs` (if present) -- top-level wiring, `ConnectivityPlan`.
- `tests/` -- the UVM-lite testbench (Sequencer, Driver, Monitor,
  Scoreboard, SimEnvBuilder helper). Mirror its components in
  full SystemVerilog UVM.
- `docs/spec.md` -- the source spec; for any detail not pinned in
  `src/`, fall back to spec language.
- `docs/analysis/decomposition.md`,
  `docs/analysis/pipeline-mapping.md`,
  `docs/analysis/data-movement.md` -- module names, pipeline
  shape, payload widths.
- `docs/plan/test-plan.md` -- pin-pointed test cases that should
  also run against the SV output (use them to seed the UVM tests).

## Output

All generated files live under `generated/`. The directory exists
to keep machine-emitted code separate from the hand-curated Rust
model under `src/` and `tests/`.

- `generated/rtl/payloads.sv` -- a single header file with
  `typedef struct packed { ... }` declarations matching every
  Rust payload `struct` in `src/model/`. RTL files include this.
- `generated/rtl/<module-name>.sv` -- one file per Foundation
  module. Module name uses snake_case; file basename matches
  module name.
- `generated/rtl/top.sv` -- top-level wiring matching the
  `ConnectivityPlan`. Instantiates every leaf module and wires
  ready/valid/payload busses.
- `generated/test/uvm_<name>_seq.sv` -- one per Sequencer,
  with a `uvm_sequence` plus a `uvm_sequencer #(item_t)`.
- `generated/test/uvm_<name>_drv.sv` -- one per Driver
  (`uvm_driver #(item_t)`).
- `generated/test/uvm_<name>_mon.sv` -- one per Monitor
  (`uvm_monitor` with `uvm_analysis_port`).
- `generated/test/uvm_<name>_sb.sv` -- one per Scoreboard
  (`uvm_scoreboard` with `uvm_analysis_imp`).
- `generated/test/uvm_env.sv` -- a `uvm_env` that instantiates
  every Sequencer / Driver / Monitor / Scoreboard above and
  hooks them via `analysis_imp_decl`.
- `generated/test/uvm_test_base.sv` -- a base `uvm_test`
  extending `uvm_env`; concrete tests extend this and override
  `run_phase`.
- `generated/test/uvm_test_<smoke|edge|stress|random>.sv` -- one
  test class per category from `docs/plan/test-plan.md`. Each
  test uses sequences whose stimulus mirrors the Rust test of
  the same name when one exists.
- `generated/test/tb_top.sv` -- top-level testbench module.
  Instantiates `top.sv`, ties the UVM env to the DUT virtual
  interfaces, and calls `run_test()` from `initial`.
- `generated/test/sim.f` -- a flat file list of every `.sv`
  under `generated/` that simulators can compile from. Order
  matters: `payloads.sv` first, then RTL modules, then `top.sv`,
  then UVM components, then `tb_top.sv`.
- `generated/test/Makefile` -- minimal Modelsim / VCS / Verilator
  recipe (use whichever the project's spec / targets prefer; if
  unspecified, default to Verilator since it's open-source). At
  minimum the Makefile should expose `make sim` to run all
  tests and `make sim TEST=<name>` to run a single one.

Use the artifact-write convention you've been instructed to use
this session (Write tool for native-tools mode, fenced
` ``` <path> ` blocks for JSONL hosts). Every output file must be
under `generated/`. Do **NOT** modify anything under `src/`,
`tests/`, `.sim-flow/`, `docs/`, or files in `generated/` that
already exist from a previous run -- treat each generation as
overwrite-only at the file level: if you need to update a file,
re-emit the whole thing.

## Mapping rules

### Foundation Module → SV module

Every `Module` becomes a SystemVerilog `module` named
identically (snake_case for SV). Inputs are registered (the
framework's flopped-input invariant); the body holds combinational
logic that mirrors `evaluate()`; outputs follow ready/valid
handshake.

Skeleton:

```systemverilog
`include "payloads.sv"

module my_module #(
  parameter int FOO = 8 // any compile-time constants
)(
  input  logic         clk,
  input  logic         rst_n,
  // per-input port: valid + payload + back-pressure ready
  input  logic         in_valid,
  output logic         in_ready,
  input  payload_t     in_data,
  // per-output port: same shape, opposite direction on ready
  output logic         out_valid,
  input  logic         out_ready,
  output payload_t     out_data
);
  // 1. Registered inputs (Foundation's flopped-input invariant).
  payload_t   in_data_q;
  logic       in_valid_q;
  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) begin
      in_valid_q <= 1'b0;
      in_data_q  <= '{default: '0};
    end else if (in_valid && in_ready) begin
      in_valid_q <= 1'b1;
      in_data_q  <= in_data;
    end else if (out_valid && out_ready) begin
      in_valid_q <= 1'b0;
    end
  end

  // 2. Combinational body -- mirror of `evaluate()` in Rust.
  always_comb begin
    out_data = '{default: '0};
    // ... derived from the Rust evaluate() body, expression by
    //     expression. Match operator semantics (saturating add,
    //     wrapping mul, etc.) explicitly with SV operators.
  end

  // 3. Ready / valid (one-deep skid).
  assign in_ready  = !in_valid_q || (out_valid && out_ready);
  assign out_valid = in_valid_q;
endmodule
```

If the Rust module has multiple inputs or outputs, add one
`<port>_valid / <port>_ready / <port>_data` triple per port. If a
port carries a non-payload control signal (`u32` clock-divider, a
`bool` enable), keep it as a typed input/output rather than
embedding it inside a payload struct.

### Foundation `settle() / update()` → registered state

The `Module` phase order is `evaluate -> settle -> update`. Any
state the model holds in `update()` becomes a register array in
the SV module:

```systemverilog
typedef struct packed {
    logic [31:0] count;
} my_state_t;

my_state_t state_q, state_d;

always_comb state_d = state_q; // default: hold

always_comb begin
  // ... when conditions match, override state_d
end

always_ff @(posedge clk or negedge rst_n) begin
  if (!rst_n)        state_q <= '{default: '0};
  else if (out_valid && out_ready) state_q <= state_d;
end
```

Use `<name>_q` for the registered value and `<name>_d` for the
combinational next-state.

### Payload structs

Every Rust payload `struct` becomes a SystemVerilog
`typedef struct packed`. Field order and bit widths must match
exactly so the testbench can reuse the same stimulus. Pack the
least-significant field at the bottom -- SystemVerilog `packed`
structs lay out fields high-to-low, so list the highest-significance
field first in the typedef.

```rust
// src/model/payload.rs
pub struct Beat {
    pub data: u32,
    pub last: bool,
}
```
becomes
```systemverilog
typedef struct packed {
    logic        last;   // [32]
    logic [31:0] data;   // [31:0]
} beat_t;
```

Type names use lowercase with `_t` suffix.

### Top-level wiring (ConnectivityPlan)

The Rust `ConnectivityPlan` becomes `generated/rtl/top.sv`. Every
edge in the plan is a SystemVerilog wire (or, for payloads, a
`payload_t` net). Match the Rust port names exactly so a netlist
diff stays readable.

```systemverilog
module top (
  input  logic     clk,
  input  logic     rst_n,
  input  logic     drv_in_valid,
  output logic     drv_in_ready,
  input  payload_t drv_in_data,
  output logic     mon_out_valid,
  input  logic     mon_out_ready,
  output payload_t mon_out_data
);
  // wires for stage1 -> stage2
  logic     s1_out_valid, s1_out_ready;
  payload_t s1_out_data;

  stage1 u_stage1 (
    .clk(clk), .rst_n(rst_n),
    .in_valid(drv_in_valid), .in_ready(drv_in_ready), .in_data(drv_in_data),
    .out_valid(s1_out_valid), .out_ready(s1_out_ready), .out_data(s1_out_data)
  );
  stage2 u_stage2 (
    .clk(clk), .rst_n(rst_n),
    .in_valid(s1_out_valid), .in_ready(s1_out_ready), .in_data(s1_out_data),
    .out_valid(mon_out_valid), .out_ready(mon_out_ready), .out_data(mon_out_data)
  );
endmodule
```

### UVM-lite → SystemVerilog UVM

| Foundation (Rust) | SystemVerilog UVM equivalent |
|-------------------|------------------------------|
| `Sequencer<Item>` | `uvm_sequence #(item_t)` + `uvm_sequencer #(item_t)` |
| `Driver`          | `uvm_driver #(item_t)` with `run_phase` and a virtual interface to the DUT |
| `Monitor`         | `uvm_monitor` exposing a `uvm_analysis_port #(item_t)` |
| `Scoreboard`      | `uvm_scoreboard` with `` `uvm_analysis_imp_decl(_<name>) `` and a check method per invariant |
| `SimEnvBuilder` helper | `uvm_env` that builds + connects all of the above |
| Per-test setup    | `uvm_test` extending the env; smoke / edge / stress / random get one class each, with a sequence library mirroring `docs/plan/test-plan.md` rows |

Stimulus must be reproducible: random tests pin a seed (use
`+UVM_SEED=<N>` in the Makefile) so failures are deterministic
just like the Rust random tests.

The DUT virtual interface (`top.sv`'s ports) is wired through a
`uvm_config_db` set by `tb_top.sv`. Drivers use `vif.cb_<port>`
clocking blocks to drive into the DUT on the clock edge --
the SV environment must respect the same flopped-input timing the
Rust tests assumed.

## Constraints

- **Synthesizable subset for RTL**. No `initial` blocks, no
  `#delays`, no `force/release`, no `$display`/`$finish` outside
  testbench code. RTL must pass Yosys / Verilator parse.
- **Test-only constructs in test/**. Initial blocks, `#delays`,
  `$display`, `$finish` are fine inside `generated/test/`.
- **No reaching into module internals**. Each `<module>.sv` is
  self-contained; instantiate by name only. Cross-module
  observation goes through Monitors, mirroring the UVM-lite
  invariant in Rust.
- **Bit-exact payload layouts**. SV `typedef struct packed` field
  order and widths must match the Rust struct layout used by the
  Rust tests' stimulus.
- **One file per module / class**. Don't lump multiple modules
  into one file; the file list (`sim.f`) reads cleaner with the
  one-to-one mapping.
- **Comment the source mapping**. At the top of each generated
  file, include a one-line comment of the form
  `// generated from src/model/<file>.rs::<name>` so a reader
  can trace back. For files derived from multiple sources, list
  all of them.
- **Don't touch anything outside `generated/`**. The Foundation
  model in `src/`, the UVM-lite tests in `tests/`, the analysis
  docs, the `.sim-flow/` state, and the plans are all read-only
  during this generation.
