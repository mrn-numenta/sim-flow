//! Perf-plan executor.
//!
//! Reads a validated `PerfPlan` and drives the per-study invocations
//! that the plan declares: one `cargo run` + `record_run` per cell
//! (Type1 = 1 cell, Type2 = N cells, Type3 = M×N cells), with a
//! parent run grouping per study. Stops invoking once `budget_runs`
//! is reached so a runaway plan can't burn unbounded simulation
//! time.
//!
//! Chart rendering and the actual stats-aggregation against
//! `experiments.db` live in a separate `perf_report` module so the
//! executor stays focused on "drive the runs, record the rows."
//!
//! Bayes / Plackett-Burman / Latin-Hypercube / RSM study patterns
//! are NOT implemented yet -- the schema accepts the enum variants
//! so plans can declare intent today, but `run()` returns
//! `Error::Config` for Pareto studies and never receives Plackett
//! / Latin / RSM / Bayes (those aren't in the `Study` enum; they
//! live on `DoePattern` and apply at the *plan* level). When those
//! patterns land they'll plug in at the "choose cells for this
//! study" step.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::tracking::perf_plan::{PerfPlan, Study};
use crate::tracking::run_recording::{RecordRunOptions, record_run};
use crate::tracking::variants::VariantManifest;
use crate::{Error, Result};

/// Result of running a single study cell -- one binary invocation
/// + one `record_run` row.
#[derive(Debug, Clone)]
pub struct StudyCell {
    pub run_id: String,
    pub parameters: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone)]
pub struct StudyResult {
    pub study_name: String,
    pub parent_run_id: String,
    pub cells: Vec<StudyCell>,
}

#[derive(Debug, Clone)]
pub struct PerfRunResults {
    pub studies: Vec<StudyResult>,
    pub total_runs: usize,
    pub budget_reached: bool,
}

/// Run `plan` end-to-end. `variants` may be `None` if the project
/// has no manifest yet, in which case Type2 studies must include
/// explicit `values` (the executor cannot enumerate the parameter's
/// approved values without it).
///
/// The function is bookkeeping-honest: if the model binary is
/// absent it still records the run row but skips invocation, which
/// keeps test execution fast and lets the perf plan be validated
/// against the index without a built model.
pub fn run(
    project_dir: &Path,
    plan: &PerfPlan,
    variants: Option<&VariantManifest>,
) -> Result<PerfRunResults> {
    let dot_sim_flow = project_dir.join(".sim-flow");
    let binary_path = resolve_default_binary(project_dir);

    let budget = plan.plan.budget_runs as usize;
    let mut total_runs = 0_usize;
    let mut studies = Vec::with_capacity(plan.studies.len());
    let mut budget_reached = false;

    for study in &plan.studies {
        if total_runs >= budget {
            budget_reached = true;
            break;
        }
        let parent_desc = format!("plan-{}", study.name());
        let parent = record_run(
            project_dir,
            &dot_sim_flow,
            &RecordRunOptions {
                description: parent_desc,
                workload: Some(study.workload().to_string()),
                study: Some(study.name().to_string()),
                tags: vec!["plan-parent".to_string()],
                ..Default::default()
            },
        )?;

        let cells = study_cells(study, variants)?;
        let mut recorded_cells = Vec::with_capacity(cells.len());
        for cell_spec in cells {
            if total_runs >= budget {
                budget_reached = true;
                break;
            }
            let cell_run_id = format!("{}-{}", parent.run_id, cell_spec.suffix);
            let recorded = record_run(
                project_dir,
                &dot_sim_flow,
                &RecordRunOptions {
                    description: cell_run_id.clone(),
                    workload: Some(study.workload().to_string()),
                    study: Some(study.name().to_string()),
                    parent_run_id: Some(parent.run_id.clone()),
                    tags: vec!["plan-cell".to_string()],
                    ..Default::default()
                },
            )?;
            invoke_binary(&binary_path, &recorded.run_id, &cell_spec.parameters);
            recorded_cells.push(StudyCell {
                run_id: recorded.run_id,
                parameters: cell_spec.parameters,
            });
            total_runs += 1;
        }

        studies.push(StudyResult {
            study_name: study.name().to_string(),
            parent_run_id: parent.run_id,
            cells: recorded_cells,
        });
    }

    Ok(PerfRunResults {
        studies,
        total_runs,
        budget_reached,
    })
}

/// One cell of a study before it's recorded: the parameter
/// assignment and a stable suffix for the run-id.
struct CellSpec {
    suffix: String,
    parameters: BTreeMap<String, toml::Value>,
}

fn study_cells(study: &Study, variants: Option<&VariantManifest>) -> Result<Vec<CellSpec>> {
    match study {
        Study::Type1 { .. } => Ok(vec![CellSpec {
            suffix: "base".to_string(),
            parameters: BTreeMap::new(),
        }]),
        Study::Type2 {
            name,
            parameter,
            values,
            ..
        } => {
            let cell_values = if !values.is_empty() {
                values.clone()
            } else {
                let manifest = variants.ok_or_else(|| {
                    Error::Config(format!(
                        "perf-plan study `{name}`: Type2 with no explicit \
                         `values` requires variants.toml to enumerate approved \
                         values for `{parameter}`"
                    ))
                })?;
                manifest
                    .parameter(parameter)
                    .ok_or_else(|| {
                        Error::Config(format!(
                            "perf-plan study `{name}`: parameter `{parameter}` \
                             is not declared in variants.toml"
                        ))
                    })?
                    .values
                    .clone()
            };
            Ok(cell_values
                .into_iter()
                .map(|v| {
                    let suffix = format!("{parameter}-{}", value_suffix(&v));
                    let mut params = BTreeMap::new();
                    params.insert(parameter.clone(), v);
                    CellSpec {
                        suffix,
                        parameters: params,
                    }
                })
                .collect())
        }
        Study::Type3 {
            name, parameters, ..
        } => {
            let manifest = variants.ok_or_else(|| {
                Error::Config(format!(
                    "perf-plan study `{name}`: Type3 cross-product requires \
                     variants.toml to enumerate approved values"
                ))
            })?;
            let [pa, pb] = parameters;
            let va = manifest
                .parameter(pa)
                .ok_or_else(|| {
                    Error::Config(format!(
                        "perf-plan study `{name}`: parameter `{pa}` not in variants.toml"
                    ))
                })?
                .values
                .clone();
            let vb = manifest
                .parameter(pb)
                .ok_or_else(|| {
                    Error::Config(format!(
                        "perf-plan study `{name}`: parameter `{pb}` not in variants.toml"
                    ))
                })?
                .values
                .clone();
            let mut out = Vec::with_capacity(va.len().saturating_mul(vb.len()));
            for a in &va {
                for b in &vb {
                    let suffix = format!("{pa}-{}-{pb}-{}", value_suffix(a), value_suffix(b));
                    let mut params = BTreeMap::new();
                    params.insert(pa.clone(), a.clone());
                    params.insert(pb.clone(), b.clone());
                    out.push(CellSpec {
                        suffix,
                        parameters: params,
                    });
                }
            }
            Ok(out)
        }
        Study::Pareto { name, .. } => Err(Error::Config(format!(
            "perf-plan study `{name}`: Pareto exploration is not yet \
             implemented in the executor. Declare your fronts as \
             explicit Type3 studies for now."
        ))),
    }
}

fn resolve_default_binary(project_dir: &Path) -> PathBuf {
    // Convention: project's binary is named after the project dir
    // and lives under `target/debug/`. Same convention DM4b uses
    // when calling `cargo run`.
    let name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("model");
    project_dir.join("target/debug").join(name)
}

fn invoke_binary(binary: &Path, run_id: &str, parameters: &BTreeMap<String, toml::Value>) {
    if !binary.exists() {
        return;
    }
    let mut cmd = Command::new(binary);
    cmd.arg("--run-id").arg(run_id);
    for (param, value) in parameters {
        let mut arg_name = String::from("--");
        for ch in param.chars() {
            if ch == '.' || ch == '_' {
                arg_name.push('-');
            } else {
                arg_name.push(ch);
            }
        }
        cmd.arg(&arg_name).arg(value_arg(value));
    }
    let _ = cmd.status();
}

fn value_suffix(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => format_float_suffix(*f),
        toml::Value::Boolean(b) => b.to_string(),
        other => sanitize_suffix(&other.to_string()),
    }
}

fn value_arg(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn format_float_suffix(f: f64) -> String {
    let formatted = format!("{f}");
    formatted.replace('.', "p")
}

fn sanitize_suffix(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracking::perf_plan::PerfPlan;

    fn make_plan(toml_text: &str) -> PerfPlan {
        toml::from_str(toml_text).expect("parse plan")
    }

    fn make_variants(toml_text: &str) -> VariantManifest {
        toml::from_str(toml_text).expect("parse variants")
    }

    fn project_dir() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".sim-flow")).unwrap();
        tmp
    }

    #[test]
    fn type1_study_produces_one_cell() {
        let plan = make_plan(
            r#"
schema_version = 1

[plan]
goal = "characterize"
doe_pattern = "oat"
budget_runs = 10

[[workloads]]
name = "w"

[[studies]]
name = "baseline"
kind = "type1"
workload = "w"
"#,
        );
        let tmp = project_dir();
        let results = run(tmp.path(), &plan, None).expect("run");
        assert_eq!(results.studies.len(), 1);
        assert_eq!(results.studies[0].cells.len(), 1);
        assert_eq!(results.total_runs, 1);
        assert!(!results.budget_reached);
    }

    #[test]
    fn type2_study_uses_variant_manifest_values() {
        let plan = make_plan(
            r#"
schema_version = 1

[plan]
goal = "characterize"
doe_pattern = "oat"
budget_runs = 50

[[workloads]]
name = "w"

[[studies]]
name = "sweep-depth"
kind = "type2"
workload = "w"
parameter = "fifo_depth"
"#,
        );
        let variants = make_variants(
            r#"
schema_version = 1

[parameters.fifo_depth]
values = [8, 16, 32]
default = 16
"#,
        );
        let tmp = project_dir();
        let results = run(tmp.path(), &plan, Some(&variants)).expect("run");
        assert_eq!(results.studies[0].cells.len(), 3);
        assert_eq!(results.total_runs, 3);
        // Run-id suffixes should mention the parameter+value.
        let cell_ids: Vec<&str> = results.studies[0]
            .cells
            .iter()
            .map(|c| c.run_id.as_str())
            .collect();
        assert!(
            cell_ids.iter().any(|id| id.contains("fifo-depth-8")),
            "expected fifo-depth-8 in {cell_ids:?}"
        );
        assert!(
            cell_ids.iter().any(|id| id.contains("fifo-depth-32")),
            "expected fifo-depth-32 in {cell_ids:?}"
        );
    }

    #[test]
    fn type2_study_uses_explicit_values_when_provided() {
        let plan = make_plan(
            r#"
schema_version = 1

[plan]
goal = "characterize"
doe_pattern = "oat"
budget_runs = 50

[[workloads]]
name = "w"

[[studies]]
name = "sweep-depth"
kind = "type2"
workload = "w"
parameter = "fifo_depth"
values = [8, 64]
"#,
        );
        let tmp = project_dir();
        let results = run(tmp.path(), &plan, None).expect("run");
        assert_eq!(results.studies[0].cells.len(), 2);
    }

    #[test]
    fn type3_study_produces_cross_product() {
        let plan = make_plan(
            r#"
schema_version = 1

[plan]
goal = "characterize"
doe_pattern = "oat-pair-screen"
budget_runs = 50

[[workloads]]
name = "w"

[[studies]]
name = "pair"
kind = "type3"
workload = "w"
parameters = ["fifo_depth", "clock_ghz"]
"#,
        );
        let variants = make_variants(
            r#"
schema_version = 1

[parameters.fifo_depth]
values = [8, 16]
default = 8

[parameters.clock_ghz]
values = [1.0, 1.5, 2.0]
default = 1.0
"#,
        );
        let tmp = project_dir();
        let results = run(tmp.path(), &plan, Some(&variants)).expect("run");
        assert_eq!(results.studies[0].cells.len(), 2 * 3);
    }

    #[test]
    fn budget_cap_stops_execution() {
        let plan = make_plan(
            r#"
schema_version = 1

[plan]
goal = "characterize"
doe_pattern = "oat"
budget_runs = 2

[[workloads]]
name = "w"

[[studies]]
name = "sweep"
kind = "type2"
workload = "w"
parameter = "fifo_depth"
values = [8, 16, 32, 64]
"#,
        );
        let tmp = project_dir();
        let results = run(tmp.path(), &plan, None).expect("run");
        assert_eq!(results.total_runs, 2);
        assert!(results.budget_reached);
        assert_eq!(results.studies[0].cells.len(), 2);
    }

    #[test]
    fn pareto_study_returns_config_error() {
        let plan = make_plan(
            r#"
schema_version = 1

[plan]
goal = "optimize"
doe_pattern = "bayes"
budget_runs = 10

[[workloads]]
name = "w"

[[studies]]
name = "p"
kind = "pareto"
workload = "w"
objectives = ["throughput", "latency"]
"#,
        );
        let tmp = project_dir();
        let err = run(tmp.path(), &plan, None).expect_err("pareto not implemented");
        assert!(format!("{err}").to_lowercase().contains("pareto"));
    }

    #[test]
    fn type2_without_variants_or_explicit_values_errors() {
        let plan = make_plan(
            r#"
schema_version = 1

[plan]
goal = "characterize"
doe_pattern = "oat"
budget_runs = 10

[[workloads]]
name = "w"

[[studies]]
name = "sweep"
kind = "type2"
workload = "w"
parameter = "fifo_depth"
"#,
        );
        let tmp = project_dir();
        let err = run(tmp.path(), &plan, None).expect_err("must require values or manifest");
        assert!(format!("{err}").contains("variants.toml"));
    }
}
