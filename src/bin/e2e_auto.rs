//! End-to-end auto-mode runner against an arbitrary `CliAgent`
//! backend. Adapts `dm_flow_smoke.rs` (claude-only) for LM Studio
//! (and future OpenAI-compat backends) so we can iterate on the
//! flow's mechanics against a local model without burning API
//! credits or driving the JSONL host protocol from the outside.
//!
//! Wires `<backend>Agent` -> `TerminalHost` -> `run_auto`. The
//! TerminalHost synthesizes the `Hello` handshake in-process, so we
//! don't need an external host like the dashboard / `sim-flow auto`
//! does over JSONL.
//!
//! Usage:
//!
//! ```sh
//! cargo run -p sim-flow --bin e2e_auto -- \
//!     --project-dir /Users/mneilly/nta/sim-models/users/mneilly/rgb_toy \
//!     --foundation-root /Users/mneilly/nta/sim-foundation \
//!     --backend openai-compat \
//!     --base-url http://localhost:8012/v1 \
//!     --model qwen3.6
//! ```
//!
//! The project dir must already be initialized (run `sim-flow new
//! model` first; the wrapper script `scripts/e2e-rgb-auto.sh` does
//! this end-to-end).
//!
//! Exit codes:
//!   0 - run_auto returned Ok (gates may still have failed; check
//!       the printed state.toml + artifact summary)
//!   1 - run_auto returned an error (host closed, LLM dispatch
//!       failed, runaway-loop guard fired, ...)
//!   2 - bad arguments / setup

use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::time::Instant;

use sim_flow::session::agent::{
    ClaudeAgent, CliAgent, LlmCallMetrics, OllamaAgent, OpenAiCompatAgent,
};
use sim_flow::session::host::TerminalHost;
use sim_flow::session::protocol::{LlmMessage, StepMode};
use sim_flow::session::{
    AutoOptions, CaptureHost, EventTap, JsonlCapture, TappedHost, WatchRegistration,
    ingest_spec_file, run_auto,
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
        eprintln!("\ne2e_auto: FAILED: {err}");
        std::process::exit(1);
    }
}

struct Args {
    project_dir: PathBuf,
    foundation_root: PathBuf,
    spec: Option<PathBuf>,
    backend: Backend,
    model: Option<String>,
    /// OpenAI-compatible endpoint base URL (overrides the agent's
    /// default — `http://localhost:1234/v1` for LM Studio,
    /// `http://localhost:11434/v1` for Ollama). Lets us point at any
    /// vLLM / TGI / llama.cpp server that speaks the same wire format.
    base_url: Option<String>,
    max_auto_iters: u32,
    max_critique_iters: u32,
    max_critique_no_progress_iters: u32,
    max_llm_requests: u32,
    max_identical_responses: u32,
    no_preamble: bool,
    /// When true (default for openai-compat backend with an
    /// explicit base_url), ping `<base_url>/models` before
    /// launching `run_auto`. Catches "vLLM is dead" up front
    /// rather than after a 30-minute warm-up + first dispatch
    /// failure. Disable with `--no-healthcheck`.
    healthcheck: bool,
    /// Optional Unix socket path for the read-only event tap.
    /// When set (or auto-generated when neither
    /// `--watch-socket` nor `--no-watch-socket` is passed), the
    /// VS Code dashboard's "Attach to Running Watcher" picker
    /// can list this run and observe events without touching
    /// e2e_auto's in-process command path.
    watch_socket: Option<PathBuf>,
    /// Optional JSONL capture file. When set, every protocol
    /// event (orchestrator -> host) AND every host event
    /// (host -> orchestrator) is teed to this path as a JSONL
    /// stream. Format: `{"ts": <unix_ms>, "dir": "out"|"in",
    /// "event": {...}}`. Used by the model-robustness study
    /// (see docs/brainstorming/model-robustness-study.md) to
    /// build per-model anomaly catalogs and a replay corpus.
    /// The capture is purely observational; the orchestrator's
    /// behavior is identical whether this flag is set or not.
    capture_jsonl: Option<PathBuf>,
}

#[derive(Clone, Copy)]
enum Backend {
    OpenAiCompat,
    Ollama,
    Claude,
}

impl Args {
    fn parse(argv: Vec<String>) -> std::result::Result<Self, String> {
        let mut project_dir: Option<PathBuf> = None;
        let mut foundation_root: Option<PathBuf> = None;
        let mut spec: Option<PathBuf> = None;
        let mut backend_str: Option<String> = None;
        let mut model: Option<String> = None;
        let mut base_url: Option<String> = None;
        let mut max_auto_iters = 3u32;
        let mut max_critique_iters = 10u32;
        let mut max_critique_no_progress_iters = 3u32;
        let mut max_llm_requests = 500u32;
        let mut max_identical_responses = 3u32;
        let mut no_preamble = true;
        let mut healthcheck = true;
        let mut watch_socket: Option<PathBuf> = None;
        let mut watch_disabled = false;
        let mut capture_jsonl: Option<PathBuf> = None;
        let mut iter = argv.into_iter().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--project-dir" => project_dir = iter.next().map(PathBuf::from),
                "--foundation-root" => foundation_root = iter.next().map(PathBuf::from),
                "--spec" => spec = iter.next().map(PathBuf::from),
                "--backend" => backend_str = iter.next(),
                "--model" => model = iter.next(),
                "--base-url" => base_url = iter.next(),
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
                "--max-identical-responses" => {
                    max_identical_responses = iter
                        .next()
                        .ok_or_else(|| "--max-identical-responses needs a value".to_string())?
                        .parse()
                        .map_err(|err| format!("--max-identical-responses: {err}"))?
                }
                "--no-preamble" => no_preamble = true,
                "--preamble" => no_preamble = false,
                "--no-healthcheck" => healthcheck = false,
                "--healthcheck" => healthcheck = true,
                "--watch-socket" => watch_socket = iter.next().map(PathBuf::from),
                "--no-watch-socket" => watch_disabled = true,
                "--capture-jsonl" => capture_jsonl = iter.next().map(PathBuf::from),
                "--help" | "-h" => {
                    println!(
                        "usage: e2e_auto --project-dir <P> --foundation-root <F> \
                         --backend {{openai-compat|ollama|claude}} [--model <M>] \
                         [--base-url <URL>] [--spec <PATH>] \
                         [--max-auto-iters <N>] [--max-critique-iters <N>] \
                         [--max-critique-no-progress-iters <N>] \
                         [--max-llm-requests <N>] [--max-identical-responses <N>] \
                         [--preamble | --no-preamble] \
                         [--healthcheck | --no-healthcheck] \
                         [--watch-socket <PATH>] [--no-watch-socket] \
                         [--capture-jsonl <PATH>]\n\
                         \n\
                         By default a `--watch-socket` path is auto-generated under the \
                         system temp dir so the VS Code dashboard's `sim-flow: Attach \
                         to Running Watcher` picker can attach as a read-only viewer. \
                         Pass `--no-watch-socket` to disable, or `--watch-socket <PATH>` \
                         to choose your own path.\n\
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
        let backend = match backend_str.as_deref() {
            Some("openai-compat") | Some("openai_compat") | Some("openai") => Backend::OpenAiCompat,
            Some("ollama") => Backend::Ollama,
            Some("claude") | Some("claude-cli") => Backend::Claude,
            Some(other) => return Err(format!("unknown backend: {other}")),
            None => return Err("--backend is required".to_string()),
        };
        let watch_socket = if watch_disabled {
            None
        } else {
            Some(watch_socket.unwrap_or_else(|| {
                std::env::temp_dir().join(format!("sim-flow-e2e-auto-{}.sock", std::process::id()))
            }))
        };
        Ok(Self {
            project_dir: project_dir.ok_or_else(|| "--project-dir is required".to_string())?,
            foundation_root: foundation_root
                .ok_or_else(|| "--foundation-root is required".to_string())?,
            spec,
            backend,
            model,
            base_url,
            max_auto_iters,
            max_critique_iters,
            max_critique_no_progress_iters,
            max_llm_requests,
            max_identical_responses,
            no_preamble,
            healthcheck,
            watch_socket,
            capture_jsonl,
        })
    }
}

/// Probe `<base_url>/models` to confirm the LLM server is alive
/// before launching the multi-hour auto run. Catches "vLLM is
/// dead" / "wrong port" / "wrong base_url path" failures up
/// front instead of 30 minutes in. 5-second per-attempt timeout;
/// 3 attempts with 2-second backoff so a momentarily-laggy server
/// doesn't fail the run. Returns `Ok` on success, `Err(message)`
/// on confirmed failure.
fn vllm_healthcheck(base_url: &str) -> std::result::Result<(), String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut last_err: Option<String> = None;
    for attempt in 1..=3 {
        match ureq::get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .call()
        {
            Ok(resp) if resp.status() == 200 => {
                return Ok(());
            }
            Ok(resp) => {
                last_err = Some(format!(
                    "GET {url} returned status {}; expected 200",
                    resp.status()
                ));
            }
            Err(err) => {
                last_err = Some(format!("GET {url} failed: {err}"));
            }
        }
        if attempt < 3 {
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }
    Err(last_err.unwrap_or_else(|| "healthcheck failed (no error captured)".to_string()))
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

    let backend_label = match args.backend {
        Backend::OpenAiCompat => "openai-compat",
        Backend::Ollama => "ollama",
        Backend::Claude => "claude",
    };
    println!(
        "e2e_auto: project_dir       = {}",
        args.project_dir.display()
    );
    println!(
        "e2e_auto: foundation_root   = {}",
        args.foundation_root.display()
    );
    if let Some(spec) = &args.spec {
        println!("e2e_auto: spec              = {}", spec.display());
    }
    println!("e2e_auto: backend           = {}", backend_label);
    println!(
        "e2e_auto: model             = {}",
        args.model.as_deref().unwrap_or("(default)")
    );
    println!(
        "e2e_auto: caps              = max_auto_iters={} max_critique_iters={} max_critique_no_progress_iters={} max_llm_requests={} max_identical_responses={} no_preamble={}",
        args.max_auto_iters,
        args.max_critique_iters,
        args.max_critique_no_progress_iters,
        args.max_llm_requests,
        args.max_identical_responses,
        args.no_preamble,
    );
    // Mirror e2e_manual: smoke projects in tempdirs would otherwise see
    // `library_root = None` (the orchestrator's ancestor walk finds
    // nothing above /tmp/), and every `lib:examples/...` read errors.
    // Default `SIM_FLOW_LIBRARY_ROOT` to the sibling `sim-models/` of
    // the foundation-root when the caller hasn't already set it.
    if std::env::var_os("SIM_FLOW_LIBRARY_ROOT").is_none() {
        if let Some(parent) = args.foundation_root.parent() {
            let candidate = parent.join("sim-models");
            if candidate.join("docs").join("modeling-guide").is_dir()
                && candidate.join("examples").is_dir()
            {
                println!(
                    "e2e_auto: library_root      = {} (auto)",
                    candidate.display()
                );
                // SAFETY: set before we spawn anything that reads it.
                unsafe {
                    std::env::set_var("SIM_FLOW_LIBRARY_ROOT", &candidate);
                }
            }
        }
    } else {
        println!(
            "e2e_auto: library_root      = {} (env)",
            std::env::var("SIM_FLOW_LIBRARY_ROOT").unwrap_or_default()
        );
    }
    println!();

    // Spec ingestion: chunk the source spec into `.sim-flow/spec-pages/`
    // and emit `source-spec-toc.md` so the orchestrator's system stack
    // (`build_spec_toc_message` / `build_session_inputs`) sees pages to
    // reference. `sim-flow auto --spec ...` does this via
    // `ensure_source_spec_ingested`; we run the underlying helper
    // directly since this binary bypasses the CLI's auto subcommand.
    if let Some(spec) = &args.spec {
        match ingest_spec_file(spec, &args.project_dir) {
            Ok(summary) => {
                println!(
                    "e2e_auto: ingested spec `{}` -> {} page(s)",
                    spec.display(),
                    summary.page_count,
                );
            }
            Err(err) => return Err(format!("ingest_spec_file({}): {err}", spec.display())),
        }
    }

    // Healthcheck the LLM server BEFORE launching the auto run.
    // Skip for Claude backend (no local server); also skip when
    // `--no-healthcheck` was explicitly set. For OpenAI-compat /
    // Ollama with a base_url, ping `<base_url>/models` to confirm
    // the server is alive.
    if args.healthcheck && !matches!(args.backend, Backend::Claude) {
        let base_url_for_check = args.base_url.clone().unwrap_or_else(|| match args.backend {
            Backend::Ollama => "http://localhost:11434/v1".to_string(),
            _ => "http://localhost:1234/v1".to_string(),
        });
        match vllm_healthcheck(&base_url_for_check) {
            Ok(()) => {
                println!("e2e_auto: healthcheck       = {base_url_for_check}/models -> 200 OK");
            }
            Err(detail) => {
                return Err(format!(
                    "LLM healthcheck failed before launch: {detail}\n\
                     Check that the server is running and the base URL is correct, \
                     or pass --no-healthcheck to skip this check.",
                ));
            }
        }
    }

    let agent: Box<dyn CliAgent> = match args.backend {
        Backend::OpenAiCompat => Box::new(OpenAiCompatAgent::new(
            args.base_url.clone(),
            args.model.clone(),
            None,
            None,
        )),
        Backend::Ollama => Box::new(OllamaAgent::new(
            args.base_url.clone(),
            args.model.clone(),
            None,
            None,
        )),
        Backend::Claude => Box::new(ClaudeAgent::new(args.model.clone(), None, None)),
    };

    // Empty stdin: auto mode shouldn't need user input. Any
    // RequestUserInput from the orchestrator (e.g. on LlmError) will
    // surface as a stuck read; the runaway-loop guards bound the
    // damage either way.
    let stdin = BufReader::new(Cursor::new(Vec::<u8>::new()));
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let host = TerminalHost::new(BoxedAgent(agent), stdin, stdout, stderr);

    // Optional read-only event broadcast. When `--watch-socket` is
    // set (default) the dashboard's "Attach to Running Watcher"
    // picker can list this run and observe events live; the tap
    // unbinds + de-registers when the EventTap is dropped at the
    // end of `run`.
    let watch_tap = match &args.watch_socket {
        Some(path) => {
            let registration = WatchRegistration {
                pid: std::process::id(),
                socket_path: path.clone(),
                project_dir: args.project_dir.clone(),
                started_at: now_iso8601(),
                llm_backend: backend_label.to_string(),
                llm_model: args.model.clone(),
            };
            match EventTap::bind_with_registration(path.clone(), registration) {
                Ok(tap) => {
                    eprintln!(
                        "e2e_auto: watch socket = {} (attach via VS Code: `sim-flow: Attach to Running Watcher`)",
                        path.display()
                    );
                    Some(tap)
                }
                Err(err) => {
                    eprintln!(
                        "e2e_auto: WARN: failed to bind --watch-socket {}: {err}",
                        path.display()
                    );
                    None
                }
            }
        }
        None => None,
    };

    let opts = AutoOptions {
        project_dir: args.project_dir.clone(),
        foundation_root: args.foundation_root.clone(),
        llm_backend: backend_label.to_string(),
        llm_model: args.model.clone(),
        llm_model_family_id: None,
        llm_runtime_profile_id: None,
        llm_debug_adaptation: false,
        llm_base_url: args.base_url.clone(),
        max_auto_iters: args.max_auto_iters,
        max_critique_iters: args.max_critique_iters,
        max_critique_no_progress_iters: args.max_critique_no_progress_iters,
        dm0_interactive: false,
        max_llm_requests: args.max_llm_requests,
        max_identical_responses: args.max_identical_responses,
        step_mode: StepMode::Auto,
        no_preamble: args.no_preamble,
    };

    // Optional JSONL capture for the model-robustness study. The
    // wrapper goes OUTERMOST (closest to the orchestrator) so it
    // sees every event before TappedHost broadcasts it AND every
    // host event after TerminalHost reads from stdin.
    let capture = match &args.capture_jsonl {
        Some(path) => {
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)
                    .map_err(|err| format!("create capture dir {}: {err}", parent.display()))?;
            }
            let file = std::fs::File::create(path)
                .map_err(|err| format!("create capture file {}: {err}", path.display()))?;
            let writer = std::io::BufWriter::new(file);
            let cap = JsonlCapture::new(writer);
            // Header line: lets the analyzer interpret a capture
            // standalone without needing to consult external config.
            cap.record_meta(serde_json::json!({
                "kind": "run-start",
                "tool": "e2e_auto",
                "backend": backend_label,
                "model": args.model.clone(),
                "base_url": args.base_url.clone(),
                "project_dir": args.project_dir.display().to_string(),
                "foundation_root": args.foundation_root.display().to_string(),
                "spec": args.spec.as_ref().map(|p| p.display().to_string()),
                "max_auto_iters": args.max_auto_iters,
                "max_critique_iters": args.max_critique_iters,
                "max_critique_no_progress_iters": args.max_critique_no_progress_iters,
                "max_llm_requests": args.max_llm_requests,
                "max_identical_responses": args.max_identical_responses,
                "no_preamble": args.no_preamble,
                "pid": std::process::id(),
                "started_at_unix": now_iso8601(),
            }));
            eprintln!("e2e_auto: capture-jsonl = {}", path.display());
            Some(cap)
        }
        None => None,
    };

    println!("e2e_auto: launching run_auto...\n");
    let started = Instant::now();
    let result = match (watch_tap, capture.clone()) {
        (Some(tap), Some(cap)) => {
            let mut host = CaptureHost::new(TappedHost::new(host, tap), cap);
            run_auto(opts, &mut host)
        }
        (Some(tap), None) => {
            let mut host = TappedHost::new(host, tap);
            run_auto(opts, &mut host)
        }
        (None, Some(cap)) => {
            let mut host = CaptureHost::new(host, cap);
            run_auto(opts, &mut host)
        }
        (None, None) => {
            let mut host = host;
            run_auto(opts, &mut host)
        }
    };
    if let Some(cap) = &capture {
        cap.record_meta(serde_json::json!({
            "kind": "run-end",
            "ok": result.is_ok(),
            "error": result.as_ref().err().map(|e| format!("{e}")),
            "wall_ms": started.elapsed().as_millis() as u64,
        }));
    }
    result.map_err(|err| format!("run_auto error: {err}"))?;
    let elapsed = started.elapsed();
    println!(
        "\ne2e_auto: run_auto returned after {:.1}s",
        elapsed.as_secs_f64()
    );

    summarize_state(&args.project_dir)?;

    // Independent post-run validation: re-runs every passed step's
    // gate AND checks artifact-existence + milestone-walk task
    // counts. Catches the gate-bug class of failure (where the
    // orchestrator advanced on un-finished collateral and the
    // existing summarize_state byte-only presence check waved it
    // through).
    let report = sim_flow::test_validation::validate_full_state(&args.project_dir);
    report.print("e2e_auto: full-state");
    if !report.is_clean() {
        return Err(format!(
            "TEST FAILED: {} post-run validation failure(s); see [VALIDATE-FAIL] lines above",
            report.failures.len()
        ));
    }

    // run_auto is supposed to drive the entire flow to its terminal
    // step. If state.toml's `current_step` isn't the registered
    // last-step (or the flow didn't fully advance), the run was
    // incomplete -- fail loudly so a partially-finished run doesn't
    // get rubber-stamped. The terminal step is whichever step the
    // registry's `order_for` lists last.
    {
        let dot = args.project_dir.join(".sim-flow");
        let state = sim_flow::__internal::state::State::load(&dot)
            .map_err(|err| format!("load state.toml: {err}"))?;
        let registry = sim_flow::__internal::steps::registry_for(state.flow);
        let order = registry.order_for(state.flow);
        if let Some(last) = order.last() {
            let last_passed = state.gates.get(*last).map(|g| g.passed).unwrap_or(false);
            if !last_passed {
                return Err(format!(
                    "TEST FAILED: terminal step `{last}` did not pass; current_step=`{}`. \
                     run_auto returned without driving the flow to completion.",
                    state.current_step
                ));
            }
        }
    }

    println!("e2e_auto: TEST PASSED (full state validated, flow advanced to terminal step)");
    Ok(())
}

fn summarize_state(project_dir: &Path) -> std::result::Result<(), String> {
    let state_path = project_dir.join(".sim-flow/state.toml");
    let body =
        std::fs::read_to_string(&state_path).map_err(|err| format!("read state.toml: {err}"))?;
    println!("\n--- state.toml ---");
    print!("{body}");
    println!("--- end state.toml ---");

    let checks = [
        ("docs/spec.md", project_dir.join("docs/spec.md")),
        ("docs/targets.md", project_dir.join("docs/targets.md")),
        ("docs/testbench.md", project_dir.join("docs/testbench.md")),
        (
            "docs/analysis/decomposition.md",
            project_dir.join("docs/analysis/decomposition.md"),
        ),
        (
            "docs/analysis/data-movement.md",
            project_dir.join("docs/analysis/data-movement.md"),
        ),
        (
            "docs/analysis/pipeline-mapping.md",
            project_dir.join("docs/analysis/pipeline-mapping.md"),
        ),
        (
            "docs/impl-plan/plan.md",
            project_dir.join("docs/impl-plan/plan.md"),
        ),
        ("src/lib.rs", project_dir.join("src/lib.rs")),
        ("Cargo.toml", project_dir.join("Cargo.toml")),
        (
            "docs/test-plan/test-plan.md",
            project_dir.join("docs/test-plan/test-plan.md"),
        ),
        (
            "docs/perf-plan/perf-plan.md",
            project_dir.join("docs/perf-plan/perf-plan.md"),
        ),
    ];
    println!("\n--- artifact presence ---");
    for (label, path) in &checks {
        let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let status = if bytes > 0 { "OK" } else { "MISSING" };
        println!("  {status:>7}  {label}  ({bytes} bytes)");
    }
    Ok(())
}

/// Wrap a `Box<dyn CliAgent>` so it satisfies `TerminalHost`'s
/// `A: CliAgent` generic bound.
struct BoxedAgent(Box<dyn CliAgent>);

impl CliAgent for BoxedAgent {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn dispatch(&self, messages: &[LlmMessage]) -> sim_flow::Result<(String, LlmCallMetrics)> {
        self.0.dispatch(messages)
    }
}

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => format!("{}", d.as_secs()),
        Err(_) => "0".to_string(),
    }
}
