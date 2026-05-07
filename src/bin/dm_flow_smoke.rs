//! End-to-end smoke test for the Direct Modeling flow against a real
//! `claude` CLI. Walks every DM step (DM0 -> DM1 -> ... -> DM4) on a
//! small fixture spec and reports per-step pass/fail so we can see
//! which stages still have bugs without burning hours of manual
//! click-through.
//!
//! Bypasses the new interactive-PTY driver -- we want the
//! one-shot `ClaudeAgent` (`claude -p`) path here so the test runs
//! unattended. Each step still goes through the full orchestrator
//! turn loop: build messages, dispatch via the agent, parse
//! artifacts, run gate, advance.
//!
//! Usage:
//!
//! ```sh
//! cargo run -p sim-flow --bin dm_flow_smoke -- \
//!     --project-dir /tmp/sim-flow-smoke-$(date +%s) \
//!     --foundation-root /Users/mneilly/nta/sim-foundation \
//!     --library-root /Users/mneilly/nta/sim-models \
//!     --model sonnet
//! ```
//!
//! The library root is required because DM2c reads modeling-guide /
//! examples / library content via `lib:` paths. Pass the local
//! `sim-models` checkout. If your fixtures don't trigger `lib:`
//! lookups, anything readable will do.
//!
//! Exit codes:
//!   0 - all steps reached and gates passed cleanly
//!   1 - flow ran but stopped early (gate failure, run-time error)
//!   2 - bad arguments / setup (no claude on PATH, project init failed, etc.)

use std::fs;
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use sim_flow::session::agent::{CliAgent, LlmCallMetrics};
use sim_flow::session::host::TerminalHost;
use sim_flow::session::protocol::LlmMessage;
use sim_flow::session::{AutoOptions, ClaudeAgent, run_auto};

const FIXTURE_SPEC: &str = include_str!("dm_flow_smoke_spec.md");

fn main() {
    let args: Args = match Args::parse(std::env::args().collect()) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };

    if let Err(err) = check_claude_available() {
        eprintln!("dm_flow_smoke: claude CLI check failed: {err}");
        std::process::exit(2);
    }

    let result = run(&args);
    if let Err(err) = result {
        eprintln!("\ndm_flow_smoke: FAILED: {err}");
        std::process::exit(1);
    }
}

struct Args {
    project_dir: PathBuf,
    foundation_root: PathBuf,
    /// Used as the `lib:` root inside the orchestrator's tool catalog
    /// + auto-detected library lookups. The crate's `detect_library_root`
    ///   walks up from the project dir; we set the project dir under
    ///   the library root so detection succeeds.
    library_root: Option<PathBuf>,
    model: Option<String>,
}

impl Args {
    fn parse(argv: Vec<String>) -> std::result::Result<Self, String> {
        let mut project_dir: Option<PathBuf> = None;
        let mut foundation_root: Option<PathBuf> = None;
        let mut library_root: Option<PathBuf> = None;
        let mut model: Option<String> = None;
        let mut iter = argv.into_iter().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--project-dir" => project_dir = iter.next().map(PathBuf::from),
                "--foundation-root" => foundation_root = iter.next().map(PathBuf::from),
                "--library-root" => library_root = iter.next().map(PathBuf::from),
                "--model" => model = iter.next(),
                "--help" | "-h" => {
                    println!(
                        "usage: dm_flow_smoke --project-dir <P> --foundation-root <F> [--library-root <L>] [--model <M>]"
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown flag: {other}")),
            }
        }
        Ok(Self {
            project_dir: project_dir.ok_or_else(|| "--project-dir is required".to_string())?,
            foundation_root: foundation_root
                .ok_or_else(|| "--foundation-root is required".to_string())?,
            library_root,
            model,
        })
    }
}

fn check_claude_available() -> std::result::Result<(), String> {
    Command::new("claude")
        .arg("--version")
        .output()
        .map_err(|err| format!("`claude --version` failed: {err}; install Claude Code CLI"))?;
    Ok(())
}

fn run(args: &Args) -> std::result::Result<(), String> {
    let started_total = Instant::now();
    println!("dm_flow_smoke: project_dir={}", args.project_dir.display());
    println!(
        "            : foundation_root={}",
        args.foundation_root.display()
    );
    if let Some(lib) = &args.library_root {
        println!("            : library_root={}", lib.display());
    }
    println!(
        "            : model={}",
        args.model.as_deref().unwrap_or("(default)")
    );
    println!();

    // 1. Set up project directory + .sim-flow state.
    if !args.project_dir.exists() {
        fs::create_dir_all(&args.project_dir)
            .map_err(|err| format!("create_dir_all({}): {err}", args.project_dir.display()))?;
    }
    init_project(&args.project_dir, &args.foundation_root)?;

    // 2. Write fixture spec to the project so DM0 has source material.
    // We bypass the spec-ingest pipeline (which copies + chunks) and
    // just drop the raw spec at `.sim-flow/source-spec.md` plus a
    // single-page TOC so the orchestrator's `build_spec_toc_message`
    // helper sees it.
    install_fixture_spec(&args.project_dir)?;

    // 3. Run the auto driver. Wraps `ClaudeAgent` (one-shot `claude -p`)
    // in a `TerminalHost` so each turn dispatches through the agent.
    // stdin is closed (the auto driver shouldn't need user input;
    // any RequestUserInput is treated as "fall back to the user" in
    // interactive mode and we'll see it as a stuck terminal).
    let agent = ClaudeAgent::new(args.model.clone());
    let stdin = BufReader::new(Cursor::new(Vec::<u8>::new()));
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let mut host = TerminalHost::new(BoxedAgent(Box::new(agent)), stdin, stdout, stderr);

    let opts = AutoOptions {
        project_dir: args.project_dir.clone(),
        foundation_root: args.foundation_root.clone(),
        llm_backend: "claude".into(),
        llm_model: args.model.clone(),
        max_auto_iters: 3,
        max_critique_iters: 3,
        dm0_interactive: false,
        max_llm_requests: 50,
        max_identical_responses: 3,
        step_mode: sim_flow::__internal::session::protocol::StepMode::Auto,
        no_preamble: true,
    };

    println!("dm_flow_smoke: launching run_auto via TerminalHost + ClaudeAgent...");
    println!("            : (each step's transcript will print to this terminal)\n");
    let started_run = Instant::now();
    run_auto(opts, &mut host).map_err(|err| format!("run_auto error: {err}"))?;
    let run_elapsed = started_run.elapsed();

    println!(
        "\ndm_flow_smoke: run_auto returned after {:.1}s",
        run_elapsed.as_secs_f64()
    );

    // 4. Walk the per-step state and report which gates passed.
    summarize_state(&args.project_dir)?;

    let total = started_total.elapsed();
    println!("\ndm_flow_smoke: total {:.1}s", total.as_secs_f64());
    Ok(())
}

fn init_project(project_dir: &Path, foundation_root: &Path) -> std::result::Result<(), String> {
    let dot = project_dir.join(".sim-flow");
    if dot.join("state.toml").exists() {
        println!(
            "            : reusing existing .sim-flow at {}",
            dot.display()
        );
        return Ok(());
    }
    let bin = foundation_root.join("target/release/sim-flow");
    let bin = if bin.exists() {
        bin
    } else {
        // Fall back to debug if release isn't built; informs the user.
        let dbg = foundation_root.join("target/debug/sim-flow");
        if !dbg.exists() {
            return Err(format!(
                "no sim-flow binary at {} or {}; run `cargo build -p sim-flow [--release]` first",
                bin.display(),
                dbg.display(),
            ));
        }
        dbg
    };
    let status = Command::new(&bin)
        .arg("--foundation-root")
        .arg(foundation_root)
        .arg("--project")
        .arg(project_dir)
        .arg("init")
        .arg("--flow")
        .arg("direct-modeling")
        .status()
        .map_err(|err| format!("`sim-flow init` spawn: {err}"))?;
    if !status.success() {
        return Err(format!(
            "`sim-flow init` exited {}",
            status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

fn install_fixture_spec(project_dir: &Path) -> std::result::Result<(), String> {
    let dot = project_dir.join(".sim-flow");
    fs::create_dir_all(&dot).map_err(|err| format!("mkdir .sim-flow: {err}"))?;
    let pages_dir = dot.join("spec-pages");
    fs::create_dir_all(&pages_dir).map_err(|err| format!("mkdir spec-pages: {err}"))?;
    let spec_path = dot.join("source-spec.md");
    fs::write(&spec_path, FIXTURE_SPEC).map_err(|err| format!("write source-spec.md: {err}"))?;
    fs::write(pages_dir.join("001.md"), FIXTURE_SPEC)
        .map_err(|err| format!("write spec-pages/001.md: {err}"))?;
    let toc = "# Source spec TOC\n\n- `001.md` -- Tiny Datapath spec\n";
    fs::write(dot.join("source-spec-toc.md"), toc)
        .map_err(|err| format!("write source-spec-toc.md: {err}"))?;
    println!(
        "            : installed fixture spec ({} bytes)",
        FIXTURE_SPEC.len()
    );
    Ok(())
}

fn summarize_state(project_dir: &Path) -> std::result::Result<(), String> {
    let state_path = project_dir.join(".sim-flow/state.toml");
    let body = fs::read_to_string(&state_path).map_err(|err| format!("read state.toml: {err}"))?;
    println!("\n--- state.toml ---");
    print!("{body}");
    println!("--- end state.toml ---");

    // Quick check: every artifact we'd expect after the flow.
    let checks = [
        ("docs/spec.md", project_dir.join("docs/spec.md")),
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
        ("src/lib.rs", project_dir.join("src/lib.rs")),
        ("Cargo.toml", project_dir.join("Cargo.toml")),
    ];
    println!("\n--- artifact presence ---");
    for (label, path) in &checks {
        let bytes = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
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
