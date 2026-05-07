//! {{project-name}} library crate.
//!
//! Public API for the {{project-name}} model. Module authoring (payload
//! types, ConnectivityPlan, and module implementations) is filled in by
//! DM2d of the Direct Modeling Flow.
//!
//! ## Stable contract: `dump_topology`
//!
//! `dump_topology` is the entry point the sim-flow orchestrator (and
//! the Block Diagram tab in the dashboard) call to materialize a
//! topology netlist. It elaborates `model::top::Top::default()` and
//! lets `TopologyDumpArgs::elaborate_root` handle the requested
//! output formats (`--dump-hierarchy`, `--dump-netlist-json`,
//! `--dump-dot`, `--dump-mermaid`).
//!
//! The orchestrator does NOT rely on the agent to wire dump glue in
//! `main.rs`. The contract DM2d must preserve:
//!
//!   - `model::top::Top` exists and implements
//!     `Module + HasInstances + HasLogic`.
//!   - `Top::default()` returns a constructible instance (an empty
//!     stub before DM2d implements the body, the real top after).
//!   - The `dump_topology` function in this file stays callable with
//!     `&TopologyDumpArgs` and returns an `ElaborationError` (or
//!     equivalent error) on failure.
//!
//! Keeping these stable lets the dashboard render a (sparse) block
//! diagram even mid-DM2d so the user can see structural progress.

pub mod model;
pub mod sim;

use foundation_framework::TopologyDumpArgs;
use foundation_framework::model::hierarchy::ElaborationError;

/// Dump the project's topology using `args`. Called by `main.rs`
/// when any of the `--dump-*` flags are present, AND by the
/// sim-flow orchestrator's auto-render hook on the DM2d -> DM3a
/// boundary. Re-entrant; safe to call repeatedly.
///
/// **Do not modify this function's signature.** The orchestrator
/// looks up `dump_topology(&TopologyDumpArgs)` by name; renaming
/// it or changing the argument shape breaks the auto-render path.
///
/// The `default_constructed_unit_structs` allow exists because the
/// pre-DM2d stub `Top` is a unit struct -- clippy would otherwise
/// flag `Top::default()` as redundant. Once DM2d adds fields the
/// allow becomes a no-op; leave it in place so the lint stays
/// silent during the stub period when sim-flow regenerates the
/// project from this template.
pub fn dump_topology(args: &TopologyDumpArgs) -> Result<(), ElaborationError> {
    #[allow(clippy::default_constructed_unit_structs)]
    let top = model::top::Top::default();
    args.elaborate_root("top", top).map(|_| ())
}
