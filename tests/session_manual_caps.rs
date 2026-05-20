//! Integration tests for manual_caps.
//!
//! Shared helpers live in `tests/common/mod.rs`.

use sim_flow::session::MockAgent;
use sim_flow::session::host::TestHost;
use sim_flow::session::protocol::{Event, HostEvent};

mod common;
use common::{auto_opts, hello, init_project};

#[test]
fn auto_mode_cap_exceeded_flips_to_manual_and_emits_step_mode_changed() {
    // The orchestrator's per-session cap fires after max_auto_iters
    // bad responses. Today the auto driver flips the shared step-
    // mode flag to manual and emits StepModeChanged so the dashboard
    // toggle matches reality. The parking loop then takes over;
    // here we have no further script so the run exits cleanly.
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    let mut mock = MockAgent::new();
    // The very first sub-session in run_auto reads Hello from the
    // actual host (every sub-session AFTER the first uses a
    // synthetic Hello queued by AutoHost).
    host.enqueue(hello());
    let bad = "```docs/spec.md\n# Spec\n\nClock: 2 GHz\n```\n";
    mock.enqueue(bad);
    mock.enqueue(bad);
    // No /end-session needed: AutoHost queues a Cancel on cap and
    // the orchestrator stops itself.

    let mut opts = auto_opts(&project, StepMode::Auto);
    opts.max_auto_iters = 2;
    sim_flow::session::run_auto(opts, &mut host, &mut mock).unwrap();

    let saw_to_manual = host
        .written
        .iter()
        .any(|e| matches!(e, Event::StepModeChanged { mode } if matches!(mode, StepMode::Manual)));
    assert!(
        saw_to_manual,
        "cap-exceeded path should emit StepModeChanged {{ manual }}; events: {:?}",
        host.written,
    );
    let saw_diag = host.written.iter().any(|e| {
        matches!(
            e,
            Event::Diagnostic { level, message }
                if matches!(level, sim_flow::session::DiagnosticLevel::Error)
                    && message.contains("flipping to manual mode")
        )
    });
    assert!(
        saw_diag,
        "cap-exceeded path should emit a clarifying Diagnostic; events: {:?}",
        host.written,
    );
}

#[test]
#[ignore = "pre-existing failure on mneilly/ai-flow; tracked separately from sim-flow extraction"]
fn auto_mode_no_progress_cap_fires_when_critique_count_stays_flat() {
    // Two caps on the critique-retry loop now: an absolute one
    // (max_critique_iters -- backstop even for progressing runs)
    // and a no-progress one (max_critique_no_progress_iters --
    // catches plateaus / oscillations early). This test pins the
    // no-progress path: we feed the orchestrator a critique that
    // reports a flat 1 blocker on every pass, so the absolute cap
    // is far away but the no-progress streak trips after the
    // configured number of stuck retries.
    use sim_flow::session::StderrPresenter;
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    // Pre-write a clean spec.md so each Work session sees the
    // structural gate clean immediately and winds down on the
    // first "Done." mock response. That removes Work-side
    // variability from the test and lets every cycle through
    // run_auto_loop drive ONE Critique pass.
    std::fs::write(
        project.join("docs/spec.md"),
        "# Spec\n\nClock: 2 GHz\nGates per cycle: 50\nNode: 7 nm\n",
    )
    .unwrap();

    let mut agent = MockAgent::new();
    // Per cycle: one "Done." for Work (wind-down on clean gate),
    // one fenced critique with a BLOCKER for Critique. Three
    // cycles -- the 3rd one's no_progress_iters=2 trips a cap
    // of 1.
    let bad_critique = "Critique done.\n\n\
        ```docs/critiques/DM0-critique.md\n\
        # DM0 Critique\n\nBLOCKER: missing technology details.\n\
        ```\n";
    for _ in 0..3 {
        agent.enqueue("Done.");
        agent.enqueue(bad_critique);
    }
    // Plus a safety pad so a runaway under-cap doesn't hang.
    for _ in 0..6 {
        agent.enqueue("Done.");
    }

    let stdin_bytes = "/end-session\n".repeat(3);
    let stdin = std::io::Cursor::new(stdin_bytes.into_bytes());
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let mut host = StderrPresenter::new("mock", stdin, &mut stdout, &mut stderr);

    let mut opts = auto_opts(&project, StepMode::Auto);
    opts.max_auto_iters = 4;
    // Absolute cap deliberately far away -- we want the
    // no-progress cap to be what trips. If the absolute cap
    // fires first the diagnostic message names a different
    // reason and the assertion below catches it.
    opts.max_critique_iters = 50;
    // Pass 1 has no prior count (no_progress stays 0); pass 2
    // bumps no_progress to 1 (1>1 false); pass 3 bumps to 2
    // (2>1 true) -> fire.
    opts.max_critique_no_progress_iters = 1;

    sim_flow::session::run_auto(opts, &mut host, &mut agent).unwrap();

    // The terminator must be the no-progress diagnostic, not the
    // absolute one. The wording is the stable contract; the
    // VS Code extension keys on the setting name in the message.
    let stderr_str = String::from_utf8(stderr).unwrap();
    assert!(
        stderr_str.contains("made no progress for") && stderr_str.contains("DM0"),
        "expected the no-progress diagnostic on DM0; stderr:\n{stderr_str}",
    );
    assert!(
        !stderr_str.contains("still has 1 gate-failing finding(s) after"),
        "absolute-cap diagnostic should NOT fire when no-progress is the smaller cap; \
         stderr:\n{stderr_str}",
    );
    // And the run should have flipped to manual mode (matching
    // the existing absolute-cap behavior).
    assert!(
        stderr_str.contains("step mode now: Manual"),
        "no-progress cap should still flip to manual; stderr:\n{stderr_str}",
    );
}

#[test]
fn manual_mode_shutdown_terminates_cleanly() {
    use sim_flow::session::protocol::StepMode;

    let tmp = tempfile::tempdir().unwrap();
    let project = init_project(&tmp);
    let mut host = TestHost::new();
    let mut mock = MockAgent::new();
    host.enqueue(hello());
    host.enqueue(HostEvent::Shutdown);

    sim_flow::session::run_auto(auto_opts(&project, StepMode::Manual), &mut host, &mut mock)
        .unwrap();

    let last = host.written.last().unwrap();
    match last {
        Event::SessionEnd { reason, message } => {
            assert_eq!(
                *reason,
                sim_flow::session::protocol::SessionEndReason::Completed
            );
            assert!(
                message.as_deref().unwrap_or("").contains("shut down"),
                "shutdown SessionEnd should mention shutdown; got {message:?}"
            );
        }
        other => panic!("expected SessionEnd, got {other:?}"),
    }
}
