//! Top-level module for {{project-name}}.
//!
//! Filled in during DM2d. The expected pattern is:
//!
//! ```ignore
//! use foundation_framework::model::{Module, HasLogic, HasInstances};
//!
//! pub struct Top { /* child modules, config */ }
//!
//! impl Module for Top { /* ... */ }
//! impl HasInstances for Top { /* ConnectivityPlan wiring */ }
//! impl HasLogic for Top { /* evaluate / settle / update */ }
//! ```
//!
//! ## Stable contract for the orchestrator's block-diagram render
//!
//! The sim-flow orchestrator calls `crate::dump_topology(&args)`
//! on the DM2d -> DM3a advance boundary so the dashboard renders
//! a block diagram automatically. That call elaborates
//! `Top::default()`. To keep that path working through the entire
//! flow:
//!
//!   - The type must remain `pub struct Top` in this module.
//!   - `Top: Default` must remain valid (use `#[derive(Default)]`
//!     when the fields support it; otherwise hand-implement
//!     `Default` returning the empty / pre-elaborate state).
//!   - `Top` must implement `Module + HasInstances + HasLogic`.
//!
//! The stub below ships an empty Top so the netlist render
//! produces a valid (if sparse) diagram before DM2d implements
//! the model body.

use foundation_framework::impl_structural_has_logic;
use foundation_framework::{HasInstances, Module};

/// Stub top module. DM2d replaces the body with the real
/// hierarchy + connectivity but keeps the type name + `Default`
/// impl + the three trait impls so `crate::dump_topology` keeps
/// compiling.
#[derive(Clone, Debug, Default)]
pub struct Top;

impl Module for Top {
    fn module_name(&self) -> &'static str {
        "top"
    }
}

impl HasInstances for Top {}

impl_structural_has_logic!(Top);
