//! End-to-end smoke test for the Phase 1 orchestrator core.
//!
//! The test wires up the mock client with on-disk fixture directories,
//! registers a single synthetic step against a scratch state, and drives
//! it through `StepRunner::run`. It exercises:
//! - instructions/ loader
//! - mock client fixture copy
//! - work + critique session pair
//! - gate validation (file existence, critique scan)
//! - happy-path gate pass
//! - gate failure on a BLOCKER: line
//! - back-transition reset cascade

use std::path::PathBuf;
use std::sync::Arc;

use sim_flow::client::SessionKind;
use sim_flow::clients::mock::MockClient;
use sim_flow::config::{ClientName, Config};
use sim_flow::gate::GateCheck;
use sim_flow::prompts;
use sim_flow::runner::StepRunner;
use sim_flow::state::{Flow, State};
use sim_flow::steps::{StepDescriptor, StepRegistry};

fn write(path: &std::path::Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn stage_foundation(root: &std::path::Path) {
    let inst = root.join(prompts::PROMPTS_DIR);
    std::fs::create_dir_all(&inst).unwrap();
    write(&inst.join("smoke-step.md"), "work prompt\n");
    write(&inst.join("smoke-step-critique.md"), "critique prompt\n");
}

fn stage_fixtures(fixtures: &std::path::Path, critique_body: &str) {
    let work = fixtures.join("SMOKE.work");
    std::fs::create_dir_all(&work).unwrap();
    write(&work.join("artifact.md"), "work output artifact\n");

    let crit = fixtures.join("SMOKE.critique");
    std::fs::create_dir_all(crit.join("docs/critiques")).unwrap();
    write(
        &crit.join("docs/critiques/SMOKE-critique.md"),
        critique_body,
    );
}

fn smoke_step() -> StepDescriptor {
    StepDescriptor {
        id: "SMOKE",
        flow: Flow::DirectModeling,
        prerequisite: None,
        instruction_slug: "smoke-step",
        per_candidate: false,
        gate_checks: vec![
            GateCheck::FileExists {
                path: PathBuf::from("artifact.md"),
                description: "artifact.md exists".into(),
            },
            GateCheck::CritiqueClean {
                path: PathBuf::from("docs/critiques/SMOKE-critique.md"),
                description: "critique clean".into(),
            },
        ],
        work_artifacts: &["artifact.md"],
        predecessor_inputs: &[],
        work_phases: &["chat"],
        critique_phases: &["chat"],
    }
}

#[test]
fn runs_work_and_critique_and_passes_gate() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("proj");
    let foundation = root.path().join("foundation");
    let fixtures = root.path().join("fixtures");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&foundation).unwrap();

    stage_foundation(&foundation);
    stage_fixtures(&fixtures, "# Critique\n\n- RESOLVED: tidied output\n");

    let mut config = Config::default();
    config.client.name = ClientName::Mock;
    let client =
        Arc::new(MockClient::with_fixtures(fixtures.clone())) as Arc<dyn sim_flow::client::Client>;

    let mut registry = StepRegistry::new();
    registry.register(smoke_step());
    let step = registry.get("SMOKE").unwrap().clone();

    let mut state = State::new(Flow::DirectModeling, "SMOKE");
    let runner = StepRunner::new(&project, &foundation, &registry, &config).with_client(client);
    let outcome = runner.run(&step, &mut state, None).unwrap();
    assert!(
        outcome.gate_report.is_clean(),
        "gate should pass: {:?}",
        outcome.gate_report.failures
    );
    assert!(state.is_passed("SMOKE"));
    assert!(
        project.join("artifact.md").exists(),
        "work session should have produced artifact.md"
    );
    assert!(
        project.join("docs/critiques/SMOKE-critique.md").exists(),
        "critique session should have produced the critique file"
    );
}

#[test]
fn gate_fails_on_blocker_line() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("proj");
    let foundation = root.path().join("foundation");
    let fixtures = root.path().join("fixtures");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&foundation).unwrap();

    stage_foundation(&foundation);
    stage_fixtures(
        &fixtures,
        "# Critique\n\n- BLOCKER: scoreboard does not verify ordering\n",
    );

    let mut config = Config::default();
    config.client.name = ClientName::Mock;
    let client =
        Arc::new(MockClient::with_fixtures(fixtures.clone())) as Arc<dyn sim_flow::client::Client>;

    let mut registry = StepRegistry::new();
    registry.register(smoke_step());
    let step = registry.get("SMOKE").unwrap().clone();

    let mut state = State::new(Flow::DirectModeling, "SMOKE");
    let runner = StepRunner::new(&project, &foundation, &registry, &config).with_client(client);
    let outcome = runner.run(&step, &mut state, None).unwrap();
    assert!(!outcome.gate_report.is_clean());
    assert!(
        !state.is_passed("SMOKE"),
        "gate failed, state must not advance"
    );
}

#[test]
fn prerequisite_enforced() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("proj");
    let foundation = root.path().join("foundation");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&foundation).unwrap();
    stage_foundation(&foundation);

    let mut registry = StepRegistry::new();
    let mut step = smoke_step();
    step.prerequisite = Some("DM0");
    registry.register(step.clone());

    let config = Config::default();
    let mut state = State::new(Flow::DirectModeling, "SMOKE");
    let runner = StepRunner::new(&project, &foundation, &registry, &config);
    let err = runner.run(&step, &mut state, None).unwrap_err();
    assert!(format!("{err}").contains("prerequisite"));
}

#[test]
fn back_transition_cascades() {
    let mut state = State::new(Flow::DirectModeling, "DM0");
    for step in ["DM0", "DM1", "DM2a", "DM2b"] {
        state.mark_passed(step, "t");
    }
    state.reset("DM1", &["DM0", "DM1", "DM2a", "DM2b"]).unwrap();
    assert!(state.is_passed("DM0"));
    assert!(!state.is_passed("DM1"));
    assert!(!state.is_passed("DM2a"));
    assert!(!state.is_passed("DM2b"));
}

#[test]
fn mock_client_direct_invocation_applies_fixtures() {
    let root = tempfile::tempdir().unwrap();
    let fixtures = root.path().join("fx");
    stage_fixtures(&fixtures, "- RESOLVED: ok\n");
    let project = root.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    let client: Box<dyn sim_flow::client::Client> =
        Box::new(MockClient::with_fixtures(fixtures.clone()));
    let invocation = sim_flow::client::Invocation {
        step: "SMOKE".into(),
        kind: SessionKind::Work,
        mode: sim_flow::client::SessionMode::OneShot,
        prompt: "p".into(),
        instructions: "i".into(),
        project_dir: project.clone(),
        candidate: None,
        timeout_seconds: None,
    };
    let session = client.invoke(&invocation).unwrap();
    assert!(session.success());
    assert!(project.join("artifact.md").exists());
}
