//! End-to-end manual-mode runner against an arbitrary `CliAgent`
//! backend. Spawns `sim-flow auto --step-mode manual` as a subprocess,
//! speaks the JSONL session protocol on its stdin/stdout, dispatches
//! `RequestLlmResponse` events to the chosen LLM backend, and walks
//! the manual flow by sending pre-scripted commands at each
//! transition point — `RunStep` (work) -> `RunStep` (critique) ->
//! `Advance` per step until the flow ends.
//!
//! What the "user input mock" is here: the orchestrator's manual
//! mode parks after handshake waiting for host commands. In
//! production, the dashboard's button presses become these commands.
//! In this driver, a deterministic sequence stands in for the user.
//! LLM dispatch still goes to a real backend (LM Studio / Ollama /
//! Claude) — the mocking is purely the human-in-the-loop layer.
//!
//! Usage:
//!
//! ```sh
//! cargo run -p sim-flow --bin e2e_manual -- \
//!     --project-dir /Users/mneilly/nta/sim-models/users/mneilly/rgb_toy \
//!     --foundation-root /Users/mneilly/nta/sim-foundation \
//!     --backend openai-compat \
//!     --base-url http://localhost:8012/v1 \
//!     --model qwen3.6
//! ```
//!
//! Exit codes: 0 ok, 1 runtime error, 2 bad args / setup.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Instant;

use serde_json::Value;
use sim_flow::session::JsonlCapture;
use sim_flow::session::agent::{ClaudeAgent, CliAgent, OllamaAgent, OpenAiCompatAgent};
use sim_flow::session::ingest_spec_file;
use sim_flow::session::protocol::{
    DiagnosticLevel, Event, HostEvent, HostInfo, LlmMessage, PROTOCOL_VERSION, SessionKindOut,
    StepMode,
};
use sim_flow::test_validation::validate_step_advanced;

fn main() {
    let args: Args = match Args::parse(std::env::args().collect()) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    if let Err(err) = run(&args) {
        eprintln!("\ne2e_manual: FAILED: {err}");
        std::process::exit(1);
    }
}

struct Args {
    project_dir: PathBuf,
    foundation_root: PathBuf,
    spec: Option<PathBuf>,
    sim_flow_bin: PathBuf,
    backend: String,
    model: Option<String>,
    max_auto_iters: u32,
    max_critique_iters: u32,
    max_critique_no_progress_iters: u32,
    max_llm_requests: u32,
    /// Optional Unix socket path passed to `sim-flow auto
    /// --watch-socket ...` so the dashboard's "Attach to Running
    /// Watcher" picker can list this run. When unset (and
    /// `--no-watch-socket` is not passed), defaults to a
    /// per-pid path under the system temp dir so every run is
    /// observable out of the box.
    watch_socket: Option<PathBuf>,
    /// Optional JSONL capture file. When set, every protocol
    /// event in both directions (orchestrator -> host AND
    /// host -> orchestrator) is teed to this path as
    /// `{"ts": <unix_ms>, "dir": "out"|"in", "event": {...}}`.
    /// Used by the model-robustness study; the capture is
    /// observational only.
    capture_jsonl: Option<PathBuf>,
}

impl Args {
    fn parse(argv: Vec<String>) -> std::result::Result<Self, String> {
        let mut project_dir: Option<PathBuf> = None;
        let mut foundation_root: Option<PathBuf> = None;
        let mut spec: Option<PathBuf> = None;
        let mut sim_flow_bin: Option<PathBuf> = None;
        let mut backend: Option<String> = None;
        let mut model: Option<String> = None;
        let mut max_auto_iters = 6u32;
        let mut max_critique_iters = 10u32;
        let mut max_critique_no_progress_iters = 3u32;
        let mut max_llm_requests = 50u32;
        let mut watch_socket: Option<PathBuf> = None;
        let mut watch_disabled = false;
        let mut capture_jsonl: Option<PathBuf> = None;
        let mut iter = argv.into_iter().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--project-dir" => project_dir = iter.next().map(PathBuf::from),
                "--foundation-root" => foundation_root = iter.next().map(PathBuf::from),
                "--spec" => spec = iter.next().map(PathBuf::from),
                "--sim-flow-bin" => sim_flow_bin = iter.next().map(PathBuf::from),
                "--backend" => backend = iter.next(),
                "--model" => model = iter.next(),
                "--max-auto-iters" => {
                    max_auto_iters = iter
                        .next()
                        .ok_or_else(|| "--max-auto-iters needs a value".to_string())?
                        .parse()
                        .map_err(|err| format!("--max-auto-iters: {err}"))?
                }
                "--max-critique-iters" => {
                    max_critique_iters = iter
                        .next()
                        .ok_or_else(|| "--max-critique-iters needs a value".to_string())?
                        .parse()
                        .map_err(|err| format!("--max-critique-iters: {err}"))?
                }
                "--max-critique-no-progress-iters" => {
                    max_critique_no_progress_iters = iter
                        .next()
                        .ok_or_else(|| {
                            "--max-critique-no-progress-iters needs a value".to_string()
                        })?
                        .parse()
                        .map_err(|err| format!("--max-critique-no-progress-iters: {err}"))?
                }
                "--max-llm-requests" => {
                    max_llm_requests = iter
                        .next()
                        .ok_or_else(|| "--max-llm-requests needs a value".to_string())?
                        .parse()
                        .map_err(|err| format!("--max-llm-requests: {err}"))?
                }
                "--watch-socket" => watch_socket = iter.next().map(PathBuf::from),
                "--no-watch-socket" => watch_disabled = true,
                "--capture-jsonl" => capture_jsonl = iter.next().map(PathBuf::from),
                "--help" | "-h" => {
                    println!(
                        "usage: e2e_manual --project-dir <P> --foundation-root <F> \
                         --backend {{openai-compat|ollama|claude}} [--model <M>] \
                         [--spec <PATH>] [--sim-flow-bin <PATH>] \
                         [--max-auto-iters <N>] [--max-critique-iters <N>] \
                         [--max-critique-no-progress-iters <N>] \
                         [--max-llm-requests <N>] \
                         [--watch-socket <PATH>] [--no-watch-socket] \
                         [--capture-jsonl <PATH>]\n\
                         \n\
                         By default a `--watch-socket` path is auto-generated under the \
                         system temp dir so the VS Code dashboard's `sim-flow: Attach to \
                         Running Watcher` picker can attach as a read-only viewer. Pass \
                         `--no-watch-socket` to disable, or `--watch-socket <PATH>` to \
                         choose your own path.\n\
                         \n\
                         `--capture-jsonl <PATH>` writes every protocol event in both \
                         directions to a JSONL file. Used by the model-robustness study \
                         to build per-model anomaly catalogs and a replay corpus."
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown flag: {other}")),
            }
        }
        let backend = backend.ok_or_else(|| "--backend is required".to_string())?;
        match backend.as_str() {
            "openai-compat" | "openai_compat" | "openai" | "ollama" | "claude" | "claude-cli" => {}
            other => return Err(format!("unknown backend: {other}")),
        }
        let project_dir = project_dir.ok_or_else(|| "--project-dir is required".to_string())?;
        let foundation_root =
            foundation_root.ok_or_else(|| "--foundation-root is required".to_string())?;
        let sim_flow_bin =
            sim_flow_bin.unwrap_or_else(|| foundation_root.join("target/debug/sim-flow"));
        // Default watch-socket: <tmp>/sim-flow-e2e-manual-<pid>.sock,
        // unique per process, removable by the orchestrator on
        // shutdown via the EventTap drop. `--no-watch-socket`
        // suppresses; an explicit `--watch-socket` overrides.
        let watch_socket = if watch_disabled {
            None
        } else {
            Some(watch_socket.unwrap_or_else(|| {
                std::env::temp_dir()
                    .join(format!("sim-flow-e2e-manual-{}.sock", std::process::id()))
            }))
        };
        Ok(Self {
            project_dir,
            foundation_root,
            spec,
            sim_flow_bin,
            backend,
            model,
            max_auto_iters,
            max_critique_iters,
            max_critique_no_progress_iters,
            max_llm_requests,
            watch_socket,
            capture_jsonl,
        })
    }
}

fn run(args: &Args) -> std::result::Result<(), String> {
    if !args.project_dir.exists() {
        return Err(format!(
            "project dir {} does not exist (init it with `sim-flow new model` first)",
            args.project_dir.display(),
        ));
    }
    if !args.project_dir.join(".sim-flow/state.toml").exists() {
        return Err(format!(
            "{} is not initialized (`.sim-flow/state.toml` missing)",
            args.project_dir.display(),
        ));
    }
    if !args.sim_flow_bin.is_file() {
        return Err(format!(
            "sim-flow binary not found at {}",
            args.sim_flow_bin.display()
        ));
    }

    println!(
        "e2e_manual: project_dir     = {}",
        args.project_dir.display()
    );
    println!(
        "e2e_manual: foundation_root = {}",
        args.foundation_root.display()
    );
    println!(
        "e2e_manual: sim-flow-bin    = {}",
        args.sim_flow_bin.display()
    );
    println!("e2e_manual: backend         = {}", args.backend);
    println!(
        "e2e_manual: model           = {}",
        args.model.as_deref().unwrap_or("(default)")
    );
    // Default `SIM_FLOW_LIBRARY_ROOT` to the sibling `sim-models/` of
    // the foundation-root when the caller hasn't already set it. The
    // smoke project lives in a tempdir, so the orchestrator's
    // ancestor-walk auto-detection finds nothing and every
    // `lib:examples/...` / `lib:docs/modeling-guide/...` read errors
    // -- which kneecaps every DM step that's supposed to learn from
    // sim-models. Honor an explicit env override; otherwise probe the
    // sibling and only set if it has the expected layout.
    if std::env::var_os("SIM_FLOW_LIBRARY_ROOT").is_none() {
        if let Some(parent) = args.foundation_root.parent() {
            let candidate = parent.join("sim-models");
            if candidate.join("docs").join("modeling-guide").is_dir()
                && candidate.join("examples").is_dir()
            {
                println!(
                    "e2e_manual: library_root    = {} (auto)",
                    candidate.display()
                );
                // SAFETY: we set the env var before spawning the child;
                // no other thread is reading it yet.
                unsafe {
                    std::env::set_var("SIM_FLOW_LIBRARY_ROOT", &candidate);
                }
            }
        }
    } else {
        println!(
            "e2e_manual: library_root    = {} (env)",
            std::env::var("SIM_FLOW_LIBRARY_ROOT").unwrap_or_default()
        );
    }
    println!();

    // Pre-ingest the spec so the orchestrator's first session has the
    // chunked spec + TOC available. `sim-flow auto --spec ...` runs
    // this hook itself, and we pass `--spec` through too, but doing
    // it here gives an obvious early failure if the spec path is bad.
    if let Some(spec) = &args.spec {
        match ingest_spec_file(spec, &args.project_dir) {
            Ok(s) => println!("e2e_manual: ingested spec -> {} page(s)", s.page_count),
            Err(err) => return Err(format!("ingest_spec_file({}): {err}", spec.display())),
        }
    }

    // Spawn the orchestrator as a child process speaking JSONL over
    // stdio. `--step-mode manual` makes it park after the handshake
    // waiting for our `RunStep` / `Advance` / `Shutdown` commands.
    let mut cmd = Command::new(&args.sim_flow_bin);
    cmd.arg("--foundation-root")
        .arg(&args.foundation_root)
        .arg("--project")
        .arg(&args.project_dir)
        .arg("auto")
        .arg("--step-mode")
        .arg("manual")
        .arg("--llm-backend")
        .arg(&args.backend)
        .arg("--max-auto-iters")
        .arg(args.max_auto_iters.to_string())
        .arg("--max-critique-iters")
        .arg(args.max_critique_iters.to_string())
        .arg("--max-critique-no-progress-iters")
        .arg(args.max_critique_no_progress_iters.to_string())
        .arg("--max-llm-requests")
        .arg(args.max_llm_requests.to_string());
    if let Some(model) = &args.model {
        cmd.arg("--llm-model").arg(model);
    }
    if let Some(spec) = &args.spec {
        cmd.arg("--spec").arg(spec);
    }
    // Pass through `--watch-socket` so the orchestrator binds a
    // read-only event tap. The dashboard's "Attach to Running
    // Watcher" picker discovers this run via the registry the tap
    // writes; observers see history + live events without
    // touching e2e_manual's command channel.
    if let Some(watch) = &args.watch_socket {
        cmd.arg("--watch-socket").arg(watch);
        eprintln!(
            "e2e_manual: watch socket = {} (attach via VS Code: `sim-flow: Attach to Running Watcher`)",
            watch.display()
        );
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Forward sim-flow's stderr (tracing, diagnostics) to our
        // stderr so the user sees what's happening in real time.
        .stderr(Stdio::inherit());

    println!(
        "e2e_manual: launching `{}` ...\n",
        args.sim_flow_bin.display()
    );
    let started = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|err| format!("spawn sim-flow: {err}"))?;

    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");

    // Optional JSONL capture for the model-robustness study. Both
    // the reader (subprocess -> us) and the writer (us ->
    // subprocess) tee through the same `JsonlCapture`, so a single
    // file carries a faithful transcript of every protocol exchange
    // for this trial.
    let capture: Option<JsonlCapture> = match &args.capture_jsonl {
        Some(path) => {
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)
                    .map_err(|err| format!("create capture dir {}: {err}", parent.display()))?;
            }
            let file = std::fs::File::create(path)
                .map_err(|err| format!("create capture file {}: {err}", path.display()))?;
            let cap = JsonlCapture::new(std::io::BufWriter::new(file));
            cap.record_meta(serde_json::json!({
                "kind": "run-start",
                "tool": "e2e_manual",
                "backend": args.backend,
                "model": args.model.clone(),
                "project_dir": args.project_dir.display().to_string(),
                "foundation_root": args.foundation_root.display().to_string(),
                "spec": args.spec.as_ref().map(|p| p.display().to_string()),
                "max_auto_iters": args.max_auto_iters,
                "max_critique_iters": args.max_critique_iters,
                "max_critique_no_progress_iters": args.max_critique_no_progress_iters,
                "max_llm_requests": args.max_llm_requests,
                "pid": std::process::id(),
            }));
            eprintln!("e2e_manual: capture-jsonl = {}", path.display());
            Some(cap)
        }
        None => None,
    };

    // Reader thread: parse one Event per stdout line and forward on
    // a channel so the main loop can do a straight `recv()` even
    // though stdin/stdout are independent file descriptors.
    let (event_tx, event_rx) = channel::<EventFromOrch>();
    let reader_capture = capture.clone();
    let reader_handle = thread::Builder::new()
        .name("e2e-manual-reader".into())
        .spawn(move || reader_loop(stdout, event_tx, reader_capture))
        .map_err(|err| format!("spawn reader thread: {err}"))?;

    let agent: Box<dyn CliAgent> = match args.backend.as_str() {
        "openai-compat" | "openai_compat" | "openai" => {
            Box::new(OpenAiCompatAgent::new(None, args.model.clone(), None, None))
        }
        "ollama" => Box::new(OllamaAgent::new(None, args.model.clone(), None, None)),
        "claude" | "claude-cli" => Box::new(ClaudeAgent::new(args.model.clone(), None, None)),
        _ => unreachable!("validated in Args::parse"),
    };

    let outcome = drive(
        stdin,
        &event_rx,
        agent.as_ref(),
        &args.project_dir,
        capture.as_ref(),
    );

    // Tear down: signal Shutdown if the loop didn't already, then
    // wait for child and reader.
    let _ = reader_handle.join();
    let exit_status = child
        .wait()
        .map_err(|err| format!("wait sim-flow: {err}"))?;

    let elapsed = started.elapsed();
    println!(
        "\ne2e_manual: sim-flow exited with {exit_status:?} after {:.1}s",
        elapsed.as_secs_f64()
    );

    summarize_state(&args.project_dir)?;

    let verdict = outcome?;
    // Convert collected error diagnostics + validation failures into
    // a non-zero exit. The drive() loop logs them as they arrive, so
    // the caller already saw the detail; this final summary makes
    // the failure mode explicit and ensures CI / scripted runs treat
    // a "completed but with errors" run as the failure it actually is.
    let mut bail: Vec<String> = Vec::new();
    if !verdict.errors_seen.is_empty() {
        bail.push(format!(
            "{} orchestrator Error diagnostic(s) (e.g. {})",
            verdict.errors_seen.len(),
            verdict
                .errors_seen
                .first()
                .map(|s| s.as_str())
                .unwrap_or("")
        ));
    }
    if !verdict.validation_failures.is_empty() {
        bail.push(format!(
            "{} post-advance validation failure(s) (e.g. {})",
            verdict.validation_failures.len(),
            verdict
                .validation_failures
                .first()
                .map(|s| s.as_str())
                .unwrap_or("")
        ));
    }
    if !bail.is_empty() {
        return Err(format!("TEST FAILED: {}", bail.join("; ")));
    }
    println!("e2e_manual: TEST PASSED (no errors, all post-advance validations clean)");
    Ok(())
}

/// Outcome wrapper carried over the channel: either a parsed Event
/// or the fact that the reader hit EOF / a parse error. The state
/// machine treats EOF as "session ended" (the orchestrator emits
/// SessionEnd then drops; no further input is expected). `Event` is
/// boxed because it's a large variant relative to the others.
enum EventFromOrch {
    Event(Box<Event>),
    Eof,
    Err(String),
}

fn reader_loop(stdout: ChildStdout, tx: Sender<EventFromOrch>, capture: Option<JsonlCapture>) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                let _ = tx.send(EventFromOrch::Eof);
                return;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Event>(trimmed) {
                    Ok(event) => {
                        if let Some(cap) = &capture {
                            cap.record_out(&event);
                        }
                        if tx.send(EventFromOrch::Event(Box::new(event))).is_err() {
                            return;
                        }
                    }
                    Err(err) => {
                        // Non-protocol stdout chatter (e.g. stray
                        // `println!` from a binary helper) is a real
                        // bug in sim-flow, but for the smoke test we
                        // want to flag it and keep going so a single
                        // stray line doesn't sink the whole run.
                        let _ = tx.send(EventFromOrch::Err(format!(
                            "parse stdout line `{trimmed}`: {err}"
                        )));
                    }
                }
            }
            Err(err) => {
                let _ = tx.send(EventFromOrch::Err(format!("read sim-flow stdout: {err}")));
                return;
            }
        }
    }
}

/// Test verdict accumulated by `drive`. Lets `run` decide the
/// process exit code AFTER teardown -- a run can complete cleanly
/// from the orchestrator's POV (Shutdown sent, child exited 0)
/// while still having raised Error diagnostics or failed
/// post-advance invariants. Without this the test silently passed
/// on a corrupted state.
#[derive(Debug, Default)]
pub struct TestVerdict {
    pub errors_seen: Vec<String>,
    pub validation_failures: Vec<String>,
}

fn drive(
    mut stdin: ChildStdin,
    rx: &Receiver<EventFromOrch>,
    agent: &dyn CliAgent,
    project_dir: &std::path::Path,
    capture: Option<&JsonlCapture>,
) -> std::result::Result<TestVerdict, String> {
    // 1. Send Hello to kick off the handshake.
    send_host_event(
        &mut stdin,
        &HostEvent::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host: HostInfo {
                name: "e2e-manual".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            capabilities: vec![
                "text".into(),
                "user-input".into(),
                "llm-request".into(),
                "tool-notifications".into(),
            ],
        },
        capture,
    )?;

    // State machine. `current_step` tracks the orchestrator's view
    // (we read it from `state.toml` after each Advance to stay
    // honest); `phase` tracks where we are in the (work, critique,
    // advance) triplet for the current step.
    let mut phase = ManualPhase::AwaitHelloAck;
    let mut verdict = TestVerdict::default();
    loop {
        let evt = match rx.recv() {
            Ok(e) => e,
            Err(_) => return Err("reader channel closed unexpectedly".into()),
        };
        match evt {
            EventFromOrch::Eof => {
                println!("e2e_manual: sim-flow stdout closed (session ended)");
                return Ok(verdict);
            }
            EventFromOrch::Err(err) => {
                eprintln!("e2e_manual: stdout parse error: {err}");
                continue;
            }
            EventFromOrch::Event(event) => {
                phase = handle_event(
                    phase,
                    *event,
                    &mut stdin,
                    agent,
                    project_dir,
                    &mut verdict,
                    capture,
                )?;
                if matches!(phase, ManualPhase::Done) {
                    return Ok(verdict);
                }
            }
        }
    }
}

// Fields hold step/kind for debugging visibility; the state machine
// transitions on SubSessionEnded / StateAdvanced events and rederives
// the next-step info there, so the embedded copies aren't read again.
#[allow(dead_code)]
#[derive(Debug, Clone)]
enum ManualPhase {
    AwaitHelloAck,
    /// Working on a step. Tracks which sub-session (work / critique)
    /// we last commanded so we know what to send next when
    /// `SubSessionEnded` lands.
    InStep {
        step: String,
        kind: SessionKindOut,
    },
    /// Sub-session for the current step finished cleanly; waiting
    /// for the orchestrator's emitted SubSessionEnded so we can
    /// dispatch the next command.
    AfterSubSession {
        step: String,
        kind: SessionKindOut,
    },
    /// Sent Advance; waiting for StateAdvanced to learn the next
    /// `current_step` (or that the flow is over).
    AwaitAdvance {
        from_step: String,
    },
    Done,
}

fn handle_event(
    phase: ManualPhase,
    event: Event,
    stdin: &mut ChildStdin,
    agent: &dyn CliAgent,
    project_dir: &std::path::Path,
    verdict: &mut TestVerdict,
    capture: Option<&JsonlCapture>,
) -> std::result::Result<ManualPhase, String> {
    match event {
        Event::HelloAck { session, .. } => {
            // Two HelloAck events arrive in a typical manual run:
            //   1. The initial handshake post-Hello (auto.rs's
            //      `perform_initial_handshake`). This is our
            //      "session is up, please drive" signal -- send
            //      RunStep here.
            //   2. Each sub-session also emits a synthetic
            //      HelloAck carrying its own step descriptor (so
            //      the dashboard's banner can update); these are
            //      banner-only and MUST NOT trigger another
            //      RunStep, or the orchestrator rejects it as "a
            //      sub-session is currently running" and the
            //      driver hangs.
            // We discriminate by the phase we're in: only the
            // first HelloAck is dispatched against
            // `AwaitHelloAck`.
            if !matches!(phase, ManualPhase::AwaitHelloAck) {
                println!(
                    "e2e_manual: HelloAck (step={}, kind={:?}) -- sub-session banner, ignoring",
                    session.step, session.kind
                );
                return Ok(phase);
            }
            let step = session.step.clone();
            println!(
                "e2e_manual: HelloAck (step={}, kind={:?}) -- sending RunStep work",
                step, session.kind
            );
            send_host_event(
                stdin,
                &HostEvent::RunStep {
                    step: step.clone(),
                    kind: SessionKindOut::Work,
                },
                capture,
            )?;
            Ok(ManualPhase::InStep {
                step,
                kind: SessionKindOut::Work,
            })
        }
        Event::SubSessionStarted { step, kind } => {
            println!("e2e_manual: SubSessionStarted {step}.{kind:?}");
            Ok(phase)
        }
        Event::SubSessionEnded {
            step,
            kind,
            outcome,
        } => {
            println!("e2e_manual: SubSessionEnded {step}.{kind:?} -> {outcome}");
            // While in `AwaitAdvance` the orchestrator may
            // auto-launch its own Work + Critique sub-sessions
            // (run_manual_advance's MoreMilestonesPending and
            // critique-blocker retry loops both call
            // run_subsession internally). Their SubSessionEnded
            // events are NOT signals for the host to drive --
            // they're internal bookkeeping, and dispatching another
            // RunStep here would land while the orchestrator is
            // still inside its loop and get rejected with
            // "ignored RunStep: a sub-session is currently
            // running". Stay in AwaitAdvance and wait for the
            // ultimate StateAdvanced (or an Error diagnostic).
            if matches!(phase, ManualPhase::AwaitAdvance { .. }) {
                Ok(phase)
            } else {
                Ok(ManualPhase::AfterSubSession { step, kind })
            }
        }
        Event::PhaseChanged { phase: phase_str } => {
            println!("e2e_manual:   phase -> {phase_str}");
            Ok(phase)
        }
        Event::AssistantText { text, final_chunk } => {
            // Render concisely so the smoke run is readable.
            if !text.is_empty() {
                print!("{text}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            if final_chunk {
                println!();
            }
            Ok(phase)
        }
        Event::ArtifactWritten { path, bytes } => {
            println!("e2e_manual:   wrote {path} ({bytes} bytes)");
            Ok(phase)
        }
        Event::ToolInvoked {
            name,
            args_summary,
            status,
            duration_ms,
        } => {
            println!("e2e_manual:   tool {name} ({args_summary}) -> {status} ({duration_ms} ms)");
            Ok(phase)
        }
        Event::BuildOutput {
            command, exit_code, ..
        } => {
            println!("e2e_manual:   `{command}` -> exit {exit_code}");
            Ok(phase)
        }
        Event::GateResult {
            step,
            clean,
            failures,
        } => {
            if clean {
                println!("e2e_manual:   gate {step}: clean");
            } else {
                println!("e2e_manual:   gate {step}: {} failure(s)", failures.len());
                for f in failures {
                    println!("e2e_manual:     - {}: {}", f.description, f.reason);
                }
            }
            Ok(phase)
        }
        Event::StateAdvanced { from, to } => {
            // Independent post-advance validation: re-runs the gate
            // and additionally checks artifact existence / size +
            // milestone-walk task counts. Catches gate BUGS that
            // would otherwise let the test silently chase a
            // corrupted state (the marker-substring mismatch class
            // of bug we hit on DM2cd). Failures don't stop the run
            // -- we collect them and fail the test at exit -- so
            // the user sees what else breaks downstream.
            let report = validate_step_advanced(project_dir, &from);
            report.print(&format!("post-advance:{from}"));
            verdict.validation_failures.extend(report.failures);
            match to {
                Some(next) => {
                    println!("e2e_manual: StateAdvanced {from} -> {next}; running next step");
                    send_host_event(
                        stdin,
                        &HostEvent::RunStep {
                            step: next.clone(),
                            kind: SessionKindOut::Work,
                        },
                        capture,
                    )?;
                    Ok(ManualPhase::InStep {
                        step: next,
                        kind: SessionKindOut::Work,
                    })
                }
                None => {
                    println!("e2e_manual: StateAdvanced {from} -> (end of flow); shutting down");
                    send_host_event(stdin, &HostEvent::Shutdown, capture)?;
                    Ok(ManualPhase::Done)
                }
            }
        }
        Event::RequestUserInput { .. } => {
            // In manual mode the orchestrator parks for user input
            // when a sub-session needs human guidance (e.g. an
            // LlmError or a runaway-loop guard fired). For the
            // smoke test we issue `/end-session` which the
            // orchestrator treats as a clean session terminator and
            // then re-parks at the top of the manual loop.
            eprintln!("e2e_manual: WARN: orchestrator requested user input; sending /end-session");
            send_host_event(
                stdin,
                &HostEvent::UserMessage {
                    text: "/end-session".into(),
                },
                capture,
            )?;
            Ok(phase)
        }
        Event::RequestLlmResponse {
            request_id,
            messages,
            ..
        } => {
            // Dispatch synchronously to the configured backend.
            // The orchestrator blocks waiting for chunks/end on the
            // same request_id, so doing this inline (rather than on
            // a thread) is safe and keeps the driver simple.
            let dispatch_start = Instant::now();
            println!(
                "e2e_manual:   LLM dispatch ({}, {} messages) ...",
                request_id,
                messages.len()
            );
            use std::io::Write;
            let _ = std::io::stdout().flush();
            match agent.dispatch(&messages) {
                Ok((text, metrics)) => {
                    println!(
                        "e2e_manual:   LLM dispatch ({}) -> {} bytes in {:.1}s",
                        request_id,
                        text.len(),
                        dispatch_start.elapsed().as_secs_f64()
                    );
                    tracing::info!(
                        target: "sim_flow::metrics",
                        event = "llm_call",
                        request_id = %request_id,
                        agent = %agent.name(),
                        tokens_in = ?metrics.tokens_in,
                        tokens_out = ?metrics.tokens_out,
                        wall_ms = metrics.wall_ms,
                        content_bytes = text.len(),
                    );
                    send_host_event(
                        stdin,
                        &HostEvent::LlmChunk {
                            request_id: request_id.clone(),
                            text,
                        },
                        capture,
                    )?;
                    send_host_event(
                        stdin,
                        &HostEvent::LlmEnd {
                            request_id,
                            stop_reason: Some("stop".into()),
                            tool_calls: Vec::new(),
                        },
                        capture,
                    )?;
                }
                Err(err) => {
                    println!(
                        "e2e_manual:   LLM dispatch ({}) FAILED after {:.1}s: {}",
                        request_id,
                        dispatch_start.elapsed().as_secs_f64(),
                        err,
                    );
                    send_host_event(
                        stdin,
                        &HostEvent::LlmError {
                            request_id,
                            kind: "dispatch".into(),
                            message: format!("{err}"),
                        },
                        capture,
                    )?;
                }
            }
            Ok(phase)
        }
        Event::Followup { label, action } => {
            println!("e2e_manual:   followup: {label} ({action})");
            Ok(phase)
        }
        Event::Diagnostic { level, message } => {
            println!("e2e_manual:   [{level:?}] {message}");
            // Record orchestrator-emitted Error diagnostics so the
            // process exits non-zero. Warning / Info are noise. The
            // run_manual_advance loop in the orchestrator now
            // handles transient gate dirties (critique blockers,
            // milestones-pending) internally with retry; if an
            // Error escapes to the host, it's a real terminal
            // failure -- max-retries exhausted, runaway loop, or
            // protocol-level bug -- and the test should fail.
            if matches!(level, DiagnosticLevel::Error) {
                verdict.errors_seen.push(message.clone());
            }
            Ok(phase)
        }
        Event::SessionEnd { reason, message } => {
            println!(
                "e2e_manual: SessionEnd reason={reason:?} message={}",
                message.as_deref().unwrap_or("(none)"),
            );
            Ok(phase)
        }
        Event::StepModeChanged { mode } => {
            println!("e2e_manual:   step-mode -> {mode:?}");
            let _ = StepMode::Auto;
            Ok(phase)
        }
    }
    .and_then(|new_phase| {
        // After a sub-session ends, dispatch the next command in the
        // manual sequence: work -> critique -> advance.
        match &new_phase {
            ManualPhase::AfterSubSession { step, kind } => match kind {
                SessionKindOut::Work => {
                    println!("e2e_manual: -> RunStep critique for {step}");
                    send_host_event(
                        stdin,
                        &HostEvent::RunStep {
                            step: step.clone(),
                            kind: SessionKindOut::Critique,
                        },
                        capture,
                    )?;
                    Ok(ManualPhase::InStep {
                        step: step.clone(),
                        kind: SessionKindOut::Critique,
                    })
                }
                SessionKindOut::Critique => {
                    println!("e2e_manual: -> Advance for {step}");
                    send_host_event(stdin, &HostEvent::Advance { step: step.clone() }, capture)?;
                    Ok(ManualPhase::AwaitAdvance {
                        from_step: step.clone(),
                    })
                }
            },
            _ => Ok(new_phase),
        }
    })
}

fn send_host_event(
    stdin: &mut ChildStdin,
    event: &HostEvent,
    capture: Option<&JsonlCapture>,
) -> std::result::Result<(), String> {
    if let Some(cap) = capture {
        cap.record_in(event);
    }
    let line =
        serde_json::to_string(event).map_err(|err| format!("serialize host event: {err}"))?;
    stdin
        .write_all(line.as_bytes())
        .map_err(|err| format!("write host event: {err}"))?;
    stdin
        .write_all(b"\n")
        .map_err(|err| format!("write newline: {err}"))?;
    stdin.flush().map_err(|err| format!("flush stdin: {err}"))?;
    Ok(())
}

fn summarize_state(project_dir: &std::path::Path) -> std::result::Result<(), String> {
    let state_path = project_dir.join(".sim-flow/state.toml");
    if let Ok(body) = std::fs::read_to_string(&state_path) {
        println!("\n--- state.toml ---");
        print!("{body}");
        println!("--- end state.toml ---");
    }
    Ok(())
}

// Currently unused; quiets the unused-import warning when the agent
// happens to not be invoked (e.g. early-fail paths).
#[allow(dead_code)]
fn _unused(_: &LlmMessage, _: &Value, _: &Child) {}
