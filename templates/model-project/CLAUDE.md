# {{project-name}}

<!--
This file MUST stay in sync with AGENTS.md. When updating Claude Code
guidance, mirror the same content into AGENTS.md so Codex and Copilot
sessions receive identical project context.
-->

## What This Is

A sim-foundation model project using the Direct Modeling Flow. Models are
cycle-accurate hardware simulations built on the `foundation-framework`
crate. The orchestrator that drives this project is `sim-flow`; see
docs/architecture/ai-flow/ in the sim-foundation repository for flow
specifications.

## Project Structure

- `docs/impl-plan/` - Implementation plan (DM2c) -- `plan.md` index
  plus per-milestone `milestone-NN-<name>.md` files DM2d works through.
- `docs/test-plan/` - Verification plan (DM3a) -- `test-plan.md` index
  plus per-category `smoke.md` / `edge.md` / `stress.md` / `random.md`
  / `coverage.md` files DM3b/DM3c implement.
- `docs/perf-plan/` - Performance analysis plan (DM4a) -- `perf-plan.md`
  index plus per-milestone `perf-milestone-NN-<name>.md` files DM4b
  executes.
- `src/model/` - Module definitions, topology, hierarchy (DM2a/DM2b
  analysis; DM2d implementation)
- `src/sim.rs` - Simulation harness and runtime wiring (DM2d / DM3)
- `src/main.rs` - CLI entrypoint; honors `--run-id <id>` from sim-flow
- `tests/` - Self-checking verification tests (DM3a/DM3b/DM3c)
- `docs/analysis/` - Performance analysis reports (DM4)
- `.sim-flow/` - Flow state, config, and experiment index
- `.experiments/` - Per-run observability artifacts

## Key Framework Patterns

- Modules implement the `Module` trait with `HasLogic` and `HasInstances`
- Payload types flow through typed input / output ports
- Topology is declared via a `ConnectivityPlan`
- Phase model: evaluate -> settle -> update
- Tests use UVM-lite: Sequencer, Driver, Monitor, Scoreboard, SimEnv

## Run ID Contract

The `{{crate_name}}` binary accepts `--run-id <id>`. The sim-flow
orchestrator passes this flag for every simulation invocation. Thread the
value into `RunManifest::new(run_id)` and
`ObservabilityRunWriter::new(output_dir, run_id)` so tracking can
correlate `.obsv` artifacts with the experiments index.

## Flow State

This project is managed by the sim-flow orchestrator. Check
`.sim-flow/state.toml` for the current step and
`docs/critiques/` for the most recent critique output.

## Build Commands

- `cargo build`
- `cargo test`
- `cargo run -- --run-id local`
- `cargo run -- --dump-hierarchy`
- `cargo run -- --dump-netlist-json .sim-flow/block-diagram.netlist.json`

## Topology Dumps

`main.rs` flattens `foundation_framework::TopologyDumpArgs`, so the
binary accepts `--dump-hierarchy`, `--dump-dot`, `--dump-mermaid`,
`--render-mermaid`, and `--dump-netlist-json <PATH>`. Once DM2d defines
the model's topology, the wiring becomes
`cli.dump.elaborate_root("<root>", top)?`.

The sim-flow extension's Block Diagram tab runs `sim-flow block-diagram`,
which shells out to `cargo run -- --dump-netlist-json …` and feeds the
JSON through `tools/block-diagram` (Sugiyama -> SVG). Until DM2d wires
`elaborate_root`, the diagram tab will report a generation error.
