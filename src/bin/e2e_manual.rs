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
use sim_flow::session::agent::{ClaudeAgent, CliAgent, OllamaAgent, OpenAiCompatAgent};
use sim_flow::session::ingest_spec_file;
use sim_flow::session::protocol::{
    Event, HostEvent, HostInfo, LlmMessage, PROTOCOL_VERSION, SessionKindOut, StepMode,
};

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
    max_llm_requests: u32,
}

impl Args {
    fn parse(argv: Vec<String>) -> std::result::Result<Self, String> {
        let mut project_dir: Option<PathBuf> = None;
        let mut foundation_root: Option<PathBuf> = None;
        let mut spec: Option<PathBuf> = None;
        let mut sim_flow_bin: Option<PathBuf> = None;
        let mut backend: Option<String> = None;
        let mut model: Option<String> = None;
        let mut max_auto_iters = 3u32;
        let mut max_critique_iters = 3u32;
        let mut max_llm_requests = 50u32;
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
                "--max-llm-requests" => {
                    max_llm_requests = iter
                        .next()
                        .ok_or_else(|| "--max-llm-requests needs a value".to_string())?
                        .parse()
                        .map_err(|err| format!("--max-llm-requests: {err}"))?
                }
                "--help" | "-h" => {
                    println!(
                        "usage: e2e_manual --project-dir <P> --foundation-root <F> \
                         --backend {{openai-compat|ollama|claude}} [--model <M>] \
                         [--spec <PATH>] [--sim-flow-bin <PATH>] \
                         [--max-auto-iters <N>] [--max-critique-iters <N>] \
                         [--max-llm-requests <N>]"
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
        Ok(Self {
            project_dir,
            foundation_root,
            spec,
            sim_flow_bin,
            backend,
            model,
            max_auto_iters,
            max_critique_iters,
            max_llm_requests,
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
        .arg("--max-llm-requests")
        .arg(args.max_llm_requests.to_string());
    if let Some(model) = &args.model {
        cmd.arg("--llm-model").arg(model);
    }
    if let Some(spec) = &args.spec {
        cmd.arg("--spec").arg(spec);
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

    // Reader thread: parse one Event per stdout line and forward on
    // a channel so the main loop can do a straight `recv()` even
    // though stdin/stdout are independent file descriptors.
    let (event_tx, event_rx) = channel::<EventFromOrch>();
    let reader_handle = thread::Builder::new()
        .name("e2e-manual-reader".into())
        .spawn(move || reader_loop(stdout, event_tx))
        .map_err(|err| format!("spawn reader thread: {err}"))?;

    let agent: Box<dyn CliAgent> = match args.backend.as_str() {
        "openai-compat" | "openai_compat" | "openai" => {
            Box::new(OpenAiCompatAgent::new(None, args.model.clone()))
        }
        "ollama" => Box::new(OllamaAgent::new(None, args.model.clone())),
        "claude" | "claude-cli" => Box::new(ClaudeAgent::new(args.model.clone())),
        _ => unreachable!("validated in Args::parse"),
    };

    let outcome = drive(stdin, &event_rx, agent.as_ref());

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

    outcome
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

fn reader_loop(stdout: ChildStdout, tx: Sender<EventFromOrch>) {
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

fn drive(
    mut stdin: ChildStdin,
    rx: &Receiver<EventFromOrch>,
    agent: &dyn CliAgent,
) -> std::result::Result<(), String> {
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
    )?;

    // State machine. `current_step` tracks the orchestrator's view
    // (we read it from `state.toml` after each Advance to stay
    // honest); `phase` tracks where we are in the (work, critique,
    // advance) triplet for the current step.
    let mut phase = ManualPhase::AwaitHelloAck;
    loop {
        let evt = match rx.recv() {
            Ok(e) => e,
            Err(_) => return Err("reader channel closed unexpectedly".into()),
        };
        match evt {
            EventFromOrch::Eof => {
                println!("e2e_manual: sim-flow stdout closed (session ended)");
                return Ok(());
            }
            EventFromOrch::Err(err) => {
                eprintln!("e2e_manual: stdout parse error: {err}");
                continue;
            }
            EventFromOrch::Event(event) => {
                phase = handle_event(phase, *event, &mut stdin, agent)?;
                if matches!(phase, ManualPhase::Done) {
                    return Ok(());
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
            Ok(ManualPhase::AfterSubSession { step, kind })
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
        Event::StateAdvanced { from, to } => match to {
            Some(next) => {
                println!("e2e_manual: StateAdvanced {from} -> {next}; running next step");
                send_host_event(
                    stdin,
                    &HostEvent::RunStep {
                        step: next.clone(),
                        kind: SessionKindOut::Work,
                    },
                )?;
                Ok(ManualPhase::InStep {
                    step: next,
                    kind: SessionKindOut::Work,
                })
            }
            None => {
                println!("e2e_manual: StateAdvanced {from} -> (end of flow); shutting down");
                send_host_event(stdin, &HostEvent::Shutdown)?;
                Ok(ManualPhase::Done)
            }
        },
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
                    )?;
                    send_host_event(
                        stdin,
                        &HostEvent::LlmEnd {
                            request_id,
                            stop_reason: Some("stop".into()),
                        },
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
                    )?;
                    Ok(ManualPhase::InStep {
                        step: step.clone(),
                        kind: SessionKindOut::Critique,
                    })
                }
                SessionKindOut::Critique => {
                    println!("e2e_manual: -> Advance for {step}");
                    send_host_event(stdin, &HostEvent::Advance { step: step.clone() })?;
                    Ok(ManualPhase::AwaitAdvance {
                        from_step: step.clone(),
                    })
                }
            },
            _ => Ok(new_phase),
        }
    })
}

fn send_host_event(stdin: &mut ChildStdin, event: &HostEvent) -> std::result::Result<(), String> {
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
