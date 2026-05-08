//! DM step registration with detailed gate checks per
//! `docs/architecture/ai-flow/02-direct-modeling-flow.md`.
//!
//! Gate checks are structural (file exists, regex matches, shell command
//! succeeds, critique scan). Semantic cross-file checks (e.g. every
//! operation name from `decomposition.md` appears in `pipeline-mapping.md`)
//! live in the corresponding critique prompt, not here.

use std::path::PathBuf;

use crate::gate::GateCheck;
use crate::state::Flow;
use crate::steps::{MilestoneWalkConfig, StepDescriptor, StepRegistry};

pub fn register(reg: &mut StepRegistry) {
    reg.register(dm0());
    reg.register(dm1());
    reg.register(dm2a());
    reg.register(dm2b());
    reg.register(dm2c());
    reg.register(dm2cd());
    reg.register(dm2d());
    reg.register(dm3a());
    reg.register(dm3ad());
    reg.register(dm3b());
    reg.register(dm3c());
    reg.register(dm4a());
    reg.register(dm4ad());
    reg.register(dm4b());
}

fn critique_clean(step: &str) -> GateCheck {
    GateCheck::CritiqueClean {
        path: PathBuf::from(format!("docs/critiques/{step}-critique.md")),
        description: format!("{step} critique has no blockers"),
    }
}

fn file_exists(path: &str, description: &str) -> GateCheck {
    GateCheck::FileExists {
        path: PathBuf::from(path),
        description: description.to_string(),
    }
}

fn file_matches(path: &str, pattern: &str, description: &str) -> GateCheck {
    GateCheck::FileMatches {
        path: PathBuf::from(path),
        pattern: pattern.to_string(),
        description: description.to_string(),
    }
}

fn shell(cmd: &str, args: &[&str], description: &str) -> GateCheck {
    GateCheck::Shell {
        cmd: cmd.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        description: description.to_string(),
    }
}

fn any_exists(paths: &[&str], description: &str) -> GateCheck {
    GateCheck::AnyExists {
        paths: paths.iter().map(PathBuf::from).collect(),
        description: description.to_string(),
    }
}

fn any_matches(paths: &[&str], pattern: &str, description: &str) -> GateCheck {
    GateCheck::AnyMatches {
        paths: paths.iter().map(PathBuf::from).collect(),
        pattern: pattern.to_string(),
        description: description.to_string(),
    }
}

/// Pair this with a `StepDescriptor::milestone_walk` so the step's
/// gate cannot pass while any milestone file under `dir` is still
/// pending. Defaults to execution-step semantics (`- [ ]` rows must
/// resolve); use `milestones_all_detailed` for planning-detail
/// steps where the placeholder-marker mode applies.
fn milestones_all_resolved(dir: &str, file_prefix: &str, description: &str) -> GateCheck {
    GateCheck::MilestonesAllResolved {
        dir: PathBuf::from(dir),
        file_prefixes: vec![file_prefix.to_string()],
        placeholder_marker: None,
        description: description.to_string(),
    }
}

/// Planning-detail variant of `milestones_all_resolved`: gate is
/// clean iff no milestone file under `dir` (matching any prefix in
/// `file_prefixes`) still contains `placeholder_marker` in its body.
/// The detail step replaces stub bodies with full task lists; the
/// outline step's `- [ ]` task rows are intentionally left pending
/// (they're for the downstream execution step), so the row-count
/// gate would never advance here.
fn milestones_all_detailed(
    dir: &str,
    file_prefixes: &[&str],
    placeholder_marker: &str,
    description: &str,
) -> GateCheck {
    GateCheck::MilestonesAllResolved {
        dir: PathBuf::from(dir),
        file_prefixes: file_prefixes.iter().map(|s| (*s).to_string()).collect(),
        placeholder_marker: Some(placeholder_marker.to_string()),
        description: description.to_string(),
    }
}

fn dm0() -> StepDescriptor {
    // The spec can land in either of two layouts and the gate
    // accepts both:
    //   - Single file: `docs/spec.md` (small designs).
    //   - Paginated:  `docs/spec/<NN>-<slug>.md` section files
    //     (large designs that exceed an LLM's single-response
    //     budget; mirrors the input-spec staging convention).
    // The `any_exists` / `any_matches` helpers expand the directory
    // entry to all `*.md` files inside (excluding scaffolding and
    // index files like `README.md` / `_toc.md`).
    StepDescriptor {
        id: "DM0",
        flow: Flow::DirectModeling,
        prerequisite: None,
        instruction_slug: "dm0-specification",
        per_candidate: false,
        gate_checks: vec![
            any_exists(
                &["docs/spec.md", "docs/spec/"],
                "docs/spec.md or docs/spec/ exists and is non-empty",
            ),
            any_matches(
                &["docs/spec.md", "docs/spec/"],
                r"\d+\s*(MHz|GHz)",
                "spec declares a clock frequency",
            ),
            any_matches(
                &["docs/spec.md", "docs/spec/"],
                r"\d+\s*nm",
                "spec declares a technology node",
            ),
            critique_clean("DM0"),
        ],
        // Both layouts listed so a Reset to DM0 (or any downstream
        // reset that cascades through DM0) clears either form.
        work_artifacts: &["docs/spec.md", "docs/spec/"],
        predecessor_inputs: &[],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

fn dm1() -> StepDescriptor {
    StepDescriptor {
        id: "DM1",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM0"),
        instruction_slug: "dm1-modeling-setup",
        per_candidate: false,
        gate_checks: vec![
            file_exists("docs/targets.md", "docs/targets.md exists"),
            file_matches(
                "docs/targets.md",
                r"(?i)\d+\s*(cycles?|ns|MHz|GHz|items|bits|gates)",
                "docs/targets.md contains at least one quantitative target",
            ),
            file_exists("docs/testbench.md", "docs/testbench.md exists"),
            file_matches(
                "docs/testbench.md",
                r"(Sequencer|Driver|Monitor|Scoreboard)",
                "docs/testbench.md names at least one UVM-lite component",
            ),
            file_matches(
                "docs/testbench.md",
                r"lib:examples/\d{2}-[a-z0-9-]+/test/?",
                "docs/testbench.md names a concrete lib:examples/<NN-name>/test/ baseline DM3b will mirror",
            ),
            critique_clean("DM1"),
        ],
        work_artifacts: &["docs/targets.md", "docs/testbench.md"],
        predecessor_inputs: &["docs/spec.md", "docs/spec/"],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

fn dm2a() -> StepDescriptor {
    StepDescriptor {
        id: "DM2a",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM1"),
        instruction_slug: "dm2a-decomposition",
        per_candidate: false,
        gate_checks: vec![
            file_exists(
                "docs/analysis/decomposition.md",
                "docs/analysis/decomposition.md exists",
            ),
            file_matches(
                "docs/analysis/decomposition.md",
                r"(?m)^##\s*Operation:\s*\S+",
                "docs/analysis/decomposition.md declares at least one ## Operation: <name> heading",
            ),
            file_exists(
                "docs/analysis/data-movement.md",
                "docs/analysis/data-movement.md exists",
            ),
            critique_clean("DM2a"),
        ],
        work_artifacts: &[
            "docs/analysis/decomposition.md",
            "docs/analysis/data-movement.md",
        ],
        predecessor_inputs: &["docs/spec.md", "docs/spec/", "docs/targets.md"],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

fn dm2b() -> StepDescriptor {
    StepDescriptor {
        id: "DM2b",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM2a"),
        instruction_slug: "dm2b-pipeline-mapping",
        per_candidate: false,
        gate_checks: vec![
            file_exists(
                "docs/analysis/pipeline-mapping.md",
                "docs/analysis/pipeline-mapping.md exists",
            ),
            file_matches(
                "docs/analysis/pipeline-mapping.md",
                r"(?i)stage",
                "docs/analysis/pipeline-mapping.md mentions pipeline stages",
            ),
            critique_clean("DM2b"),
        ],
        work_artifacts: &["docs/analysis/pipeline-mapping.md"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/spec/",
            "docs/analysis/decomposition.md",
            "docs/analysis/data-movement.md",
        ],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

/// DM2c (Implementation Plan, OUTLINE) — produces the plan index
/// at `docs/impl-plan/plan.md` plus a milestone-NN-*.md STUB per
/// milestone. Each stub carries its scope, predecessor / dependency
/// notes, and a `<!-- detail-pending -->` placeholder that DM2cd
/// later replaces with the full task list. Splitting outline from
/// detail bounds each session's context: a large design (e.g.
/// RISC-V core with 25+ milestones) can name its milestones in one
/// session even when the per-milestone task lists are too large to
/// fit alongside the spec + decomposition + targets in one prompt.
fn dm2c() -> StepDescriptor {
    StepDescriptor {
        id: "DM2c",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM2b"),
        instruction_slug: "dm2c-model-impl-plan",
        per_candidate: false,
        gate_checks: vec![
            file_exists("docs/impl-plan/plan.md", "docs/impl-plan/plan.md exists"),
            file_matches(
                "docs/impl-plan/plan.md",
                r"(?i)milestone\s+\d+",
                "docs/impl-plan/plan.md references at least one numbered milestone",
            ),
            shell(
                "sh",
                &["-c", "ls docs/impl-plan/milestone-*.md >/dev/null 2>&1"],
                "docs/impl-plan/ contains at least one milestone-NN-*.md stub",
            ),
            critique_clean("DM2c"),
        ],
        work_artifacts: &["docs/impl-plan/"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/spec/",
            "docs/analysis/decomposition.md",
            "docs/analysis/data-movement.md",
            "docs/analysis/pipeline-mapping.md",
        ],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

/// DM2cd (Implementation Plan, DETAIL) — walks each
/// `docs/impl-plan/milestone-NN-*.md` stub and replaces the
/// `<!-- detail-pending -->` placeholder with the full task list per
/// `docs/impl-plan/plan-management.md`'s task format. One milestone
/// per work + critique session, so the per-milestone task list gets
/// a focused review and a critique can flag e.g. "milestone 03
/// task list is too coarse" without blocking the rest of the plan.
fn dm2cd() -> StepDescriptor {
    StepDescriptor {
        id: "DM2cd",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM2c"),
        instruction_slug: "dm2cd-impl-plan-detail",
        per_candidate: false,
        gate_checks: vec![
            milestones_all_detailed(
                "docs/impl-plan/",
                &["milestone-"],
                "<!-- detail-pending -->",
                "every docs/impl-plan/milestone-NN-*.md stub has been detailed",
            ),
            critique_clean("DM2cd"),
        ],
        work_artifacts: &["docs/impl-plan/"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/spec/",
            "docs/analysis/decomposition.md",
            "docs/analysis/data-movement.md",
            "docs/analysis/pipeline-mapping.md",
            "docs/impl-plan/plan.md",
            "docs/impl-plan/plan-management.md",
        ],
        work_write_paths: &["docs/impl-plan/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "docs/impl-plan/",
            file_prefixes: &["milestone-"],
            index_file: "docs/impl-plan/plan.md",
            placeholder_marker: Some("<!-- detail-pending -->"),
        }),
    }
}

/// DM2d (Model Implementation) — executes the DM2c plan to land
/// `src/model/`, tests, a working `cargo build` + `cargo test`. The
/// gates here are the heavyweight code checks that used to live on
/// DM2c (Cargo, build, elaboration smoke test, ConnectivityPlan and
/// HasLogic grep) so the split is purely an internal partition --
/// downstream steps still rely on the same artifacts being present
/// once DM2d's gate passes.
fn dm2d() -> StepDescriptor {
    StepDescriptor {
        id: "DM2d",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM2cd"),
        instruction_slug: "dm2d-model-implementation",
        per_candidate: false,
        gate_checks: vec![
            file_exists("Cargo.toml", "Cargo.toml exists"),
            file_matches(
                "Cargo.toml",
                "foundation-framework",
                "Cargo.toml depends on foundation-framework",
            ),
            shell(
                "cargo",
                &["fmt", "--all", "--", "--check"],
                "cargo fmt --check passes (no unformatted code)",
            ),
            shell(
                "cargo",
                &["clippy", "--all-targets", "--quiet", "--", "-D", "warnings"],
                "cargo clippy --all-targets clean (warnings denied)",
            ),
            shell("cargo", &["build", "--quiet"], "cargo build succeeds"),
            shell(
                "cargo",
                &["test", "--quiet", "--test", "elaboration"],
                "elaboration smoke test passes",
            ),
            shell(
                "grep",
                &["-r", "--include=*.rs", "-q", "ConnectivityPlan", "src"],
                "src/ references ConnectivityPlan",
            ),
            shell(
                "grep",
                &["-r", "--include=*.rs", "-q", "impl HasLogic", "src"],
                "src/ implements HasLogic",
            ),
            // Block-diagram contract. The orchestrator auto-renders
            // the block diagram on the DM2d -> DM3a advance via
            // `crate::dump_topology(&args)`. The two greps below
            // pin the contract DM2d must keep intact: the dump
            // helper in lib.rs and the stub `Top` (or its
            // post-DM2d replacement) under model/.
            file_matches(
                "src/lib.rs",
                r"pub\s+fn\s+dump_topology",
                "src/lib.rs exports dump_topology(&TopologyDumpArgs) (template contract)",
            ),
            shell(
                "sh",
                &[
                    "-c",
                    "grep -q 'pub struct Top' src/model/top.rs && grep -q 'impl Module for Top' src/model/top.rs",
                ],
                "src/model/top.rs defines `pub struct Top` with `impl Module for Top` (block-diagram contract)",
            ),
            milestones_all_resolved(
                "docs/impl-plan/",
                "milestone-",
                "every docs/impl-plan/milestone-NN-*.md row resolved",
            ),
            critique_clean("DM2d"),
        ],
        work_artifacts: &["src/", "tests/", "Cargo.toml"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/spec/",
            "docs/analysis/decomposition.md",
            "docs/analysis/data-movement.md",
            "docs/analysis/pipeline-mapping.md",
            "docs/impl-plan/plan.md",
        ],
        // `docs/impl-plan/` included so the agent can tick `[ ]` -> `[x]`
        // on completed plan tasks. The DM2d critique gates already
        // catch out-of-scope plan rewrites (gate 11: "introduced
        // major architectural structures or boundaries not reflected
        // in DM2c's plan"), so the safety net for misuse exists.
        work_write_paths: &["src/", "tests/", "Cargo.toml", "docs/impl-plan/"],
        work_phases: &["author", "build", "test"],
        critique_phases: &["chat"],
        // DM2d walks `docs/impl-plan/milestone-NN-*.md` files one
        // at a time. The orchestrator picks the current milestone
        // (first file with `- [ ]` rows) and scopes both work +
        // critique sessions to it; the auto-driver iterates
        // work-then-critique until every milestone is resolved.
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "docs/impl-plan/",
            file_prefixes: &["milestone-"],
            index_file: "docs/impl-plan/plan.md",
            placeholder_marker: None,
        }),
    }
}

/// DM3a (Test Plan) — produces the formal verification plan as a
/// directory at `docs/test-plan/`. Top-level `test-plan.md` is the
/// index (testbench architecture + traceability table back to spec
/// / targets / decomposition); two parallel milestone sequences
/// (`tb-milestone-NN-*.md` for DM3b's testbench-impl slices and
/// `test-milestone-NN-*.md` for DM3c's test-execution slices); and
/// `coverage.md` for the `cargo-tarpaulin` strategy. No test code
/// is written here; that's DM3b (scaffolding) and DM3c (test
/// cases). The milestone structure mirrors `docs/impl-plan/` and
/// `docs/perf-plan/` so DM3b/DM3c walk small reviewable chunks
/// with a critique after each milestone (10-task cap per
/// `plan-management.md`).
fn dm3a() -> StepDescriptor {
    StepDescriptor {
        id: "DM3a",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM2d"),
        instruction_slug: "dm3a-test-plan",
        per_candidate: false,
        gate_checks: vec![
            file_exists(
                "docs/test-plan/test-plan.md",
                "docs/test-plan/test-plan.md exists (index file)",
            ),
            file_exists(
                "docs/test-plan/coverage.md",
                "docs/test-plan/coverage.md exists",
            ),
            shell(
                "sh",
                &["-c", "ls docs/test-plan/tb-milestone-*.md >/dev/null 2>&1"],
                "docs/test-plan/ contains at least one tb-milestone-NN-*.md stub (DM3b's slices)",
            ),
            shell(
                "sh",
                &[
                    "-c",
                    "ls docs/test-plan/test-milestone-*.md >/dev/null 2>&1",
                ],
                "docs/test-plan/ contains at least one test-milestone-NN-*.md stub (DM3c's slices)",
            ),
            file_matches(
                "docs/test-plan/test-plan.md",
                r"(?i)Sequencer|Driver|Monitor|Scoreboard",
                "test-plan.md index describes UVM-lite testbench components",
            ),
            file_matches(
                "docs/test-plan/test-plan.md",
                r"spec\.md|targets\.md",
                "test-plan.md index traces entries back to docs/spec.md or docs/targets.md",
            ),
            file_matches(
                "docs/test-plan/coverage.md",
                r"(?i)tarpaulin",
                "docs/test-plan/coverage.md describes coverage via cargo-tarpaulin",
            ),
            critique_clean("DM3a"),
        ],
        work_artifacts: &["docs/test-plan/"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/targets.md",
            "docs/testbench.md",
            "docs/analysis/decomposition.md",
            "docs/analysis/pipeline-mapping.md",
            "docs/analysis/data-movement.md",
            "docs/impl-plan/plan.md",
            "src/",
        ],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

/// DM3ad (Test Plan, DETAIL) — walks each tb-milestone-NN-*.md and
/// test-milestone-NN-*.md stub left by DM3a and replaces the
/// `<!-- detail-pending -->` placeholder with the full task list.
/// Both file types live in `docs/test-plan/`; the milestone-walk
/// machinery walks lexicographically across both prefixes so a
/// large project's tb-* tasks land before the test-* tasks that
/// depend on them. One milestone per session = focused critiques.
fn dm3ad() -> StepDescriptor {
    StepDescriptor {
        id: "DM3ad",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM3a"),
        instruction_slug: "dm3ad-test-plan-detail",
        per_candidate: false,
        gate_checks: vec![
            milestones_all_detailed(
                "docs/test-plan/",
                &["tb-milestone-", "test-milestone-"],
                "<!-- detail-pending -->",
                "every tb-milestone-NN-*.md and test-milestone-NN-*.md stub has been detailed",
            ),
            shell(
                "sh",
                &[
                    "-c",
                    "grep -lE '^-[[:space:]]+\\[[ x]\\][[:space:]]' docs/test-plan/tb-milestone-*.md docs/test-plan/test-milestone-*.md >/dev/null 2>&1",
                ],
                "docs/test-plan/ milestone files contain markdown checklist entries",
            ),
            critique_clean("DM3ad"),
        ],
        work_artifacts: &["docs/test-plan/"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/targets.md",
            "docs/testbench.md",
            "docs/analysis/decomposition.md",
            "docs/analysis/data-movement.md",
            "docs/analysis/pipeline-mapping.md",
            "docs/test-plan/test-plan.md",
            "docs/test-plan/coverage.md",
            "docs/impl-plan/plan-management.md",
        ],
        work_write_paths: &["docs/test-plan/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "docs/test-plan/",
            file_prefixes: &["tb-milestone-", "test-milestone-"],
            index_file: "docs/test-plan/test-plan.md",
            placeholder_marker: Some("<!-- detail-pending -->"),
        }),
    }
}

/// DM3b (Testbench Implementation) — implements the UVM-lite
/// testbench scaffolding (Sequencers, Drivers, Monitors,
/// Scoreboards, `SimEnvBuilder` wiring) named in the test plan,
/// plus the basic data-flow smoke test. Edge / stress / random
/// tests are DM3c's responsibility.
fn dm3b() -> StepDescriptor {
    StepDescriptor {
        id: "DM3b",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM3ad"),
        instruction_slug: "dm3b-testbench-impl",
        per_candidate: false,
        gate_checks: vec![
            shell(
                "grep",
                &[
                    "-r",
                    "--include=*.rs",
                    "-qE",
                    "SimEnv|Sequencer|Driver|Monitor|Scoreboard",
                    "tests",
                ],
                "tests/ references UVM-lite testbench components",
            ),
            shell(
                "cargo",
                &["fmt", "--all", "--", "--check"],
                "cargo fmt --check passes (no unformatted code)",
            ),
            shell(
                "cargo",
                &["clippy", "--all-targets", "--quiet", "--", "-D", "warnings"],
                "cargo clippy --all-targets clean (warnings denied)",
            ),
            shell("cargo", &["build", "--quiet"], "cargo build succeeds"),
            milestones_all_resolved(
                "docs/test-plan/",
                "tb-milestone-",
                "every docs/test-plan/tb-milestone-NN-*.md row resolved",
            ),
            critique_clean("DM3b"),
        ],
        work_artifacts: &["tests/"],
        predecessor_inputs: &["docs/testbench.md", "docs/test-plan/", "src/"],
        // `docs/test-plan/` included so the agent can flip
        // checkboxes on `tb-milestone-NN-*.md` files as tasks
        // complete. The DM3b critique catches out-of-scope plan
        // rewrites (silent restructuring of milestone files).
        work_write_paths: &["tests/", "src/", "docs/test-plan/"],
        work_phases: &["author", "build"],
        critique_phases: &["chat"],
        // DM3b walks `docs/test-plan/tb-milestone-NN-*.md` files
        // one at a time. See StepDescriptor::milestone_walk for
        // the structural-enforcement rationale.
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "docs/test-plan/",
            file_prefixes: &["tb-milestone-"],
            index_file: "docs/test-plan/test-plan.md",
            placeholder_marker: None,
        }),
    }
}

/// DM3c (Test Execution and Coverage) — implements every test in
/// the plan's smoke / edge / stress / random categories using
/// DM3b's testbench, runs the full suite, then runs
/// `cargo-tarpaulin` to verify the coverage threshold the plan
/// declared.
fn dm3c() -> StepDescriptor {
    StepDescriptor {
        id: "DM3c",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM3b"),
        instruction_slug: "dm3c-test-execution",
        per_candidate: false,
        gate_checks: vec![
            shell(
                "cargo",
                &["fmt", "--all", "--", "--check"],
                "cargo fmt --check passes (no unformatted code)",
            ),
            shell(
                "cargo",
                &["clippy", "--all-targets", "--quiet", "--", "-D", "warnings"],
                "cargo clippy --all-targets clean (warnings denied)",
            ),
            shell(
                "cargo",
                &["test", "--quiet"],
                "full cargo test suite passes",
            ),
            file_exists(
                "docs/test-plan/test-plan.md",
                "docs/test-plan/test-plan.md still present",
            ),
            // Coverage threshold is validated by the critique (the
            // AI inspects the cargo-tarpaulin report referenced from
            // the plan and flags sub-threshold runs). A structural
            // gate that runs tarpaulin from this process would
            // double the test time; rely on the agent reporting the
            // measured percentage in the plan's `## Coverage`
            // section.
            milestones_all_resolved(
                "docs/test-plan/",
                "test-milestone-",
                "every docs/test-plan/test-milestone-NN-*.md row resolved",
            ),
            critique_clean("DM3c"),
        ],
        work_artifacts: &["tests/"],
        predecessor_inputs: &["docs/testbench.md", "docs/test-plan/", "tests/", "src/"],
        // `docs/test-plan/` included so the agent can flip
        // checkboxes on `test-milestone-NN-*.md` rows and append
        // measured-coverage info to `test-plan.md`'s `## Coverage`
        // section. The DM3c critique catches structural plan
        // rewrites.
        work_write_paths: &["tests/", "src/", "docs/test-plan/"],
        work_phases: &["author", "test"],
        critique_phases: &["chat"],
        // DM3c walks `docs/test-plan/test-milestone-NN-*.md` files
        // one at a time. See StepDescriptor::milestone_walk for
        // the structural-enforcement rationale.
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "docs/test-plan/",
            file_prefixes: &["test-milestone-"],
            index_file: "docs/test-plan/test-plan.md",
            placeholder_marker: None,
        }),
    }
}

/// DM4a (Performance Analysis Plan, OUTLINE) — produces the index
/// at `docs/perf-plan/perf-plan.md` plus stub
/// `perf-milestone-NN-*.md` files. Each stub names its workload
/// scope and traceability hooks; DM4ad fills in the per-milestone
/// task list. No simulations or report writing happen here.
fn dm4a() -> StepDescriptor {
    StepDescriptor {
        id: "DM4a",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM3c"),
        instruction_slug: "dm4a-performance-plan",
        per_candidate: false,
        gate_checks: vec![
            file_exists(
                "docs/perf-plan/perf-plan.md",
                "docs/perf-plan/perf-plan.md exists",
            ),
            file_matches(
                "docs/perf-plan/perf-plan.md",
                r"(?i)milestone\s+\d+",
                "docs/perf-plan/perf-plan.md references at least one numbered milestone",
            ),
            shell(
                "sh",
                &[
                    "-c",
                    "ls docs/perf-plan/perf-milestone-*.md >/dev/null 2>&1",
                ],
                "docs/perf-plan/ contains at least one perf-milestone-NN-*.md stub",
            ),
            critique_clean("DM4a"),
        ],
        work_artifacts: &["docs/perf-plan/"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/targets.md",
            "docs/analysis/decomposition.md",
            "docs/analysis/pipeline-mapping.md",
            "docs/test-plan/",
        ],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: None,
    }
}

/// DM4ad (Performance Analysis Plan, DETAIL) — walks each
/// `docs/perf-plan/perf-milestone-NN-*.md` stub and replaces its
/// `<!-- detail-pending -->` placeholder with the full task list per
/// `docs/impl-plan/plan-management.md`. Tasks reference workload
/// configs, target rows, and run-id schemes that DM4b later
/// executes.
fn dm4ad() -> StepDescriptor {
    StepDescriptor {
        id: "DM4ad",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM4a"),
        instruction_slug: "dm4ad-performance-plan-detail",
        per_candidate: false,
        gate_checks: vec![
            milestones_all_detailed(
                "docs/perf-plan/",
                &["perf-milestone-"],
                "<!-- detail-pending -->",
                "every docs/perf-plan/perf-milestone-NN-*.md stub has been detailed",
            ),
            critique_clean("DM4ad"),
        ],
        work_artifacts: &["docs/perf-plan/"],
        predecessor_inputs: &[
            "docs/spec.md",
            "docs/targets.md",
            "docs/analysis/decomposition.md",
            "docs/analysis/pipeline-mapping.md",
            "docs/perf-plan/perf-plan.md",
            "docs/impl-plan/plan-management.md",
        ],
        work_write_paths: &["docs/perf-plan/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "docs/perf-plan/",
            file_prefixes: &["perf-milestone-"],
            index_file: "docs/perf-plan/perf-plan.md",
            placeholder_marker: Some("<!-- detail-pending -->"),
        }),
    }
}

/// DM4b (Performance Analysis) — executes the DM4a plan: runs
/// experiments + sweeps, identifies bottlenecks, verifies targets,
/// and writes per-topic reports under `docs/analysis/`. Depends on
/// experiment tracking (Phase 4); the critique surfaces a BLOCKER
/// if `.sim-flow/experiments.db` is unreachable.
fn dm4b() -> StepDescriptor {
    StepDescriptor {
        id: "DM4b",
        flow: Flow::DirectModeling,
        prerequisite: Some("DM4ad"),
        instruction_slug: "dm4b-performance-analysis",
        per_candidate: false,
        gate_checks: vec![
            GateCheck::ExperimentsRecorded {
                description: "experiments.db has at least one recorded run".to_string(),
            },
            shell(
                "sh",
                &["-c", "ls docs/analysis/*.md >/dev/null 2>&1"],
                "docs/analysis/ contains at least one report",
            ),
            shell(
                "sh",
                &["-c", "grep -rqiE 'throughput|latency' docs/analysis/"],
                "analysis report covers throughput and latency metrics",
            ),
            file_exists(
                "docs/perf-plan/perf-plan.md",
                "docs/perf-plan/perf-plan.md still present",
            ),
            // DM4b mostly writes markdown reports, but any Rust
            // helpers / sweep glue / scratch binaries it lands
            // must still meet the same fmt + clippy bar as
            // earlier code-generating steps. `cargo build` is
            // already implied by `cargo clippy` so we skip a
            // separate build check here.
            shell(
                "cargo",
                &["fmt", "--all", "--", "--check"],
                "cargo fmt --check passes (no unformatted code)",
            ),
            shell(
                "cargo",
                &["clippy", "--all-targets", "--quiet", "--", "-D", "warnings"],
                "cargo clippy --all-targets clean (warnings denied)",
            ),
            milestones_all_resolved(
                "docs/perf-plan/",
                "perf-milestone-",
                "every docs/perf-plan/perf-milestone-NN-*.md row resolved",
            ),
            critique_clean("DM4b"),
        ],
        work_artifacts: &["docs/analysis/"],
        predecessor_inputs: &[
            "docs/targets.md",
            "docs/perf-plan/perf-plan.md",
            ".sim-flow/experiments.db",
        ],
        work_write_paths: &["docs/"],
        work_phases: &["chat"],
        critique_phases: &["chat"],
        // DM4b walks `docs/perf-plan/perf-milestone-NN-*.md` files
        // one at a time. See StepDescriptor::milestone_walk for
        // the structural-enforcement rationale.
        milestone_walk: Some(MilestoneWalkConfig {
            dir: "docs/perf-plan/",
            file_prefixes: &["perf-milestone-"],
            index_file: "docs/perf-plan/perf-plan.md",
            placeholder_marker: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_every_dm_step_in_order() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        let order = reg.order_for(Flow::DirectModeling);
        assert_eq!(
            order,
            vec![
                "DM0", "DM1", "DM2a", "DM2b", "DM2c", "DM2cd", "DM2d", "DM3a", "DM3ad", "DM3b",
                "DM3c", "DM4a", "DM4ad", "DM4b",
            ]
        );
    }

    #[test]
    fn every_dm_step_has_a_critique_check() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        for step in reg.steps() {
            assert!(
                step.gate_checks
                    .iter()
                    .any(|c| matches!(c, GateCheck::CritiqueClean { .. })),
                "{} is missing a critique clean check",
                step.id
            );
        }
    }

    #[test]
    fn prerequisites_chain_as_expected() {
        let mut reg = StepRegistry::new();
        register(&mut reg);
        let pairs: Vec<_> = reg.steps().iter().map(|s| (s.id, s.prerequisite)).collect();
        assert_eq!(
            pairs,
            vec![
                ("DM0", None),
                ("DM1", Some("DM0")),
                ("DM2a", Some("DM1")),
                ("DM2b", Some("DM2a")),
                ("DM2c", Some("DM2b")),
                ("DM2cd", Some("DM2c")),
                ("DM2d", Some("DM2cd")),
                ("DM3a", Some("DM2d")),
                ("DM3ad", Some("DM3a")),
                ("DM3b", Some("DM3ad")),
                ("DM3c", Some("DM3b")),
                ("DM4a", Some("DM3c")),
                ("DM4ad", Some("DM4a")),
                ("DM4b", Some("DM4ad")),
            ]
        );
    }
}
