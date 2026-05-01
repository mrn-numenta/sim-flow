//! {{project-name}} simulation entry point.
//!
//! This binary honors the sim-flow `--run-id <id>` contract. When sim-flow
//! launches a simulation for tracking (see docs/architecture/ai-flow/
//! 04-experiment-tracking.md), it passes the run id here. The run id is
//! threaded into Foundation's RunManifest and ObservabilityRunWriter by
//! the model's simulation harness in `sim.rs` (filled in during DM2d).

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
    /// `--dump-dot`, `--dump-mermaid`, `--render-mermaid`. DM2d wires these
    /// into `TopologyDumpArgs::elaborate_root(...)` once a topology exists.
    /// The sim-flow extension's Block Diagram tab calls
    /// `--dump-netlist-json` to render the design.
    #[command(flatten)]
    dump: TopologyDumpArgs,
}

fn main() {
    let cli = Cli::parse();
    println!("{{project-name}}: run_id = {}", cli.run_id);
    if cli.dump.should_dump_any() {
        eprintln!(
            "(no topology to dump yet; DM2d wires `cli.dump.elaborate_root(...)` once \
             the model's topology is defined)"
        );
    } else {
        println!("(simulation harness not yet implemented; see DM2d)");
    }
}
