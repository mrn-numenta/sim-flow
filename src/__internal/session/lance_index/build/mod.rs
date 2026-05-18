//! Build pipelines (Chapter 3 §3.9).

pub mod framework;
pub mod spec;

pub use framework::{FrameworkBuildOpts, FrameworkBuildOutcome, build_framework_index};
pub use spec::{
    SpecBuildOpts, SpecBuildOutcome, build_cross_spec_refs, build_signal_table_rows,
    build_spec_chunks, build_spec_index,
};
