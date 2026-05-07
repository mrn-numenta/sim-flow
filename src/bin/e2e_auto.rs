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
use sim_flow::session::{AutoOptions, ingest_spec_file, run_auto};

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
    max_llm_requests: u32,
    max_identical_responses: u32,
    no_preamble: bool,
    /// When true (default for openai-compat backend with an
    /// explicit base_url), ping `<base_url>/models` before
    /// launching `run_auto`. Catches "vLLM is dead" up front
    /// rather than after a 30-minute warm-up + first dispatch
    /// failure. Disable with `--no-healthcheck`.
    healthcheck: bool,
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
        let mut max_critique_iters = 3u32;
        let mut max_llm_requests = 500u32;
        let mut max_identical_responses = 3u32;
        let mut no_preamble = true;
        let mut healthcheck = true;
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
                "--help" | "-h" => {
                    println!(
                        "usage: e2e_auto --project-dir <P> --foundation-root <F> \
                         --backend {{openai-compat|ollama|claude}} [--model <M>] \
                         [--base-url <URL>] [--spec <PATH>] \
                         [--max-auto-iters <N>] [--max-critique-iters <N>] \
                         [--max-llm-requests <N>] [--max-identical-responses <N>] \
                         [--preamble | --no-preamble] \
                         [--healthcheck | --no-healthcheck]"
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
            max_llm_requests,
            max_identical_responses,
            no_preamble,
            healthcheck,
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
        "e2e_auto: caps              = max_auto_iters={} max_critique_iters={} max_llm_requests={} max_identical_responses={} no_preamble={}",
        args.max_auto_iters,
        args.max_critique_iters,
        args.max_llm_requests,
        args.max_identical_responses,
        args.no_preamble,
    );
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
        )),
        Backend::Ollama => Box::new(OllamaAgent::new(args.base_url.clone(), args.model.clone())),
        Backend::Claude => Box::new(ClaudeAgent::new(args.model.clone())),
    };

    // Empty stdin: auto mode shouldn't need user input. Any
    // RequestUserInput from the orchestrator (e.g. on LlmError) will
    // surface as a stuck read; the runaway-loop guards bound the
    // damage either way.
    let stdin = BufReader::new(Cursor::new(Vec::<u8>::new()));
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let mut host = TerminalHost::new(BoxedAgent(agent), stdin, stdout, stderr);

    let opts = AutoOptions {
        project_dir: args.project_dir.clone(),
        foundation_root: args.foundation_root.clone(),
        llm_backend: backend_label.to_string(),
        llm_model: args.model.clone(),
        llm_base_url: args.base_url.clone(),
        max_auto_iters: args.max_auto_iters,
        max_critique_iters: args.max_critique_iters,
        dm0_interactive: false,
        max_llm_requests: args.max_llm_requests,
        max_identical_responses: args.max_identical_responses,
        step_mode: StepMode::Auto,
        no_preamble: args.no_preamble,
    };

    println!("e2e_auto: launching run_auto...\n");
    let started = Instant::now();
    run_auto(opts, &mut host).map_err(|err| format!("run_auto error: {err}"))?;
    let elapsed = started.elapsed();
    println!(
        "\ne2e_auto: run_auto returned after {:.1}s",
        elapsed.as_secs_f64()
    );

    summarize_state(&args.project_dir)?;
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
