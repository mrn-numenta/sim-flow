//! Per-stage modules. Each stage takes the previous stage's output
//! and returns its own, threading a shared `warnings` vec for
//! diagnostics that land in the manifest.

pub mod chrome;
pub mod classify;
pub mod emit;
pub mod figures;
pub mod loading;
pub mod parse;
pub mod references;
