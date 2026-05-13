# SV1 - RTL Emission (work session)

You are executing step SV1 (RTL Emission) of the SystemVerilog
Conversion Flow. Prerequisite: SV0d gate passed.

## Goal

Translate the Foundation model in `src/` into synthesizable
SystemVerilog under `generated/rtl/`. SV1 is milestone-driven:
walk each `generated/plan/rtl-milestone-NN-*.md` file in order,
emit the `.sv` files it lists, mark each task `- [x]`, then stop
for the paired critique. Do NOT chain milestones.

## Inputs

- `generated/plan.md` -- the conversion plan and module table.
- `generated/plan/rtl-milestone-NN-<name>.md` -- the current
  milestone's task list. The first file with at least one `- [ ]`
  row is your current milestone.
- `src/model/<file>.rs` -- the Foundation modules the current
  milestone derives from. Read every relevant symbol's
  `evaluate()`, `settle()`, `update()`, port table, and payload
  type before writing SV.
- `docs/analysis/decomposition.md`, `pipeline-mapping.md`,
  `data-movement.md` -- stage boundaries, data edges, latency
  intent.
- `lib:docs/modeling-guide/06-design-patterns.md` -- the hardware
  pattern guide the plan classified each module against.
- `fw:api/toc.md` then specific `fw:api/pages/...` files when you
  need exact framework API semantics.

## Procedure

1. **Pick the current milestone**. Walk
   `generated/plan/rtl-milestone-NN-<name>.md` files in
   lexicographic order. First file with an open `- [ ]` row is
   yours. Read ONLY that milestone file plus its Trace
   predecessors; do NOT bulk-read later milestones.

2. **Walk tasks in order**. For each `- [ ]` row:
   - Read the referenced Foundation construct.
   - Emit the named `.sv` file under `generated/rtl/`.
   - At the top of every generated file, include a source-trace
     comment: `// generated from src/model/<file>.rs::<symbol>`.
   - Mark the row `- [x]` (the orchestrator also auto-ticks rows
     whose first backtick token resolves to a file on disk).

3. **Semantic mapping rules** (apply per file):

   - `bool` → `logic`; `uN` → `logic [N-1:0]`; `iN` →
     `logic signed [N-1:0]`; enums → `typedef enum logic [...]`;
     payload structs → `typedef struct packed`.
   - Preserve signedness, truncation, wrapping, saturation,
     comparison semantics, reset values. Never assume SV defaults
     happen to match Rust.
   - `evaluate()` → combinational next-output / next-state;
     `settle()` → same-cycle resolution (squash / redirect /
     arbitration); `update()` → clocked state commit. Use `_q`
     for registered state, `_d` for next-state. Fully assign
     defaults in combinational blocks (no inferred latches).
   - **Handshake invariant** (Foundation): every consumed input
     crosses at least one register stage before influencing later
     registered state or externally visible output. Derive per-
     port ready/valid from actual module behavior; do NOT collapse
     into a single one-deep shell.
   - **Multi-input / multi-output / lane-parallel**: preserve the
     Rust behavior. If `HasLogic` iterates lanes or conditionally
     consumes ports, the RTL must too.
   - **Sideband / feedback / kill / redirect**: model as explicit
     SV control signals or combinational override paths. Do NOT
     drop because they don't fit a basic ready/valid shell.

4. **`payloads.sv` first**. The plan's first RTL milestone always
   emits `generated/rtl/payloads.sv` -- shared packed structs,
   enums, typedefs. Field order and widths must match the Rust
   payload intent as consumed by the model and tests.

5. **`top.sv` last**. The plan's final RTL milestone emits
   `generated/rtl/top.sv` -- top-level wiring matching `src/sim.rs`
   (when present) and `pipeline-mapping.md`. Module instance names
   should match the model where practical; inter-stage nets follow
   stage / operation naming; feedback / sideband paths explicit.

6. **Surface the milestone-complete notice**:

   > `<milestone-name> complete; ready for critique.`
   > `<one-line summary: N files added, M tasks resolved, K
   > deferred items, any ambiguity flagged>`

   Do NOT proceed to the next milestone.

## Output

{{ output_intro }}

Files this step adds under `generated/rtl/` (per milestone task
list).

## Constraints

- **Synthesizable subset**. No `initial` blocks, no `#delays`, no
  `force/release`, no debug-only system tasks in RTL.
- **No inferred latches**. Fully assign defaults in combinational
  blocks.
- **No accidental combinational loops** on ready/valid or feedback
  paths.
- **One file per major module / shared-type group**. Keep a
  readable mapping between Foundation modules and emitted SV.
- **Comment the source mapping** at the top of every generated
  file.
- **Do not touch anything outside `generated/rtl/`** (the rest of
  `generated/` is SV2 / SV3 territory).
- **Do not modify `generated/plan.md` or any milestone stub** other
  than ticking your own task rows.

When the current milestone is fully emitted, stop. Do not write
`docs/critiques/SV1-critique.md`. Do not `/exit`.
