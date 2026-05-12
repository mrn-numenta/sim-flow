//! Mocked end-to-end flow/transition tests.
//!
//! These are the mocked-LLM siblings of the `e2e_auto` / `e2e_manual`
//! binaries: instead of driving a real backend (claude / openai-compat
//! / ollama / vLLM / LM Studio), they wire a [`MockAgent`] with
//! pre-scripted responses and assert the orchestrator + its
//! transitions behave correctly. The full backend binaries stay the
//! source of truth for "does the model actually produce the right
//! artifact"; these tests pin down the surrounding mechanics
//! (handshake, work -> critique -> advance, multi-step gate
//! transitions, manual-mode dispatch surface) so a regression in the
//! orchestrator or the VS Code-facing protocol is caught in CI
//! without burning API credits or local-LLM cycles.
//!
//! Scope:
//! - `e2e_auto_walks_docs_only_dm_flow`: full auto-mode walk DM0 -> DM1 ->
//!   DM2a -> DM2b against a MockAgent + TerminalHost. Exercises the
//!   work-session structural-gate wind-down, the critique-session
//!   `RequestUserInput` -> `/end-session` wind-down, the per-step
//!   `try_advance_classified` gate, and chained step transitions.
//! - `e2e_manual_drives_docs_only_dm_flow`: manual-mode walk over the
//!   same docs-only DM steps. Drives the orchestrator via explicit
//!   `RunStep` / `Advance` host commands on a `TestHost`, asserting
//!   that each command bracket emits `SubSessionStarted` /
//!   `SubSessionEnded` and that `StateAdvanced` lands the state at
//!   the next step.
//!
//! Coverage:
//! - `e2e_auto_walks_docs_only_dm_flow` / `e2e_manual_drives_docs_only_dm_flow`
//!   exercise the docs-only prefix DM0 -> DM2cd. This includes one
//!   milestone-walk step (DM2cd, placeholder mode) so milestone
//!   transitions are pinned WITHOUT cargo invocations. Fast enough
//!   for CI; runs in seconds.
//! - `e2e_auto_walks_full_dm_flow` is the all-14-steps walk
//!   (DM0 -> DM4b, including DM2d / DM3b / DM3c / DM4b which gate on
//!   cargo fmt / clippy / build / test). Marked `#[ignore]` because
//!   the cargo runs put it over the CI budget; run on demand with
//!   `cargo test -p sim-flow --test e2e_mocked -- --ignored
//!   e2e_auto_walks_full_dm_flow`. The full backend binaries
//!   (`e2e_auto`, `e2e_manual`) remain the source of truth for
//!   "does the model actually produce the right artifact".

use std::io::Cursor;
use std::path::{Path, PathBuf};

use sim_flow::new_project::{NewModelOptions, new_model};
use sim_flow::session::protocol::{
    DiagnosticLevel, Event, HostEvent, HostInfo, PROTOCOL_VERSION, SessionKindOut, StepMode,
};
use sim_flow::session::{AutoOptions, MockAgent, TerminalHost, TestHost, run_auto};
use sim_flow::state::{Flow, State};
use sim_flow::tracking::{ExperimentIndex, RunRow};

// -------------------------------------------------------------------
// Shared fixtures.
// -------------------------------------------------------------------

fn foundation_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn init_project(tmp: &tempfile::TempDir) -> PathBuf {
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(project.join("docs/critiques")).unwrap();
    let state = State::new(Flow::DirectModeling, "DM0");
    state.save(&project.join(".sim-flow")).unwrap();
    let config = sim_flow::config::Config::default();
    config.save(&project.join(".sim-flow")).unwrap();
    project
}

fn auto_opts(project: &Path, mode: StepMode) -> AutoOptions {
    AutoOptions {
        project_dir: project.to_path_buf(),
        foundation_root: foundation_root(),
        llm_backend: "mock".into(),
        llm_model: None,
        llm_model_family_id: None,
        llm_runtime_profile_id: None,
        llm_debug_adaptation: false,
        llm_base_url: None,
        max_auto_iters: 4,
        max_critique_iters: 2,
        // The mocked walk pre-writes clean critiques; the
        // no-progress cap isn't exercised here. Disable it so
        // we only test the absolute-cap path.
        max_critique_no_progress_iters: 0,
        dm0_interactive: false,
        max_llm_requests: 50,
        // Cancel the loop-guard substring matcher: identical mock
        // responses across the four docs-only steps would otherwise
        // trip it. 0 disables the check (it requires >= 2).
        max_identical_responses: 0,
        step_mode: mode,
        no_preamble: true,
    }
}

// Per-step (work, critique) response pairs. Each work response writes
// the step's structural-gate-satisfying artifacts; each critique
// response writes a blocker-free critique. The work-session
// allowlist is `docs/` for DM0..DM2b and the critique-session
// allowlist is the canonical `docs/critiques/<step>-critique.{md,json}`.
fn dm0_work_response() -> String {
    "Drafting the spec.\n\n\
     ```docs/spec.md\n\
     # Spec\n\nClock: 2 GHz\nGates per cycle: 50\nNode: 7 nm\n\
     ```\n"
        .into()
}

fn dm0_critique_response() -> String {
    "Critique done.\n\n\
     ```docs/critiques/DM0-critique.md\n\
     # DM0 Critique\n\nNo blockers; clock and node both declared.\n\
     ```\n"
        .into()
}

fn dm1_work_response() -> String {
    "Drafting targets + testbench.\n\n\
     ```docs/targets.md\n\
     # Targets\n\nThroughput: 100 cycles per item.\n\
     ```\n\n\
     ```docs/testbench.md\n\
     # Testbench\n\n\
     Components: Sequencer, Driver, Monitor, Scoreboard.\n\
     Baseline: lib:examples/00-simple-pipeline/test/\n\
     ```\n"
        .into()
}

fn dm1_critique_response() -> String {
    "Critique done.\n\n\
     ```docs/critiques/DM1-critique.md\n\
     # DM1 Critique\n\nNo blockers.\n\
     ```\n"
        .into()
}

fn dm2a_work_response() -> String {
    "Drafting decomposition + data movement.\n\n\
     ```docs/analysis/decomposition.md\n\
     # Decomposition\n\n## Operation: combine\n\n\
     The combine operation merges three input streams.\n\
     ```\n\n\
     ```docs/analysis/data-movement.md\n\
     # Data Movement\n\n\
     One-pass streaming with no back-pressure.\n\
     ```\n"
        .into()
}

fn dm2a_critique_response() -> String {
    "Critique done.\n\n\
     ```docs/critiques/DM2a-critique.md\n\
     # DM2a Critique\n\nNo blockers.\n\
     ```\n"
        .into()
}

fn dm2b_work_response() -> String {
    "Drafting pipeline mapping.\n\n\
     ```docs/analysis/pipeline-mapping.md\n\
     # Pipeline Mapping\n\n\
     Stage 0: ingest.\nStage 1: combine.\nStage 2: emit.\n\
     ```\n"
        .into()
}

fn dm2b_critique_response() -> String {
    "Critique done.\n\n\
     ```docs/critiques/DM2b-critique.md\n\
     # DM2b Critique\n\nNo blockers.\n\
     ```\n"
        .into()
}

// DM2c is the outline step: writes the impl-plan index + per-
// milestone stubs carrying the `<!-- detail-pending` placeholder
// that DM2cd later replaces with the full task list. The mock plan
// declares exactly one milestone so DM2cd's milestone walk has one
// cycle of work + critique (which is what we want to exercise).
fn dm2c_work_response() -> String {
    "Outlining the impl plan.\n\n\
     ```docs/impl-plan/plan.md\n\
     # Implementation Plan\n\n\
     ## Milestone 1 -- Top setup\n\n\
     Scaffold the top module and its trait impls.\n\
     ```\n\n\
     ```docs/impl-plan/milestone-01-top.md\n\
     # Milestone 01 -- Top setup\n\n\
     ## Tasks\n\n\
     <!-- detail-pending\nDM2cd fills the task list. -->\n\
     ```\n"
        .into()
}

fn dm2c_critique_response() -> String {
    "Critique done.\n\n\
     ```docs/critiques/DM2c-critique.md\n\
     # DM2c Critique\n\nNo blockers.\n\
     ```\n"
        .into()
}

// DM2cd is a placeholder-mode milestone walk. The orchestrator
// scopes the work session to the FIRST milestone whose body still
// contains `<!-- detail-pending`; the work response below
// replaces the stub with a real task list (placeholder removed).
// Critique-then-advance closes the milestone; the auto driver sees
// no more pending milestones and the step gate becomes clean.
fn dm2cd_work_response() -> String {
    "Detailing milestone 01.\n\n\
     ```docs/impl-plan/milestone-01-top.md\n\
     # Milestone 01 -- Top setup\n\n\
     ## Tasks\n\n\
     - [ ] `src/model/top.rs::Top` -- define Top stub\n\
     - [ ] `src/lib.rs::dump_topology` -- keep dump_topology callable\n\
     ```\n"
        .into()
}

fn dm2cd_critique_response() -> String {
    "Critique done.\n\n\
     ```docs/critiques/DM2cd-critique.md\n\
     # DM2cd Critique\n\nNo blockers.\n\
     ```\n"
        .into()
}

// Ordered (step, work_response, critique_response) tuples driving the
// walk. Adding a step is a matter of appending another tuple here +
// extending the corresponding helper above. Each entry is one
// work + critique cycle; milestone-walk steps with a single
// pre-scripted milestone (DM2cd here) also fit this shape because
// the auto driver runs (work, critique, advance) per milestone.
fn docs_only_dm_script() -> Vec<(&'static str, String, String)> {
    vec![
        ("DM0", dm0_work_response(), dm0_critique_response()),
        ("DM1", dm1_work_response(), dm1_critique_response()),
        ("DM2a", dm2a_work_response(), dm2a_critique_response()),
        ("DM2b", dm2b_work_response(), dm2b_critique_response()),
        ("DM2c", dm2c_work_response(), dm2c_critique_response()),
        ("DM2cd", dm2cd_work_response(), dm2cd_critique_response()),
    ]
}

// -------------------------------------------------------------------
// Auto-mode mocked end-to-end test.
// -------------------------------------------------------------------

#[test]
fn e2e_auto_walks_docs_only_dm_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);

    // Queue work + critique responses for each step in order. Mock
    // responses unwrap to "" on queue exhaustion, which is fine -- a
    // successful walk only pulls 2 * #steps responses (one per
    // sub-session).
    let agent = MockAgent::new();
    for (_, work, critique) in docs_only_dm_script() {
        agent.enqueue(work);
        agent.enqueue(critique);
    }

    // Stdin script: one "/end-session" per critique session. Work
    // sessions wind down on a clean structural gate without ever
    // reaching RequestUserInput, so they consume nothing.
    let n_steps = docs_only_dm_script().len();
    let stdin_bytes = "/end-session\n".repeat(n_steps);
    let stdin = Cursor::new(stdin_bytes.into_bytes());
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let mut host = TerminalHost::new(agent, stdin, &mut stdout, &mut stderr);

    run_auto(auto_opts(&project, StepMode::Auto), &mut host).expect("run_auto should walk to end");

    // Every step's artifacts on disk.
    for (path, marker) in [
        ("docs/spec.md", "Clock: 2 GHz"),
        ("docs/targets.md", "Throughput"),
        ("docs/testbench.md", "Scoreboard"),
        ("docs/analysis/decomposition.md", "## Operation: combine"),
        ("docs/analysis/data-movement.md", "One-pass"),
        ("docs/analysis/pipeline-mapping.md", "Stage 0"),
        ("docs/impl-plan/plan.md", "Milestone 1"),
        // DM2cd rewrites this with the placeholder removed and a
        // real task list landed; check the post-DM2cd content.
        (
            "docs/impl-plan/milestone-01-top.md",
            "- [ ] `src/model/top.rs",
        ),
        ("docs/critiques/DM0-critique.md", "No blockers"),
        ("docs/critiques/DM1-critique.md", "No blockers"),
        ("docs/critiques/DM2a-critique.md", "No blockers"),
        ("docs/critiques/DM2b-critique.md", "No blockers"),
        ("docs/critiques/DM2c-critique.md", "No blockers"),
        ("docs/critiques/DM2cd-critique.md", "No blockers"),
    ] {
        let body = std::fs::read_to_string(project.join(path))
            .unwrap_or_else(|err| panic!("expected {path} on disk: {err}"));
        assert!(
            body.contains(marker),
            "expected `{marker}` in {path}; got:\n{body}",
        );
    }

    // DM2cd must have removed the placeholder marker from the
    // milestone stub. This is the structural proof that the
    // placeholder-mode milestone walk fired end-to-end.
    let detailed =
        std::fs::read_to_string(project.join("docs/impl-plan/milestone-01-top.md")).unwrap();
    assert!(
        !detailed.contains("<!-- detail-pending"),
        "DM2cd should have stripped the placeholder marker; body:\n{detailed}",
    );

    // state.toml: every walked step's gate flag flipped, current_step
    // landed at the step AFTER the last one we drove (DM2d is next
    // after DM2cd).
    let state = State::load(&project.join(".sim-flow")).unwrap();
    for step in ["DM0", "DM1", "DM2a", "DM2b", "DM2c", "DM2cd"] {
        assert!(
            state.is_passed(step),
            "expected {step} gate flag set in state.toml; gates: {:?}",
            state.gates,
        );
    }
    assert_eq!(
        state.current_step, "DM2d",
        "current_step should be DM2d (next after DM2cd)",
    );

    // No Error diagnostic from any step we drove explicitly
    // (DM0..DM2cd). DM2d is the first step the auto loop reaches
    // without queued responses (it has the cargo-build gate that's
    // covered by the full-flow test below), so an "exceeded
    // max_auto_iters" for DM2d is the expected terminator -- but
    // anything mentioning DM0..DM2cd would mean a transition we DID
    // drive misbehaved.
    let stderr_str = String::from_utf8_lossy(&stderr);
    for driven in ["DM0", "DM1", "DM2a", "DM2b", "DM2c", "DM2cd"] {
        let leading_marker = format!("auto: {driven} ");
        assert!(
            !stderr_str.contains(&leading_marker)
                || !stderr_str.contains("exceeded max_auto_iters"),
            "auto run hit max_auto_iters on driven step {driven}; stderr:\n{stderr_str}",
        );
    }
    // The auto loop should have entered DM2d (proving DM2cd advanced
    // cleanly) before parking. Beyond DM2d is the slow-path
    // full-flow test that runs cargo.
    assert!(
        stderr_str.contains("DM2d"),
        "auto run should have reached DM2d after walking DM0..DM2cd; stderr:\n{stderr_str}",
    );
}

// -------------------------------------------------------------------
// Manual-mode mocked end-to-end test.
// -------------------------------------------------------------------

#[test]
fn e2e_manual_drives_docs_only_dm_flow() {
    // The manual flow is the inverse of auto: the orchestrator parks
    // after handshake and waits for the host to issue RunStep /
    // Advance commands. This test plays the role of the dashboard's
    // button presses + per-step LLM dispatcher in one scripted
    // TestHost queue.
    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);

    let mut host = TestHost::new();

    // 1. Initial handshake.
    host.enqueue(HostEvent::Hello {
        protocol_version: PROTOCOL_VERSION.into(),
        host: HostInfo {
            name: "e2e-manual-mocked".into(),
            version: "0.0.0".into(),
        },
        capabilities: vec!["text".into(), "user-input".into(), "llm-request".into()],
    });

    // 2. Per-step: RunStep work -> LLM response (one turn), then
    //    RunStep critique -> LLM response + /end-session, then Advance.
    for (step, work, critique) in docs_only_dm_script() {
        // Work sub-session: dispatch and wind down on the structural gate.
        host.enqueue(HostEvent::RunStep {
            step: step.into(),
            kind: SessionKindOut::Work,
        });
        host.enqueue_llm_response("lr-1", work);

        // Critique sub-session: write critique then `/end-session`.
        host.enqueue(HostEvent::RunStep {
            step: step.into(),
            kind: SessionKindOut::Critique,
        });
        host.enqueue_llm_response("lr-1", critique);
        host.enqueue(HostEvent::UserMessage {
            text: "/end-session".into(),
        });

        host.enqueue(HostEvent::Advance { step: step.into() });
    }

    // 3. Clean shutdown after the last Advance.
    host.enqueue(HostEvent::Shutdown);

    run_auto(auto_opts(&project, StepMode::Manual), &mut host)
        .expect("manual-mode run_auto should walk to end");

    // state.toml: all walked steps passed, current_step landed at
    // the step AFTER the last one we drove.
    let state = State::load(&project.join(".sim-flow")).unwrap();
    for step in ["DM0", "DM1", "DM2a", "DM2b", "DM2c", "DM2cd"] {
        assert!(
            state.is_passed(step),
            "expected {step} gate flag set; gates: {:?}",
            state.gates,
        );
    }
    assert_eq!(state.current_step, "DM2d");

    // Sub-session bracket invariants: for each step we drove, we
    // must see a SubSessionStarted/Ended pair for Work AND for
    // Critique, in that order, both with outcome=completed.
    let brackets: Vec<&Event> = host
        .written
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::SubSessionStarted { .. } | Event::SubSessionEnded { .. }
            )
        })
        .collect();
    // 2 sub-sessions * 6 steps * 2 events = 24.
    let expected_brackets = docs_only_dm_script().len() * 2 * 2;
    assert_eq!(
        brackets.len(),
        expected_brackets,
        "expected {expected_brackets} sub-session bracket events, got {} ({:?})",
        brackets.len(),
        brackets,
    );
    // Spot-check the first Work bracket: DM0.Work, started before ended.
    match (brackets[0], brackets[1]) {
        (
            Event::SubSessionStarted {
                step: a_step,
                kind: a_kind,
            },
            Event::SubSessionEnded {
                step: b_step,
                kind: b_kind,
                outcome,
            },
        ) => {
            assert_eq!(a_step, "DM0");
            assert!(matches!(a_kind, SessionKindOut::Work));
            assert_eq!(b_step, "DM0");
            assert!(matches!(b_kind, SessionKindOut::Work));
            assert_eq!(outcome, "completed");
        }
        other => {
            panic!("expected (SubSessionStarted, SubSessionEnded) at brackets[0..2]: {other:?}")
        }
    }

    // StateAdvanced events: one per Advance command, with from -> to
    // matching the registered order.
    let advances: Vec<(String, Option<String>)> = host
        .written
        .iter()
        .filter_map(|e| match e {
            Event::StateAdvanced { from, to } => Some((from.clone(), to.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        advances,
        vec![
            ("DM0".into(), Some("DM1".into())),
            ("DM1".into(), Some("DM2a".into())),
            ("DM2a".into(), Some("DM2b".into())),
            ("DM2b".into(), Some("DM2c".into())),
            ("DM2c".into(), Some("DM2cd".into())),
            ("DM2cd".into(), Some("DM2d".into())),
        ],
        "StateAdvanced events should chain through the registered DM order",
    );

    // No Error diagnostics escaped.
    let err_diags: Vec<&str> = host
        .written
        .iter()
        .filter_map(|e| match e {
            Event::Diagnostic {
                level: DiagnosticLevel::Error,
                message,
            } => Some(message.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        err_diags.is_empty(),
        "manual-mode walk should not emit any Error diagnostics; got: {err_diags:?}",
    );
}

// -------------------------------------------------------------------
// Full DM-flow walk DM0 -> DM4b (the slow path).
//
// This test goes ALL the way: the same auto-mode walk as
// `e2e_auto_walks_docs_only_dm_flow` but extended past DM2cd
// through DM2d (model-impl, gates on cargo fmt/clippy/build/test),
// DM3a/DM3ad/DM3b/DM3c (testbench + test-impl, more cargo), and
// DM4a/DM4ad/DM4b (perf-analysis, gates on `experiments.db` having
// at least one row).
//
// Strategy. Cargo-running gates can't be skipped from the test, so
// instead of having the mock LLM author Rust code that has to
// compile across template revisions, the test PRE-WRITES every
// artifact each step's gate expects before launching `run_auto`.
// The MockAgent then returns `"Done."` for every dispatch -- the
// auto driver's wind-down path sees a clean structural gate
// immediately and ends each sub-session without iterating. The
// flow's *transitions* are what this test pins; the artifact
// authoring quality is the real backend binaries' job.
//
// What's exercised: every step's gate, every step transition, both
// placeholder-mode milestone walks (DM2cd / DM3ad / DM4ad) and
// execution-mode milestone walks (DM2d / DM3b / DM3c / DM4b), the
// `ExperimentsRecorded` gate on DM4b (seeded via
// `ExperimentIndex::insert_run` before launch), and the block-diagram
// auto-render hook on the DM2d -> DM3a boundary.
//
// Why it's `#[ignore]`: walking through the cargo-gated steps runs
// `cargo fmt`, `cargo clippy`, `cargo build`, and `cargo test` in a
// generated project that depends on `foundation-framework`. On a
// clean target dir the first cargo invocation builds the workspace
// dep graph, which is more than a CI-budget test should pay. Run
// on demand:
//
//   cargo test -p sim-flow --test e2e_mocked -- --ignored \
//       e2e_auto_walks_full_dm_flow

#[test]
#[ignore = "walks the full DM flow; runs cargo fmt/clippy/build/test \
            on a bootstrapped project. Slow (~1-3 min). Run with \
            `--ignored` when iterating on flow transitions or gates."]
fn e2e_auto_walks_full_dm_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let project = bootstrap_full_project(&tmp);
    pre_write_full_artifacts(&project);
    pre_write_full_milestones(&project);
    pre_write_full_critiques(&project);
    pre_write_dm3_tests(&project);
    seed_experiments_db(&project);

    // Mock returns "Done." for every dispatch. With all artifacts
    // pre-written, work sessions see effective_artifacts_empty for
    // Work + can_auto_wind_down + clean structural gate -> SessionEnd.
    // Critique sessions see effective_artifacts_empty=false (Critique
    // text non-empty, no tools) and fall through to RequestUserInput,
    // which the stdin "/end-session" lines below satisfy.
    let agent = MockAgent::new();
    // Over-enqueue. Mock returns "" on exhaustion, which the
    // orchestrator's empty-retry path handles cleanly when paired
    // with the pre-written artifacts (the wind-down hits before
    // empty-retry exhausts).
    for _ in 0..200 {
        agent.enqueue("Done.");
    }
    // One /end-session per critique session (one per step, 14 steps).
    let stdin_bytes = "/end-session\n".repeat(14);
    let stdin = Cursor::new(stdin_bytes.into_bytes());
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let mut host = TerminalHost::new(agent, stdin, &mut stdout, &mut stderr);

    // Sanity-probe every cargo-running step's structural gate
    // BEFORE launching run_auto. Saves a 40 s false-positive run
    // when a gate fixture (template path, src/model/top.rs shape,
    // milestone file naming) is broken: if anything trips here we
    // panic with the failing check verbatim instead of letting
    // run_auto burn through max_auto_iters and report a generic
    // "exceeded cap" diagnostic.
    {
        use sim_flow::__internal::steps::registry_for;
        let registry = registry_for(Flow::DirectModeling);
        for step_id in ["DM2d", "DM3b", "DM3c", "DM4b"] {
            let step = registry.get(step_id).unwrap();
            // Use the same filter the orchestrator uses
            // (CritiqueClean skipped; pre-written critiques are
            // checked by the real run separately).
            let checks: Vec<_> = step
                .gate_checks
                .iter()
                .filter(|c| !matches!(c, sim_flow::gate::GateCheck::CritiqueClean { .. }))
                .cloned()
                .collect();
            let report = sim_flow::gate::evaluate(&project, &checks).expect("gate eval");
            if !report.failures.is_empty() {
                panic!(
                    "pre-run structural-gate probe failed for {step_id}:\n{}",
                    report
                        .failures
                        .iter()
                        .map(|f| format!("  - {}: {}", f.description, f.reason))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
        }
    }

    let mut opts = auto_opts(&project, StepMode::Auto);
    // Cargo gates can take a few seconds; give the orchestrator
    // headroom in case any single sub-session needs more than the
    // 4-iter default.
    opts.max_auto_iters = 6;
    opts.max_critique_iters = 3;
    opts.max_llm_requests = 200;
    run_auto(opts, &mut host).expect("full-flow run_auto should walk to DM4b");

    // state.toml: every DM step's gate flag flipped. After DM4b
    // (the last step) advances, `state.current_step` stays at
    // "DM4b" (try_advance sees no successor and leaves
    // current_step unchanged) but gates[DM4b].passed is true.
    let state = State::load(&project.join(".sim-flow")).unwrap();
    for step in [
        "DM0", "DM1", "DM2a", "DM2b", "DM2c", "DM2cd", "DM2d", "DM3a", "DM3ad", "DM3b", "DM3c",
        "DM4a", "DM4ad", "DM4b",
    ] {
        assert!(
            state.is_passed(step),
            "expected {step} gate flag set; gates: {:?}\nstderr:\n{}",
            state.gates,
            String::from_utf8_lossy(&stderr),
        );
    }

    // Block-diagram render artifacts land in .sim-flow/ on the
    // DM2d -> DM3a advance boundary (see auto.rs::try_advance ->
    // dump_topology). Their absence isn't fatal (the render is
    // best-effort and emits a Warning on failure), but their
    // presence is the cleanest signal that DM2d actually advanced.
    let stderr_str = String::from_utf8_lossy(&stderr);
    assert!(
        stderr_str.contains("DM2d") && stderr_str.contains("DM3a"),
        "auto run should have spanned the DM2d -> DM3a boundary; \
         stderr:\n{stderr_str}",
    );

    // No Error diagnostics escaped.
    assert!(
        !stderr_str.contains("[error]"),
        "full-flow walk should produce no Error diagnostics; \
         stderr:\n{stderr_str}",
    );
}

// -------------------------------------------------------------------
// Full-flow helpers.
// -------------------------------------------------------------------

fn bootstrap_full_project(tmp: &tempfile::TempDir) -> PathBuf {
    let library_path = PathBuf::from("/Users/mneilly/nta/sim-models");
    let opts = NewModelOptions {
        project_name: "mocked_full_dm".into(),
        destination: tmp.path().to_path_buf(),
        foundation_root: foundation_root(),
        library_path: library_path.display().to_string(),
        // Defer the cargo build until DM2d hits its gate; saves a
        // round-trip during init.
        skip_cargo_check: true,
    };
    let outcome = new_model(&opts).expect("new_model bootstrap");

    // Replace src/model/top.rs with code that contains the literal
    // text `impl HasLogic for Top` (DM2d's grep gate matches on text,
    // not macro expansion -- the template's `impl_structural_has_logic!`
    // would otherwise miss).
    let top_path = outcome.project_dir.join("src/model/top.rs");
    std::fs::write(
        &top_path,
        "//! Top module (test fixture).\n\n\
         use foundation_framework::{HasInstances, Module};\n\
         use foundation_framework::model::dataflow::HasLogic;\n\n\
         #[derive(Clone, Debug, Default)]\n\
         pub struct Top;\n\n\
         impl Module for Top {\n\
            \x20   fn module_name(&self) -> &'static str { \"top\" }\n\
         }\n\n\
         impl HasInstances for Top {}\n\n\
         impl HasLogic for Top {\n\
            \x20   fn has_logic(&self) -> bool { false }\n\
         }\n",
    )
    .unwrap();

    outcome.project_dir
}

fn pre_write_full_artifacts(project: &Path) {
    let writes: &[(&str, &str)] = &[
        (
            "docs/spec.md",
            "# Spec\n\nClock: 2 GHz\nGates per cycle: 50\nNode: 7 nm\n",
        ),
        (
            "docs/targets.md",
            "# Targets\n\nThroughput: 100 cycles per item.\n",
        ),
        (
            "docs/testbench.md",
            "# Testbench\n\n\
             Components: Sequencer, Driver, Monitor, Scoreboard.\n\
             Baseline: lib:examples/00-simple-pipeline/test/\n",
        ),
        (
            "docs/analysis/decomposition.md",
            "# Decomposition\n\n## Operation: combine\n\nMerges streams.\n",
        ),
        (
            "docs/analysis/data-movement.md",
            "# Data Movement\n\nOne-pass streaming.\n",
        ),
        (
            "docs/analysis/pipeline-mapping.md",
            "# Pipeline Mapping\n\nStage 0: ingest. Stage 1: combine.\n",
        ),
        (
            "docs/impl-plan/plan.md",
            "# Implementation Plan\n\n## Milestone 1 -- Top setup\n",
        ),
        (
            "docs/test-plan/test-plan.md",
            "# Test Plan\n\n\
             Sequencer, Driver, Monitor, Scoreboard wired against \
             `docs/spec.md` and `docs/targets.md`.\n",
        ),
        (
            "docs/test-plan/coverage.md",
            "# Coverage\n\nMeasured with cargo-tarpaulin; target 80%.\n",
        ),
        (
            "docs/perf-plan/perf-plan.md",
            "# Performance Plan\n\n## Milestone 1 -- baseline run\n",
        ),
        (
            "docs/analysis/perf-report.md",
            "# Performance Report\n\n\
             Throughput: 100 cycles/item. Latency: 7 cycles.\n",
        ),
    ];
    for (rel, body) in writes {
        let path = project.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, body).unwrap();
    }
}

fn pre_write_full_milestones(project: &Path) {
    // All milestones land pre-resolved: no placeholder marker, all
    // task rows `- [x]` (so MilestonesAllResolved with
    // forbid_deferred=true passes immediately).
    let resolved = "# Milestone\n\n## Tasks\n\n- [x] task one\n- [x] task two\n";
    let mut writes: Vec<(String, &str)> = Vec::new();
    writes.push(("docs/impl-plan/milestone-01-top.md".into(), resolved));
    writes.push((
        "docs/test-plan/tb-milestone-01-scoreboard.md".into(),
        resolved,
    ));
    writes.push(("docs/test-plan/test-milestone-01-smoke.md".into(), resolved));
    writes.push((
        "docs/perf-plan/perf-milestone-01-baseline.md".into(),
        resolved,
    ));
    for (rel, body) in writes {
        let path = project.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, body).unwrap();
    }
}

fn pre_write_full_critiques(project: &Path) {
    // One markdown critique per step, all clean (no BLOCKER /
    // UNRESOLVED markers). The orchestrator's CritiqueClean gate
    // resolves the JSON sibling first, then the .md; without a
    // JSON sibling, `Critique::parse` reads the markdown body's
    // line markers (none -> empty findings -> clean).
    let body = "# Critique\n\nNo blockers found.\n";
    let dir = project.join("docs/critiques");
    std::fs::create_dir_all(&dir).unwrap();
    for step in [
        "DM0", "DM1", "DM2a", "DM2b", "DM2c", "DM2cd", "DM2d", "DM3a", "DM3ad", "DM3b", "DM3c",
        "DM4a", "DM4ad", "DM4b",
    ] {
        std::fs::write(dir.join(format!("{step}-critique.md")), body).unwrap();
    }
}

fn pre_write_dm3_tests(project: &Path) {
    // DM3b's gate greps `tests/` for `SimEnv|Sequencer|Driver|
    // Monitor|Scoreboard`. The mention can be a comment -- the
    // grep just looks for the literal. Use a self-contained test
    // file that compiles standalone (no foundation-framework imports
    // beyond what the template's Cargo.toml already provides).
    let tests_dir = project.join("tests");
    std::fs::create_dir_all(&tests_dir).unwrap();
    std::fs::write(
        tests_dir.join("dm3_smoke.rs"),
        "//! DM3 smoke test (Sequencer + Driver + Monitor + Scoreboard refs).\n\n\
         #[test]\n\
         fn dm3_smoke() {\n\
            \x20   // Sequencer/Driver/Monitor/Scoreboard hooked via SimEnv.\n\
            \x20   let crate_name = env!(\"CARGO_PKG_NAME\");\n\
            \x20   assert!(!crate_name.is_empty());\n\
         }\n",
    )
    .unwrap();
}

fn seed_experiments_db(project: &Path) {
    let dot = project.join(".sim-flow");
    let index = ExperimentIndex::open(&dot).expect("open experiments.db");
    let row = RunRow {
        id: 0,
        run_id: "mocked-run-0001".into(),
        timestamp: "1970-01-01T00:00:00Z".into(),
        git_commit: "0000000".into(),
        git_branch: None,
        git_dirty: false,
        config_fingerprint: "mocked".into(),
        manifest_path: None,
        workload: Some("baseline".into()),
        candidate: None,
        study: None,
        metrics_summary: None,
        parent_run_id: None,
        sweep_parameter: None,
        sweep_value: None,
        tags: None,
        notes: None,
        lifecycle: "complete".into(),
    };
    index.insert_run(&row).expect("seed experiments.db row");
}
