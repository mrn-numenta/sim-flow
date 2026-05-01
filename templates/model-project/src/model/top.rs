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
