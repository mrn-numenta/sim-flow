# Generate SystemVerilog (work session)

You are emitting synthesizable SystemVerilog RTL plus a full
SystemVerilog UVM testbench from the cycle-accurate sim-foundation
model in `src/`. The generated hardware and UVM testbench must
preserve the Foundation model's externally visible behavior under the
same stimulus. Treat the Rust model as the executable specification and
the project documents as the architectural contract.

Foundation's key hardware-facing invariant is:

- every consumed module input crosses at least one register stage before
  it influences downstream registered state or externally visible output

Honor that invariant in the SV. Do not over-simplify it into a single
hard-coded one-deep shell for every module; derive the actual control
and buffering behavior from the model.

This is a one-off generation triggered from the dashboard, not a flow
step. There is no critique session and no orchestrator gate. If the
dashboard's "Run and debug generated SystemVerilog after emission"
setting is on, a `## Simulate and iterate` section is appended below; in
that mode you also run the emitted code through the configured
simulator and fix failures until it passes. When the appendix is absent,
stop after emission plus any available static validation described below
-- do not invoke a simulator on your own unless the appendix tells you
to.

## Source of truth and required references

Use sources in this order of authority:

1. Project-local implementation and analysis artifacts
2. Project-local tests and test plan
3. The modeling guides and worked examples in `sim-models`
4. The curated Foundation public API docs under `fw:api/`
5. `fw:src/...` only as a secondary source for exact signatures or
   source-level clarification

Read all of the following before writing code:

- `docs/spec.md`
- `docs/targets.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/pipeline-mapping.md`
- `docs/analysis/data-movement.md`
- `docs/test-plan/test-plan.md`
- `src/model/`
- `src/sim.rs` if present
- `tests/`
- `lib:docs/modeling-guide/04-testing-models.md`
- `lib:docs/modeling-guide/06-design-patterns.md`
- `api_semantic_search(query)` then `api_hover(symbol)` for any
  framework symbol you need to look up. Do NOT `read_file` paths
  under `fw:api/pages/*.md`; the lance API index + LSP queries
  supersede the static page mirror.

Use `fw:src/...` only when even `api_hover` cannot show the
exact source detail you need (rare; almost always an internal
helper body that the LSP-backed tools intentionally hide).

## Generation goals

Produce SystemVerilog that is:

- synthesizable for the RTL subset
- faithful to the model's functional, ordering, latency, and
  backpressure behavior
- structured enough that a hardware engineer can trace RTL and UVM back
  to the Foundation model and test plan
- statically validated where possible, even when simulator iteration is
  disabled

## Inputs

- `src/model/` -- the Foundation model. Modules implement
  `Module + HasLogic + HasInstances`, `SimpleModule`, or library-module
  wrappers. Read every relevant module to map ports, payload types,
  state, `evaluate()`, `settle()`, and `update()` semantics to SV.
- `src/sim.rs` (if present) -- top-level wiring, `ConnectivityPlan`,
  runtime/testbench setup.
- `tests/` -- the UVM-lite testbench (Sequencer, Driver, Monitor,
  Scoreboard, SimEnvBuilder helper) and any direct assertions. Mirror
  the verification intent in full SystemVerilog UVM; do not reduce the
  testbench to category-level smoke only.
- `docs/spec.md` -- architectural intent and any behavior not fully
  obvious from code.
- `docs/analysis/decomposition.md`,
  `docs/analysis/pipeline-mapping.md`,
  `docs/analysis/data-movement.md` -- module names, pipeline structure,
  payload widths, module/stage boundaries, data edges, and intended
  staging.
- `docs/test-plan/test-plan.md` -- the concrete verification contract. Treat
  each planned test row as something that should have an equivalent SV
  sequence/test or a clearly documented grouping into a coherent test
  family.

## Output

All generated files live under `generated/`. The directory exists to
keep machine-emitted code separate from the hand-curated Rust model
under `src/` and `tests/`.

- `generated/manifest.md` -- generation summary:
  - source files consulted
  - module-to-RTL mapping
  - chosen design pattern per module
  - key assumptions / unresolved ambiguities
  - validation status
- `generated/rtl/payloads.sv` -- shared packed payload structs / enums /
  typedefs
- `generated/rtl/<module-name>.sv` -- one RTL file per Foundation module
- `generated/rtl/top.sv` -- top-level DUT wiring matching the model
- `generated/test/uvm_types_pkg.sv` -- sequence items, shared typedefs,
  and common testbench package definitions
- `generated/test/<name>_if.sv` -- virtual interfaces / clocking blocks
  used by the UVM driver and monitor side
- `generated/test/uvm_<name>_seq.sv` -- one or more sequences derived
  from planned test rows or coherent test families
- `generated/test/uvm_<name>_drv.sv` -- one per driver role
- `generated/test/uvm_<name>_mon.sv` -- one per monitor role
- `generated/test/uvm_<name>_sb.sv` -- one per scoreboard role when the
  Rust side uses scoreboard-style checking; otherwise emit explicit
  assertion/check helpers instead of forcing a scoreboard abstraction
- `generated/test/uvm_env.sv` -- UVM env that instantiates and connects
  the testbench components
- `generated/test/uvm_test_base.sv` -- shared base test
- `generated/test/uvm_test_<name>.sv` -- one per planned test row or
  per coherent test family when multiple rows differ only in parameters;
  do not collapse everything into only four category tests
- `generated/test/tb_top.sv` -- top-level testbench module
- `generated/test/sim.f` -- flat file list in compile order
- `generated/test/Makefile` -- minimal runnable simulation flow
- `generated/validation.md` -- what was statically validated, with tool
  commands and results; if no validation tool was available, say so
  explicitly

Every output file must stay under `generated/`. Do **NOT** modify
anything under `src/`, `tests/`, `.sim-flow/`, `docs/`, or any
non-generated file. When re-emitting an existing generated file,
overwrite the whole file rather than making partial edits.

## Required generation process

1. Read the source-of-truth documents and code listed above.
2. Classify each Foundation module into a hardware pattern before
   writing SV. Use the design-pattern guide explicitly:
   - simple pipeline / transform stage
   - custom stateful / hazard stage
   - feedback / kill / redirect stage
   - FIFO / queue / CDC / library wrapper
   - other pattern, if none of the above fit
3. Write `generated/manifest.md` first with:
   - the module list
   - the chosen pattern per module
   - the planned RTL file mapping
   - the planned UVM test mapping from `docs/test-plan/test-plan.md`
   - any ambiguity you had to resolve from spec/tests/code
4. Emit RTL, then emit the UVM testbench, then run static validation if
   any compile/lint tool is available.
5. If the appended `## Simulate and iterate` section is present, follow
   it after emission and continue fixing generated files until the SV
   simulation passes.

## Semantic mapping rules

### General rule

Do not mechanically translate Rust syntax into SV syntax. Translate
behavior:

- externally visible handshakes
- data transformations
- state updates
- latency / staging
- sideband and feedback effects
- test expectations

If multiple plausible RTL structures could implement the same external
behavior, prefer the one that most directly reflects the documented
pipeline mapping, decomposition, and testbench intent.

### Numeric and type semantics

Map Rust/Foundation types explicitly:

- `bool` -> `logic`
- `uN` -> `logic [N-1:0]`
- `iN` -> `logic signed [N-1:0]`
- enums -> `typedef enum logic [...]`
- payload structs -> `typedef struct packed`
- fixed-size arrays -> explicit packed/unpacked arrays as appropriate

Preserve:

- signedness
- truncation behavior
- wrapping arithmetic
- saturating arithmetic
- comparison semantics
- default/reset values

Never assume SV defaults happen to match Rust semantics. If the Rust
behavior relies on wrap, saturate, truncate, or explicit masking, spell
that out in SV instead of relying on accidental tool behavior.

### Evaluate / settle / update

Foundation's phase order is `evaluate -> settle -> update`.

Translate it as:

- `evaluate()` -> combinational next-output / next-state logic
- `settle()` -> same-cycle combinational resolution such as squash,
  redirect, sideband-driven overrides, or arbitration resolution
- `update()` -> clocked state commit

Do not collapse `settle()` and `update()` into a simplistic
"registered input, combinational output" template when the model uses
same-cycle sideband or feedback semantics.

Use `_q` for registered state and `_d` for next-state. Every
combinational block must fully assign defaults to avoid latches.

### Handshake and buffering

Ready/valid structure must preserve the Foundation model's externally
visible behavior:

- backpressure propagation
- transaction acceptance rules
- bubble insertion or occupancy behavior implied by the model
- latency boundaries from `pipeline-mapping.md`

The "register at every consumed input" invariant means each consumed
input crosses at least one register boundary before affecting later
registered behavior. It does **not** mean:

- every module has exactly one outstanding item
- every module can use the same one-deep shell
- every output valid is simply `in_valid_q`

Derive per-port ready/valid logic from the actual module behavior.

### Multi-input, multi-output, and multi-lane behavior

For modules with:

- multiple inputs
- multiple outputs
- arbitration
- optional inputs
- lane-parallel behavior

preserve the Rust behavior explicitly. Do not reduce them to a single
input/single output streaming stage. If a module's `HasLogic` iterates
lanes or conditionally consumes ports, the RTL must do the same.

### Sideband, feedback, kill, redirect

If the Rust model uses:

- sideband channels
- same-cycle squash / kill
- redirect / feedback ports
- control-only signals

model them as explicit SV control signals, combinational override paths,
or dedicated ports/interfaces. Do not silently drop them because they do
not fit a basic ready/valid shell.

### Payload structs

Every Rust payload struct becomes a SystemVerilog packed struct with
matching field widths and semantics. Field order and widths must match
the intent of the Rust payload as consumed by the model and tests.

If there is any ambiguity about field significance or signedness, resolve
it from the Rust type definitions and usage, then record the decision in
`generated/manifest.md`.

### Top-level wiring

Use the topology and stage boundaries from:

- `src/sim.rs` when present
- `docs/analysis/decomposition.md`
- `docs/analysis/pipeline-mapping.md`
- `docs/analysis/data-movement.md`

`top.sv` should be structurally readable against the model:

- module instance names should match the model where practical
- inter-stage nets should follow stage/operation naming
- feedback and sideband paths should be explicit

## UVM generation rules

### Preserve verification intent, not just component names

Use the testing guide in
`lib:docs/modeling-guide/04-testing-models.md` as the behavioral model
for what the verification environment needs to check.

The generated UVM environment must preserve:

- functional correctness checking
- ordering and latency behavior
- backpressure behavior
- scoreboard intent where the Rust side uses scoreboards
- explicit assertions where the Rust side uses assertion-style checking

Do not reduce the testbench to generic category smoke tests.

### Planned-test mapping

`docs/test-plan/test-plan.md` is the primary contract. For each planned test
row:

- emit a corresponding UVM test/sequence, or
- explicitly group it into a coherent parameterized family and record
  that grouping in `generated/manifest.md`

The category structure (`Smoke`, `Edge`, `Stress`, `Random`) is useful
organization, but it is **not** sufficient coverage by itself.

### Scoreboards vs explicit assertions

If the Rust tests use a scoreboard-style expected-vs-observed model,
generate a scoreboard. If they use direct assertions because duplicating
the full logic in a scoreboard would be misleading, preserve that style
in SV rather than forcing every check into a scoreboard.

### Reproducibility

Randomized tests must pin seeds and be rerunnable:

- `make sim TEST=<name>`
- simulator seed control such as `+UVM_SEED=<N>`

Use deterministic names and comments so failures can be tied back to the
planned test rows.

## Validation requirements

### Always do at least one of these

After emission, perform the strongest available validation that does not
violate the session mode:

1. If the appended `## Simulate and iterate` section is present, follow
   it fully.
2. Otherwise, if a SystemVerilog parser/linter/compiler is available in
   the environment, run a static compile/lint/elaboration pass on the
   generated files.
3. If no tool is available, write that fact explicitly in
   `generated/validation.md` and do not claim the output was validated.

Prefer:

- `make sim` / `make sim TEST=<name>` when the appendix is present
- otherwise a syntax/elaboration-oriented check such as Verilator or the
  configured simulator in compile-only mode, if available

Record in `generated/validation.md`:

- tool used
- command invoked
- which files/tests were checked
- success/failure summary
- any unvalidated gaps

## Constraints

- **Synthesizable RTL subset**. No `initial` blocks, no `#delays`, no
  `force/release`, no debug-only system tasks in RTL. RTL should be
  parseable by synthesis-oriented tools.
- **No inferred latches**. Fully assign defaults in combinational logic.
- **No accidental combinational loops**. Be careful with ready/valid and
  feedback paths.
- **No silent semantic weakening**. If the Rust model's behavior cannot
  be represented faithfully without a non-trivial assumption, record it
  in `generated/manifest.md`.
- **One file per major artifact**. Keep a readable one-to-one-ish
  mapping between Foundation modules/components and emitted SV files.
- **Comment the source mapping**. At the top of each generated file,
  include source-trace comments like
  `// generated from src/model/<file>.rs::<name>`.
- **Do not touch anything outside `generated/`**.

## Minimum acceptance bar

Before stopping, the generated artifacts should be good enough that a
hardware engineer can:

- trace each RTL module back to the Foundation model
- trace each UVM test back to the planned verification intent
- understand any unresolved assumptions from `generated/manifest.md`
- see whether the output was statically validated from
  `generated/validation.md`
