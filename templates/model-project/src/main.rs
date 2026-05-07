//! {{project-name}} simulation entry point.
//!
//! This binary honors the sim-flow `--run-id <id>` contract. When sim-flow
//! launches a simulation for tracking (see docs/architecture/ai-flow/
//! 04-experiment-tracking.md), it passes the run id here. The run id is
//! threaded into Foundation's RunManifest and ObservabilityRunWriter by
//! the model's simulation harness in `sim.rs` (filled in during DM2d).
//!
//! `--dump-netlist-json <path>` and the other topology dumps in
//! `TopologyDumpArgs` are wired UNCONDITIONALLY through
//! `{{crate_name}}::dump_topology`. The orchestrator (and the
//! Block Diagram tab in the dashboard) call this binary with
//! `--dump-netlist-json` and rely on the netlist being written
//! regardless of whether DM2d has filled in the model body yet --
//! the template ships a stub `model::top::Top` so an empty
//! diagram renders before the agent does any work, and the agent
//! replaces the stub body in DM2d while keeping the same type
//! name + `Default` impl.

use clap::Parser;
use foundation_framework::TopologyDumpArgs;

/// CLI for the {{project-name}} model binary.
#[derive(Debug, Parser)]
#[command(name = "{{crate_name}}", version, about = "{{project-name}} cycle-accurate model")]
struct Cli {
    /// Run identifier injected by the sim-flow orchestrator. Propagated to
    /// Foundation's `RunManifest::new(run_id)` and observability writers
    /// so tracking can correlate `.obsv` artifacts with the experiments
    /// index.
    #[arg(long, default_value = "local")]
    run_id: String,

    /// Topology dump flags: `--dump-hierarchy`, `--dump-netlist-json <PATH>`,
    /// `--dump-dot`, `--dump-mermaid`, `--render-mermaid`. The dispatch
    /// below threads them into `{{crate_name}}::dump_topology` so the
    /// netlist lands on disk without the agent having to wire
    /// `cli.dump.elaborate_root(...)` manually.
    #[command(flatten)]
    dump: TopologyDumpArgs,
}

fn main() {
    let cli = Cli::parse();
    if cli.dump.should_dump_any() {
        // Topology dump path. Never depends on the agent's main.rs
        // edits -- `lib.rs::dump_topology` elaborates
        // `model::top::Top::default()` (a stub before DM2d, the
        // real model after) and emits the requested artifacts.
        match {{crate_name}}::dump_topology(&cli.dump) {
            Ok(_) => return,
            Err(err) => {
                eprintln!("{{crate_name}}: dump_topology failed: {err}");
                std::process::exit(1);
            }
        }
    }
    println!("{{project-name}}: run_id = {}", cli.run_id);
    println!("(simulation harness not yet implemented; see DM2d)");
}
