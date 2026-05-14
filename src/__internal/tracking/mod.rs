//! Experiment tracking: SQLite index, run recording, baselines, sweeps.
//!
//! See `docs/architecture/ai-flow/04-experiment-tracking.md` for the design
//! and `docs/plan/ai-flow/04-phase-experiment-tracking.md` for the
//! milestones this module implements.

pub mod baseline;
pub mod git_state;
pub mod index;
pub mod metrics;
pub mod perf_plan;
pub mod perf_run;
pub mod run_recording;
pub mod sweep;
pub mod variants;

pub use baseline::{BaselineDelta, BaselineRecord};
pub use git_state::GitState;
pub use index::{ExperimentIndex, RunRow, experiments_db_path};
pub use perf_plan::{
    Chart, ChartKind, DoePattern, Metric, MetricAggregate, PerfPlan, PlanGoal, PlanHeader, Study,
    Workload,
};
pub use run_recording::{RecordRunOptions, RecordedRun};
pub use sweep::{SweepDefinition, SweepResults};
pub use variants::{
    MANIFEST_FILENAME, ModuleVariant, ParameterVariant, ValidationError, VariantManifest,
};
