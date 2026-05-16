//! DM step registration with detailed gate checks per
//! `docs/architecture/ai-flow/02-direct-modeling-flow.md`.
//!
//! Gate checks are structural (file exists, regex matches, shell command
//! succeeds, critique scan). Semantic cross-file checks (e.g. every
//! operation name from `decomposition.md` appears in `pipeline-mapping.md`)
//! live in the corresponding critique prompt, not here.
//!
//! Per-step descriptors are split across `dm/tierN*.rs` submodules:
//!   - `tier0`: DM0 (spec) + DM1 (decomposition).
//!   - `tier2_plan`: DM2a / DM2b / DM2c / DM2cd (analysis + impl plan).
//!   - `tier2_impl`: DM2d (model implementation walk).
//!   - `tier3`: DM3a / DM3ad / DM3b / DM3c (test plan + testbench + tests).
//!   - `tier4`: DM4a / DM4ad / DM4b (perf plan + perf walk).
//!
//! Shared `GateCheck` constructors live in `dm/helpers.rs`.

mod helpers;

mod tier0;
mod tier2_impl;
mod tier2_plan;
mod tier3;
mod tier4;

#[cfg(test)]
mod tests;

use crate::steps::StepRegistry;

pub fn register(reg: &mut StepRegistry) {
    reg.register(tier0::dm0());
    reg.register(tier0::dm1());
    reg.register(tier2_plan::dm2a());
    reg.register(tier2_plan::dm2b());
    reg.register(tier2_plan::dm2c());
    reg.register(tier2_plan::dm2cd());
    reg.register(tier2_impl::dm2d());
    reg.register(tier3::dm3a());
    reg.register(tier3::dm3ad());
    reg.register(tier3::dm3b());
    reg.register(tier3::dm3c());
    reg.register(tier4::dm4a());
    reg.register(tier4::dm4ad());
    reg.register(tier4::dm4b());
}
