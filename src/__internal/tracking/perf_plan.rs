//! Declarative perf-plan execution format.
//!
//! Lives at `<project>/docs/perf-plan/plan.toml` (path is
//! convention, not a hardcoded requirement). The plan replaces hand-
//! written Rust study setup with a declarative description of: the
//! DOE pattern + budget, the workloads to exercise, the metrics +
//! targets the plan cares about, the studies (single-config / 1D /
//! 2D / Pareto), and the charts to render.
//!
//! Closes the "no declarative perf-plan executor format" gap called
//! out in `docs/brainstorming/perf-plan-formalization.md`. The
//! `sim-flow perf-run <plan.toml>` subcommand (separate module) is
//! the consumer; this module is the schema + loader + validator only,
//! and is the place to evolve the format. Anything the executor
//! needs at runtime should live in this module so the schema and
//! its consumers stay in sync.
//!
//! Schema (TOML):
//!
//! ```toml
//! schema_version = 1
//!
//! [plan]
//! goal = "characterize"           # characterize | optimize | validate
//! doe_pattern = "oat"             # oat | oat-pair-screen | plackett-burman |
//!                                 # latin-hypercube | rsm | bayes
//! budget_runs = 50                # cap on total simulation runs
//! description = "RGB toy perf characterization"
//!
//! [[workloads]]
//! name = "random-1k-burst"
//! description = "Random pixel arrivals, 1000 cycles"
//!
//! [[metrics]]
//! id = "output_latency_p99"
//! probe = "rgb_pair.output_latency"
//! aggregate = "p99"               # average | p50 | p90 | p99 | peak | total
//! target_max = 5                  # optional ceiling
//!
//! [[metrics]]
//! id = "throughput_avg"
//! probe = "rgb_pair.output_throughput"
//! aggregate = "average"
//! target_min = 0.95               # optional floor
//!
//! [[studies]]
//! name = "baseline"
//! kind = "type1"                  # type1 | type2 | type3 | pareto
//! workload = "random-1k-burst"
//!
//! [[studies]]
//! name = "sweep-fifo-depth"
//! kind = "type2"
//! workload = "random-1k-burst"
//! parameter = "fifo_depth"        # must be declared in variants.toml
//!
//! [[charts]]
//! study = "sweep-fifo-depth"
//! kind = "line"                   # line | scatter | heatmap | bar | roofline
//! x_axis = "fifo_depth"
//! y_axis = "throughput_avg"
//! ```

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::tracking::variants::VariantManifest;
use crate::{Error, Result};

const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// Convention path under the project root.
pub const DEFAULT_PLAN_PATH: &str = "docs/perf-plan/plan.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PerfPlan {
    pub schema_version: u32,
    pub plan: PlanHeader,
    #[serde(default)]
    pub workloads: Vec<Workload>,
    #[serde(default)]
    pub metrics: Vec<Metric>,
    #[serde(default)]
    pub studies: Vec<Study>,
    #[serde(default)]
    pub charts: Vec<Chart>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanHeader {
    pub goal: PlanGoal,
    pub doe_pattern: DoePattern,
    pub budget_runs: u32,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PlanGoal {
    Characterize,
    Optimize,
    Validate,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DoePattern {
    Oat,
    OatPairScreen,
    PlackettBurman,
    LatinHypercube,
    Rsm,
    Bayes,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Workload {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Metric {
    pub id: String,
    pub probe: String,
    pub aggregate: MetricAggregate,
    #[serde(default)]
    pub target_min: Option<toml::Value>,
    #[serde(default)]
    pub target_max: Option<toml::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MetricAggregate {
    Average,
    P50,
    P90,
    P99,
    Peak,
    Total,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Study {
    /// Single run: no parameters varied.
    Type1 { name: String, workload: String },
    /// 1D sweep across approved values of one parameter.
    Type2 {
        name: String,
        workload: String,
        parameter: String,
        /// Optional: override the approved values. Each value must
        /// still be in the variant manifest. Empty/missing means
        /// "use all approved values."
        #[serde(default)]
        values: Vec<toml::Value>,
    },
    /// 2D sweep cross-product of two parameters.
    Type3 {
        name: String,
        workload: String,
        parameters: [String; 2],
    },
    /// Pareto exploration -- the executor decides how to populate the
    /// trade-off frontier. Schema accepts it; executor implementation
    /// is a follow-up.
    Pareto {
        name: String,
        workload: String,
        objectives: Vec<String>,
    },
}

impl Study {
    pub fn name(&self) -> &str {
        match self {
            Study::Type1 { name, .. }
            | Study::Type2 { name, .. }
            | Study::Type3 { name, .. }
            | Study::Pareto { name, .. } => name,
        }
    }

    pub fn workload(&self) -> &str {
        match self {
            Study::Type1 { workload, .. }
            | Study::Type2 { workload, .. }
            | Study::Type3 { workload, .. }
            | Study::Pareto { workload, .. } => workload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Chart {
    pub study: String,
    pub kind: ChartKind,
    pub x_axis: String,
    pub y_axis: String,
    /// Used by heatmap to declare the cell value (a metric id).
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ChartKind {
    Line,
    Scatter,
    Heatmap,
    Bar,
    Roofline,
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("plan.toml: unsupported schema_version {found} (this build supports {supported})")]
    UnsupportedSchemaVersion { found: u32, supported: u32 },
    #[error("plan.toml: budget_runs must be > 0")]
    BudgetRunsZero,
    #[error("plan.toml: duplicate workload name `{name}`")]
    DuplicateWorkload { name: String },
    #[error("plan.toml: duplicate metric id `{id}`")]
    DuplicateMetric { id: String },
    #[error("plan.toml: duplicate study name `{name}`")]
    DuplicateStudy { name: String },
    #[error("plan.toml study `{study}`: references undeclared workload `{workload}`")]
    StudyWorkloadUndeclared { study: String, workload: String },
    #[error("plan.toml study `{study}`: parameter `{parameter}` is not in variants.toml")]
    StudyParameterNotInVariants { study: String, parameter: String },
    #[error(
        "plan.toml study `{study}`: value {value:?} for parameter `{parameter}` is not approved \
         in variants.toml"
    )]
    StudyValueNotApproved {
        study: String,
        parameter: String,
        value: toml::Value,
    },
    #[error("plan.toml chart for `{study}`: study not declared")]
    ChartStudyUndeclared { study: String },
    #[error("plan.toml chart for `{study}`: y_axis `{axis}` is not a declared metric id")]
    ChartYAxisNotAMetric { study: String, axis: String },
    #[error("plan.toml chart for `{study}`: heatmap requires `value` (a metric id)")]
    HeatmapMissingValue { study: String },
}

impl PerfPlan {
    /// Validate the plan against the (optional) variant manifest.
    /// Passing `None` skips parameter-approval checks; the executor
    /// always passes the manifest when one exists, but tests can
    /// validate a plan in isolation.
    pub fn validate(
        &self,
        variants: Option<&VariantManifest>,
    ) -> std::result::Result<(), ValidationError> {
        if self.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(ValidationError::UnsupportedSchemaVersion {
                found: self.schema_version,
                supported: SUPPORTED_SCHEMA_VERSION,
            });
        }
        if self.plan.budget_runs == 0 {
            return Err(ValidationError::BudgetRunsZero);
        }
        let mut workload_names: BTreeSet<&str> = BTreeSet::new();
        for w in &self.workloads {
            if !workload_names.insert(w.name.as_str()) {
                return Err(ValidationError::DuplicateWorkload {
                    name: w.name.clone(),
                });
            }
        }
        let mut metric_ids: BTreeSet<&str> = BTreeSet::new();
        for m in &self.metrics {
            if !metric_ids.insert(m.id.as_str()) {
                return Err(ValidationError::DuplicateMetric { id: m.id.clone() });
            }
        }
        let mut study_names: BTreeSet<&str> = BTreeSet::new();
        for s in &self.studies {
            if !study_names.insert(s.name()) {
                return Err(ValidationError::DuplicateStudy {
                    name: s.name().to_string(),
                });
            }
            if !workload_names.contains(s.workload()) {
                return Err(ValidationError::StudyWorkloadUndeclared {
                    study: s.name().to_string(),
                    workload: s.workload().to_string(),
                });
            }
            self.validate_study_parameters(s, variants)?;
        }
        for c in &self.charts {
            if !study_names.contains(c.study.as_str()) {
                return Err(ValidationError::ChartStudyUndeclared {
                    study: c.study.clone(),
                });
            }
            if !metric_ids.contains(c.y_axis.as_str()) {
                return Err(ValidationError::ChartYAxisNotAMetric {
                    study: c.study.clone(),
                    axis: c.y_axis.clone(),
                });
            }
            if c.kind == ChartKind::Heatmap && c.value.is_none() {
                return Err(ValidationError::HeatmapMissingValue {
                    study: c.study.clone(),
                });
            }
        }
        Ok(())
    }

    fn validate_study_parameters(
        &self,
        study: &Study,
        variants: Option<&VariantManifest>,
    ) -> std::result::Result<(), ValidationError> {
        let Some(manifest) = variants else {
            return Ok(());
        };
        match study {
            Study::Type1 { .. } | Study::Pareto { .. } => Ok(()),
            Study::Type2 {
                name,
                parameter,
                values,
                ..
            } => {
                if manifest.parameter(parameter).is_none() {
                    return Err(ValidationError::StudyParameterNotInVariants {
                        study: name.clone(),
                        parameter: parameter.clone(),
                    });
                }
                for value in values {
                    if !manifest.is_parameter_value_approved(parameter, value) {
                        return Err(ValidationError::StudyValueNotApproved {
                            study: name.clone(),
                            parameter: parameter.clone(),
                            value: value.clone(),
                        });
                    }
                }
                Ok(())
            }
            Study::Type3 {
                name, parameters, ..
            } => {
                for parameter in parameters {
                    if manifest.parameter(parameter).is_none() {
                        return Err(ValidationError::StudyParameterNotInVariants {
                            study: name.clone(),
                            parameter: parameter.clone(),
                        });
                    }
                }
                Ok(())
            }
        }
    }
}

/// Load + validate a plan from `path`. If a `variants.toml` sits
/// next to the plan (or under the project root), the caller can
/// pass it to `validate()` separately for cross-checking; this
/// function only validates the plan's internal consistency.
pub fn load(path: &Path) -> Result<PerfPlan> {
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let plan: PerfPlan = toml::from_str(&text).map_err(|source| Error::TomlParse {
        path: path.to_path_buf(),
        source,
    })?;
    plan.validate(None)
        .map_err(|err| Error::Config(format!("{}: {err}", path.display())))?;
    Ok(plan)
}

/// Convenience: load the project's default plan if it exists.
pub fn load_project(project_dir: &Path) -> Result<Option<PerfPlan>> {
    let path: PathBuf = project_dir.join(DEFAULT_PLAN_PATH);
    if !path.exists() {
        return Ok(None);
    }
    load(&path).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_valid_plan() -> &'static str {
        r#"
schema_version = 1

[plan]
goal = "characterize"
doe_pattern = "oat"
budget_runs = 50
description = "RGB toy perf characterization"

[[workloads]]
name = "random-1k-burst"

[[workloads]]
name = "full-frame-continuous"

[[metrics]]
id = "output_latency_p99"
probe = "rgb_pair.output_latency"
aggregate = "p99"
target_max = 5

[[metrics]]
id = "throughput_avg"
probe = "rgb_pair.output_throughput"
aggregate = "average"

[[studies]]
name = "baseline"
kind = "type1"
workload = "random-1k-burst"

[[studies]]
name = "sweep-fifo-depth"
kind = "type2"
workload = "random-1k-burst"
parameter = "fifo_depth"

[[charts]]
study = "sweep-fifo-depth"
kind = "line"
x_axis = "fifo_depth"
y_axis = "throughput_avg"
"#
    }

    fn variants_with_fifo_depth() -> VariantManifest {
        toml::from_str(
            r#"
schema_version = 1

[parameters.fifo_depth]
values = [8, 16, 32, 64]
default = 16
"#,
        )
        .expect("valid variants")
    }

    #[test]
    fn loads_and_validates_small_plan() {
        let plan: PerfPlan = toml::from_str(small_valid_plan()).expect("parse");
        plan.validate(None).expect("validates without variants");
        let variants = variants_with_fifo_depth();
        plan.validate(Some(&variants))
            .expect("validates with variants");
    }

    #[test]
    fn rejects_zero_budget_runs() {
        let plan: PerfPlan = toml::from_str(
            r#"
schema_version = 1

[plan]
goal = "characterize"
doe_pattern = "oat"
budget_runs = 0
"#,
        )
        .expect("parse");
        let err = plan.validate(None).expect_err("should reject zero budget");
        assert!(matches!(err, ValidationError::BudgetRunsZero));
    }

    #[test]
    fn rejects_unsupported_schema_version() {
        let plan: PerfPlan = toml::from_str(
            r#"
schema_version = 99

[plan]
goal = "characterize"
doe_pattern = "oat"
budget_runs = 10
"#,
        )
        .expect("parse");
        let err = plan
            .validate(None)
            .expect_err("should reject future version");
        assert!(matches!(
            err,
            ValidationError::UnsupportedSchemaVersion { found: 99, .. }
        ));
    }

    #[test]
    fn rejects_duplicate_workload() {
        let plan: PerfPlan = toml::from_str(
            r#"
schema_version = 1

[plan]
goal = "characterize"
doe_pattern = "oat"
budget_runs = 10

[[workloads]]
name = "foo"

[[workloads]]
name = "foo"
"#,
        )
        .expect("parse");
        let err = plan.validate(None).expect_err("duplicate workload");
        assert!(matches!(err, ValidationError::DuplicateWorkload { .. }));
    }

    #[test]
    fn rejects_study_referencing_undeclared_workload() {
        let plan: PerfPlan = toml::from_str(
            r#"
schema_version = 1

[plan]
goal = "characterize"
doe_pattern = "oat"
budget_runs = 10

[[studies]]
name = "test"
kind = "type1"
workload = "missing"
"#,
        )
        .expect("parse");
        let err = plan.validate(None).expect_err("undeclared workload");
        assert!(matches!(
            err,
            ValidationError::StudyWorkloadUndeclared { .. }
        ));
    }

    #[test]
    fn rejects_study_with_unapproved_parameter_value() {
        let plan: PerfPlan = toml::from_str(
            r#"
schema_version = 1

[plan]
goal = "characterize"
doe_pattern = "oat"
budget_runs = 10

[[workloads]]
name = "foo"

[[studies]]
name = "sweep"
kind = "type2"
workload = "foo"
parameter = "fifo_depth"
values = [8, 99]
"#,
        )
        .expect("parse");
        let variants = variants_with_fifo_depth();
        let err = plan
            .validate(Some(&variants))
            .expect_err("99 is not approved");
        assert!(matches!(err, ValidationError::StudyValueNotApproved { .. }));
    }

    #[test]
    fn rejects_chart_referencing_undeclared_study() {
        let plan: PerfPlan = toml::from_str(
            r#"
schema_version = 1

[plan]
goal = "characterize"
doe_pattern = "oat"
budget_runs = 10

[[metrics]]
id = "m"
probe = "p"
aggregate = "average"

[[charts]]
study = "ghost"
kind = "line"
x_axis = "x"
y_axis = "m"
"#,
        )
        .expect("parse");
        let err = plan.validate(None).expect_err("undeclared study");
        assert!(matches!(err, ValidationError::ChartStudyUndeclared { .. }));
    }

    #[test]
    fn rejects_chart_with_y_axis_not_a_metric() {
        let plan: PerfPlan = toml::from_str(
            r#"
schema_version = 1

[plan]
goal = "characterize"
doe_pattern = "oat"
budget_runs = 10

[[workloads]]
name = "w"

[[studies]]
name = "s"
kind = "type1"
workload = "w"

[[charts]]
study = "s"
kind = "line"
x_axis = "x"
y_axis = "not_a_metric"
"#,
        )
        .expect("parse");
        let err = plan.validate(None).expect_err("y_axis not a metric");
        assert!(matches!(err, ValidationError::ChartYAxisNotAMetric { .. }));
    }

    #[test]
    fn rejects_heatmap_without_value() {
        let plan: PerfPlan = toml::from_str(
            r#"
schema_version = 1

[plan]
goal = "characterize"
doe_pattern = "oat"
budget_runs = 10

[[workloads]]
name = "w"

[[metrics]]
id = "m"
probe = "p"
aggregate = "average"

[[studies]]
name = "s"
kind = "type3"
workload = "w"
parameters = ["a", "b"]

[[charts]]
study = "s"
kind = "heatmap"
x_axis = "a"
y_axis = "m"
"#,
        )
        .expect("parse");
        let err = plan.validate(None).expect_err("heatmap needs value");
        assert!(matches!(err, ValidationError::HeatmapMissingValue { .. }));
    }

    #[test]
    fn load_project_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_project(tmp.path()).expect("ok").is_none());
    }

    #[test]
    fn study_accessors_work_for_each_variant() {
        let study = Study::Type2 {
            name: "swp".into(),
            workload: "w".into(),
            parameter: "p".into(),
            values: vec![],
        };
        assert_eq!(study.name(), "swp");
        assert_eq!(study.workload(), "w");
    }
}
