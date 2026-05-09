use std::path::{Path, PathBuf};

use sim_flow::__internal::config::Config;
use sim_flow::__internal::foundation_root;
use sim_flow::__internal::runner::{DOT_SIM_FLOW, StepRunner};
use sim_flow::__internal::session::protocol::SessionEndReason;
use sim_flow::__internal::session::{Event, Host};
use sim_flow::__internal::state::{Flow, State};
use sim_flow::__internal::steps::registry_for;

use crate::cli::{
    BaselineAction, Cli, Command, ConfigAction, CoverageAction, KeysAction, NewKind,
    PromptResetScope, PromptScopeArg, PromptsAction, SessionMode, WatchersAction,
};

pub(crate) fn run(cli: &Cli) -> sim_flow::Result<()> {
    let project_dir = match &cli.project {
        Some(p) => p.clone(),
        None => std::env::current_dir().map_err(|source| sim_flow::Error::Io {
            path: PathBuf::from("."),
            source,
        })?,
    };
    match &cli.command {
        Command::Init { flow } => init(&project_dir, (*flow).into()),
        Command::Status { json } => status(&project_dir, *json),
        Command::Run { step, candidate } => run_step(
            cli,
            &project_dir,
            step.as_deref(),
            candidate.as_deref(),
            false,
            false,
        ),
        Command::Gate {
            step,
            candidate,
            json,
        } => run_step(
            cli,
            &project_dir,
            step.as_deref(),
            candidate.as_deref(),
            true,
            *json,
        ),
        Command::Reset { step } => reset(&project_dir, step),
        Command::Config { action } => config_cmd(&project_dir, action),
        Command::New { kind } => new_cmd(cli, &project_dir, kind),
        Command::Runs {
            workload,
            candidate,
            study,
            sweep,
            limit,
            json,
        } => runs_cmd(
            &project_dir,
            workload.as_deref(),
            candidate.as_deref(),
            study.as_deref(),
            sweep.as_deref(),
            *limit,
            *json,
        ),
        Command::RecordRun {
            description,
            workload,
            candidate,
            study,
            manifest,
            notes,
        } => record_run_cmd(
            &project_dir,
            description,
            workload.as_deref(),
            candidate.as_deref(),
            study.as_deref(),
            manifest.as_deref(),
            notes.as_deref(),
        ),
        Command::Baseline { action } => baseline_cmd(&project_dir, action),
        Command::Sweep { file } => sweep_cmd(&project_dir, file),
        Command::SweepResults { parent } => sweep_results_cmd(&project_dir, parent),
        Command::Advance {
            step,
            candidate,
            json,
        } => advance(&project_dir, step.as_deref(), candidate.as_deref(), *json),
        Command::Describe { step_kind, json } => describe(cli, &project_dir, step_kind, *json),
        Command::Session {
            step_kind,
            jsonl,
            transport_socket,
            llm_backend,
            llm_model,
            ollama_base_url,
            openai_base_url,
            llm_base_url,
            candidate,
        } => session_cmd(
            cli,
            &project_dir,
            step_kind,
            *jsonl,
            transport_socket.as_deref(),
            llm_backend,
            llm_model.as_deref(),
            ollama_base_url.as_deref(),
            openai_base_url.as_deref(),
            llm_base_url.as_deref(),
            candidate.as_deref(),
        ),
        Command::Auto {
            llm_backend,
            llm_model,
            llm_base_url,
            max_auto_iters,
            max_critique_iters,
            dm0_interactive,
            spec,
            transport_socket,
            watch_socket,
            session_mode,
            step_mode,
            max_llm_requests,
            max_identical_responses,
            no_preamble,
        } => auto_cmd(
            cli,
            &project_dir,
            llm_backend,
            llm_model.as_deref(),
            llm_base_url.as_deref(),
            *max_auto_iters,
            *max_critique_iters,
            *dm0_interactive,
            spec.as_deref(),
            transport_socket.as_deref(),
            watch_socket.as_deref(),
            *session_mode,
            (*step_mode).into(),
            *max_llm_requests,
            *max_identical_responses,
            *no_preamble,
        ),
        Command::Prompts { action } => prompts_cmd(cli, &project_dir, action),
        Command::Coverage { action } => coverage_cmd(&project_dir, action),
        Command::BlockDiagram {
            output,
            direction,
            show_types,
            netlist,
        } => block_diagram_cmd(
            &project_dir,
            output.as_deref(),
            direction,
            *show_types,
            netlist.as_deref(),
        ),
        Command::Keys { action } => keys_cmd(action),
        Command::Watchers { action } => watchers_cmd(action),
    }
}

fn watchers_cmd(action: &WatchersAction) -> sim_flow::Result<()> {
    match action {
        WatchersAction::List { json } => {
            let mut entries = sim_flow::__internal::session::list_watch_registrations()?;
            entries.sort_by(|a, b| a.started_at.cmp(&b.started_at));
            if *json {
                let out: Vec<_> = entries
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "pid": r.pid,
                            "socket_path": r.socket_path.display().to_string(),
                            "project_dir": r.project_dir.display().to_string(),
                            "started_at": r.started_at,
                            "llm_backend": r.llm_backend,
                            "llm_model": r.llm_model,
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&out)
                        .map_err(|e| sim_flow::Error::State(format!("watchers list json: {e}")))?
                );
            } else if entries.is_empty() {
                println!("(no live watchers)");
            } else {
                println!(
                    "{:>6} {:24} {:32} SOCKET",
                    "PID", "BACKEND/MODEL", "PROJECT"
                );
                for r in &entries {
                    let backend = match &r.llm_model {
                        Some(m) => format!("{}/{}", r.llm_backend, m),
                        None => r.llm_backend.clone(),
                    };
                    println!(
                        "{:>6} {:24} {:32} {}",
                        r.pid,
                        truncate_field(&backend, 24),
                        truncate_field(&r.project_dir.display().to_string(), 32),
                        r.socket_path.display(),
                    );
                }
            }
            Ok(())
        }
    }
}

fn truncate_field(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let head: String = s.chars().take(n.saturating_sub(1)).collect();
    format!("{head}…")
}

fn prompts_cmd(cli: &Cli, project_dir: &Path, action: &PromptsAction) -> sim_flow::Result<()> {
    use sim_flow::__internal::prompts::{self, PromptScope};

    let foundation = foundation_root::resolve(cli.foundation_root.as_deref())?;
    match action {
        PromptsAction::List { json } => {
            let entries = prompts::list_prompts(&foundation, project_dir)?;
            if *json {
                let out: Vec<_> = entries
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "slug": e.slug,
                            "kind": kind_str(e.kind),
                            "active_scope": e.active_scope.as_str(),
                            "project_path": e.project_path.display().to_string(),
                            "project_present": e.project_present,
                            "global_path": e.global_path.as_ref().map(|p| p.display().to_string()),
                            "global_present": e.global_present,
                            "default_path": e.default_path.display().to_string(),
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&out)
                        .map_err(|e| sim_flow::Error::State(format!("prompts list json: {e}")))?
                );
            } else {
                for e in &entries {
                    let scope_marker = match e.active_scope {
                        PromptScope::Project => "[project]",
                        PromptScope::Global => "[global]",
                        PromptScope::Default => "[default]",
                    };
                    println!("{:24} {:10} {scope_marker}", e.slug, kind_str(e.kind));
                }
            }
            Ok(())
        }
        PromptsAction::Show { slug_kind } => {
            let (slug, kind) = parse_slug_kind(slug_kind)?;
            let resolved = prompts::load_scoped(&foundation, project_dir, &slug, kind)?;
            print!("{}", resolved.content);
            Ok(())
        }
        PromptsAction::Save { slug_kind, scope } => {
            let (slug, kind) = parse_slug_kind(slug_kind)?;
            let mut content = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut content).map_err(|err| {
                sim_flow::Error::State(format!("prompts save: read stdin: {err}"))
            })?;
            let path = prompts::save_override(
                prompt_scope_for(*scope),
                project_dir,
                &slug,
                kind,
                &content,
            )?;
            eprintln!("saved override to {}", path.display());
            Ok(())
        }
        PromptsAction::Reset { slug_kind, scope } => {
            let (slug, kind) = parse_slug_kind(slug_kind)?;
            let scopes: &[PromptScope] = match scope {
                PromptResetScope::Project => &[PromptScope::Project],
                PromptResetScope::Global => &[PromptScope::Global],
                PromptResetScope::All => &[PromptScope::Project, PromptScope::Global],
            };
            for s in scopes {
                let removed = prompts::delete_override(*s, project_dir, &slug, kind)?;
                if removed {
                    eprintln!(
                        "removed {} override for {slug}.{}",
                        s.as_str(),
                        kind_str(kind)
                    );
                }
            }
            Ok(())
        }
        PromptsAction::Path { slug_kind, scope } => {
            let (slug, kind) = parse_slug_kind(slug_kind)?;
            let path = match scope {
                Some(PromptScopeArg::Project) => {
                    prompts::project_override_path(project_dir, &slug, kind)
                }
                Some(PromptScopeArg::Global) => prompts::global_override_path(&slug, kind)
                    .ok_or_else(|| {
                        sim_flow::Error::State(
                            "prompts path: global config dir is not resolvable on this platform"
                                .into(),
                        )
                    })?,
                None => {
                    let resolved = prompts::load_scoped(&foundation, project_dir, &slug, kind)?;
                    resolved.path
                }
            };
            println!("{}", path.display());
            Ok(())
        }
    }
}

fn parse_slug_kind(
    s: &str,
) -> sim_flow::Result<(String, sim_flow::__internal::client::SessionKind)> {
    use sim_flow::__internal::client::SessionKind;
    let (slug, kind_s) = s.rsplit_once('.').ok_or_else(|| {
        sim_flow::Error::InvalidStep(format!(
            "expected `<slug>.work` or `<slug>.critique`, got `{s}`"
        ))
    })?;
    let kind = match kind_s {
        "work" => SessionKind::Work,
        "critique" => SessionKind::Critique,
        other => {
            return Err(sim_flow::Error::InvalidStep(format!(
                "unknown kind `{other}`; expected `work` or `critique`"
            )));
        }
    };
    Ok((slug.to_string(), kind))
}

fn kind_str(kind: sim_flow::__internal::client::SessionKind) -> &'static str {
    use sim_flow::__internal::client::SessionKind;
    match kind {
        SessionKind::Work => "work",
        SessionKind::Critique => "critique",
    }
}

fn prompt_scope_for(scope: PromptScopeArg) -> sim_flow::__internal::prompts::PromptScope {
    use sim_flow::__internal::prompts::PromptScope;
    match scope {
        PromptScopeArg::Project => PromptScope::Project,
        PromptScopeArg::Global => PromptScope::Global,
    }
}

fn coverage_cmd(project_dir: &Path, action: &CoverageAction) -> sim_flow::Result<()> {
    let dot = project_dir.join(DOT_SIM_FLOW);
    let mut cfg = Config::load(&dot)?;
    match action {
        CoverageAction::Show { json } => {
            print_coverage(&cfg, *json);
        }
        CoverageAction::Set {
            threshold_pct,
            level,
        } => {
            // Honor each flag independently: if only `--level` is
            // passed, keep the existing threshold (and vice versa).
            // Passing neither is legal; it just round-trips the
            // current config (after the clamp pass that `set_coverage`
            // performs, which can normalize a previously-broken
            // file).
            let new_pct = threshold_pct.unwrap_or(cfg.coverage.threshold_pct);
            let new_level = level.map(Into::into).unwrap_or(cfg.coverage.level);
            cfg.set_coverage(new_pct, new_level);
            cfg.save(&dot)?;
            print_coverage(&cfg, false);
        }
    }
    Ok(())
}

fn print_coverage(cfg: &Config, json: bool) {
    if json {
        // Hand-rolled JSON: the dashboard parses this and we want
        // to avoid pulling serde_json in just for two fields. The
        // float-formatting trick (`{:.4}`) avoids `90` (no decimal)
        // round-tripping into something a JSON parser would accept
        // as an integer.
        println!(
            "{{\"threshold_pct\":{:.4},\"level\":\"{}\"}}",
            cfg.coverage.threshold_pct,
            cfg.coverage.level.as_str(),
        );
    } else {
        println!(
            "coverage: threshold={:.1}% level={}",
            cfg.coverage.threshold_pct,
            cfg.coverage.level.as_str(),
        );
    }
}

fn block_diagram_cmd(
    project: &Path,
    output: Option<&Path>,
    direction: &str,
    show_types: bool,
    netlist_in: Option<&Path>,
) -> sim_flow::Result<()> {
    let path = sim_flow::__internal::block_diagram::render_for_project(
        sim_flow::__internal::block_diagram::RenderConfig {
            project_dir: project,
            output,
            direction,
            show_types,
            netlist_in,
        },
    )?;
    eprintln!("block-diagram: wrote {}", path.display());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn auto_cmd(
    cli: &Cli,
    project: &Path,
    llm_backend: &str,
    llm_model: Option<&str>,
    llm_base_url: Option<&str>,
    max_auto_iters: u32,
    max_critique_iters: u32,
    dm0_interactive: bool,
    spec: Option<&Path>,
    transport_socket: Option<&Path>,
    watch_socket: Option<&Path>,
    session_mode: SessionMode,
    step_mode: sim_flow::__internal::session::protocol::StepMode,
    max_llm_requests: u32,
    max_identical_responses: u32,
    no_preamble: bool,
) -> sim_flow::Result<()> {
    let foundation = foundation_root::resolve(cli.foundation_root.as_deref())?;
    // Tooling preflight: DM3c shells out to `cargo tarpaulin` from
    // inside the agent's work session. If the binary isn't on PATH,
    // the agent gets a "no such command: tarpaulin" error mid-run
    // and burns LLM budget retrying. Install once at startup so the
    // tool is ready by the time the flow reaches DM3c. Failures
    // are non-fatal -- DS / DM0..DM3b don't touch tarpaulin and a
    // network outage shouldn't block them.
    ensure_tarpaulin_available();
    // Pre-DM0 ingestion hook: ensures `.sim-flow/source-spec*` is up
    // to date before the first session's system stack is built. The
    // helper resolves a spec from (1) the CLI `--spec` arg, or (2)
    // `.sim-flow/config.toml::spec_path` when the CLI arg is absent,
    // and skips re-ingestion when the source on disk hasn't changed
    // (mtime comparison). Doing this here means every dashboard
    // launch path -- manual Play, red Play, chat-participant -- gets
    // the same idempotent ingest, regardless of whether the user
    // typed the spec into the dashboard's Spec field or set it via
    // `sim-flow ... --spec ...`.
    ensure_source_spec_ingested(spec, project)?;

    // Interactive PTY path: when the user has chosen a CLI-agent
    // backend (currently only `claude`), spawn the agent on a PTY
    // instead of waiting on the JSONL protocol. The session-mode
    // flag picks per-step (fresh agent per step) vs single-session
    // (one persistent agent + control socket for the dashboard).
    let is_interactive_backend = matches!(llm_backend, "claude" | "claude-cli");
    if is_interactive_backend {
        let opts = sim_flow::__internal::session::AutoInteractiveOptions {
            project_dir: project.to_path_buf(),
            foundation_root: foundation,
            llm_backend: llm_backend.to_string(),
            llm_model: llm_model.map(String::from),
            dm0_interactive,
        };
        let _ = (max_auto_iters, max_critique_iters); // not used in interactive mode
        return match session_mode {
            SessionMode::PerStep => sim_flow::__internal::session::run_auto_interactive(opts),
            SessionMode::Single => {
                sim_flow::__internal::session::auto_interactive::run_auto_interactive_single(opts)
            }
        };
    }

    // JSONL host path: extension drives sim-flow over stdin/stdout.
    let opts = sim_flow::__internal::session::AutoOptions {
        project_dir: project.to_path_buf(),
        foundation_root: foundation,
        llm_backend: llm_backend.to_string(),
        llm_model: llm_model.map(String::from),
        llm_base_url: llm_base_url.map(String::from),
        max_auto_iters,
        max_critique_iters,
        dm0_interactive,
        max_llm_requests,
        max_identical_responses,
        step_mode,
        no_preamble,
    };
    // Optional read-only event broadcast. When `--watch-socket` is
    // set, every event the orchestrator emits is mirrored to the
    // tap; observers attach via Unix socket and receive history +
    // live stream. Initialised here (before the host wrapper) so a
    // bind error fails fast. The registration writes a JSON file in
    // the discovery directory so the dashboard's "Attach to running
    // session" picker can list this run without the user having to
    // know the socket path up front.
    let watch_tap = match watch_socket {
        Some(path) => {
            let registration = sim_flow::__internal::session::WatchRegistration {
                pid: std::process::id(),
                socket_path: path.to_path_buf(),
                project_dir: project.to_path_buf(),
                started_at: current_iso8601(),
                llm_backend: llm_backend.to_string(),
                llm_model: llm_model.map(String::from),
            };
            Some(
                sim_flow::__internal::session::EventTap::bind_with_registration(
                    path.to_path_buf(),
                    registration,
                )?,
            )
        }
        None => None,
    };

    if let Some(socket_path) = transport_socket {
        run_with_socket_session_end(socket_path, |host| match watch_tap {
            Some(tap) => {
                let mut tapped = sim_flow::__internal::session::TappedHost::new(host, tap);
                sim_flow::__internal::session::run_auto(opts, &mut tapped)
            }
            None => sim_flow::__internal::session::run_auto(opts, host),
        })
    } else {
        let host = sim_flow::__internal::session::JsonlHost::stdio();
        match watch_tap {
            Some(tap) => {
                let mut tapped = sim_flow::__internal::session::TappedHost::new(host, tap);
                sim_flow::__internal::session::run_auto(opts, &mut tapped)
            }
            None => {
                let mut host = host;
                sim_flow::__internal::session::run_auto(opts, &mut host)
            }
        }
    }
}

/// Resolve the source-spec to ingest and run `ingest_spec_file` if
/// needed. Resolution order:
///
/// 1. The explicit `--spec` CLI argument when present (overrides
///    everything; treats the on-disk source as authoritative).
/// 2. `.sim-flow/config.toml::spec_path` -- the dashboard's Spec
///    field writes here, so the orchestrator finds the user's
///    chosen spec regardless of which launch path is used.
///
/// When neither is set, the function emits a stderr line and
/// returns Ok(()) -- DM0 will then prompt the user (manual mode)
/// or auto-decide (automated mode) per the prompt instructions.
///
/// Idempotency: when the resolved spec already has a corresponding
/// `.sim-flow/source-spec.<ext>` whose mtime is at least the source's,
/// ingestion is skipped to avoid re-paginating large source specs.
/// Ensure `cargo tarpaulin` is on PATH; install it (via `cargo
/// install cargo-tarpaulin --locked`) if not. The agent runs
/// `cargo tarpaulin` during DM3c (Test Execution and Coverage);
/// pre-installing here means we don't waste an LLM turn on a
/// "command not found" error and a retry. Failures are non-fatal
/// -- not every flow uses tarpaulin (DS doesn't), and a transient
/// network outage shouldn't block flows that don't need it. We
/// log a warning either way so the user can spot the problem if
/// DM3c does come around.
fn ensure_tarpaulin_available() {
    use sim_flow::__internal::preflight::{
        SystemRunner, TarpaulinStatus, ensure_tarpaulin_installed,
    };

    let mut runner = SystemRunner;
    match ensure_tarpaulin_installed(&mut runner, |line| eprintln!("{line}")) {
        Ok(TarpaulinStatus::AlreadyInstalled { version }) => {
            // First-line slice is enough; some installs print
            // multi-line metadata that we don't need to spam.
            let first = version.lines().next().unwrap_or(version.as_str());
            eprintln!("sim-flow: cargo-tarpaulin OK ({first}).");
        }
        Ok(TarpaulinStatus::JustInstalled) => {
            eprintln!("sim-flow: cargo-tarpaulin installed.");
        }
        Err(reason) => {
            eprintln!(
                "sim-flow: cargo-tarpaulin install failed ({reason}); DM3c will surface a `command not found` error if/when it runs. Install manually with `cargo install cargo-tarpaulin --locked` to recover.",
            );
        }
    }
}

fn ensure_source_spec_ingested(cli_spec: Option<&Path>, project: &Path) -> sim_flow::Result<()> {
    use sim_flow::__internal::session::ingest_spec_file;

    let resolved: Option<PathBuf> = if let Some(p) = cli_spec {
        Some(p.to_path_buf())
    } else {
        let dot = project.join(DOT_SIM_FLOW);
        let cfg = Config::load(&dot)?;
        cfg.spec_path
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    };
    let Some(spec_path) = resolved else {
        eprintln!("sim-flow: no source spec configured; DM0 will prompt for one.",);
        return Ok(());
    };
    if !spec_path.exists() {
        eprintln!(
            "sim-flow: configured spec `{}` does not exist; DM0 will prompt for one.",
            spec_path.display(),
        );
        return Ok(());
    }
    if source_spec_up_to_date(&spec_path, project) {
        eprintln!(
            "sim-flow: source spec `{}` already ingested; skipping.",
            spec_path.display(),
        );
        return Ok(());
    }
    let summary = ingest_spec_file(&spec_path, project)?;
    eprintln!(
        "sim-flow: ingested spec `{}` -> {} page(s) under `{}`",
        spec_path.display(),
        summary.page_count,
        summary.pages_dir.display(),
    );
    Ok(())
}

/// True iff `.sim-flow/source-spec.<ext>` exists for the given
/// source path and its mtime is at least as recent as the source.
/// Conservative: any I/O error reading mtimes returns `false` so
/// the caller falls through to a re-ingest.
fn source_spec_up_to_date(spec_path: &Path, project: &Path) -> bool {
    let dot = project.join(DOT_SIM_FLOW);
    let ext = spec_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("md");
    let dest = dot.join(format!("source-spec.{ext}"));
    let Ok(dest_meta) = std::fs::metadata(&dest) else {
        return false;
    };
    let Ok(src_meta) = std::fs::metadata(spec_path) else {
        return false;
    };
    let (Ok(d), Ok(s)) = (dest_meta.modified(), src_meta.modified()) else {
        return false;
    };
    d >= s
}

#[allow(clippy::too_many_arguments)]
fn session_cmd(
    cli: &Cli,
    project: &Path,
    step_kind: &str,
    jsonl: bool,
    transport_socket: Option<&Path>,
    llm_backend: &str,
    llm_model: Option<&str>,
    ollama_base_url: Option<&str>,
    openai_base_url: Option<&str>,
    llm_base_url: Option<&str>,
    candidate: Option<&str>,
) -> sim_flow::Result<()> {
    let (step_id, kind_str) = step_kind.split_once('.').ok_or_else(|| {
        sim_flow::Error::InvalidStep(format!(
            "expected `<step>.<kind>` (e.g. `DM0.work`), got `{step_kind}`"
        ))
    })?;
    let kind = match kind_str {
        "work" => sim_flow::__internal::client::SessionKind::Work,
        "critique" => sim_flow::__internal::client::SessionKind::Critique,
        other => {
            return Err(sim_flow::Error::InvalidStep(format!(
                "unknown session kind `{other}`; expected `work` or `critique`"
            )));
        }
    };
    let foundation = foundation_root::resolve(cli.foundation_root.as_deref())?;
    let opts = sim_flow::__internal::session::OrchestratorOptions {
        project_dir: project.to_path_buf(),
        foundation_root: foundation,
        step_id: step_id.to_string(),
        kind,
        candidate: candidate.map(String::from),
        llm_backend: llm_backend.to_string(),
        llm_model: llm_model.map(String::from),
        ..Default::default()
    };
    if let Some(socket_path) = transport_socket {
        run_with_socket_session_end(socket_path, |host| {
            sim_flow::__internal::session::run_session(opts, host)
        })
    } else if jsonl {
        let mut host = sim_flow::__internal::session::JsonlHost::stdio();
        sim_flow::__internal::session::run_session(opts, &mut host)
    } else {
        let agent_config = sim_flow::__internal::session::AgentConfig {
            model: llm_model.map(String::from),
            model_family_id: None,
            base_url: llm_base_url.map(String::from),
            ollama_base_url: ollama_base_url.map(String::from),
            openai_base_url: openai_base_url.map(String::from),
        };
        let agent = match sim_flow::__internal::session::build_cli_agent(llm_backend, agent_config)
        {
            Some(a) => a,
            None => {
                return Err(sim_flow::Error::State(format!(
                    "TerminalHost has no built-in agent for `{llm_backend}`. Available: {}.",
                    sim_flow::__internal::session::KNOWN_AGENTS.join(", "),
                )));
            }
        };
        let stdin = std::io::stdin();
        let stdin_lock = stdin.lock();
        let stdout = std::io::stdout();
        let stderr = std::io::stderr();
        let mut host = sim_flow::__internal::session::TerminalHost::new(
            BoxedAgent(agent),
            stdin_lock,
            stdout,
            stderr,
        );
        sim_flow::__internal::session::run_session(opts, &mut host)
    }
}

fn run_with_socket_session_end<F>(socket_path: &Path, run: F) -> sim_flow::Result<()>
where
    F: FnOnce(
        &mut SessionEndTrackingHost<sim_flow::__internal::session::SocketHost>,
    ) -> sim_flow::Result<()>,
{
    let socket_host = sim_flow::__internal::session::SocketHost::bind(socket_path.to_path_buf())?;
    let mut host = SessionEndTrackingHost::new(socket_host);
    run_with_error_session_end(&mut host, run)
}

fn run_with_error_session_end<H: Host, F>(
    host: &mut SessionEndTrackingHost<H>,
    run: F,
) -> sim_flow::Result<()>
where
    F: FnOnce(&mut SessionEndTrackingHost<H>) -> sim_flow::Result<()>,
{
    match run(host) {
        Ok(()) => Ok(()),
        Err(err) => {
            if !host.saw_session_end {
                let message = format!("sim-flow session failed: {err}");
                let _ = host.write(&Event::SessionEnd {
                    reason: SessionEndReason::Error,
                    message: Some(message),
                });
            }
            Err(err)
        }
    }
}

struct SessionEndTrackingHost<H> {
    inner: H,
    saw_session_end: bool,
}

impl<H> SessionEndTrackingHost<H> {
    fn new(inner: H) -> Self {
        Self {
            inner,
            saw_session_end: false,
        }
    }
}

impl<H: Host> Host for SessionEndTrackingHost<H> {
    fn write(&mut self, event: &Event) -> sim_flow::Result<()> {
        if matches!(event, Event::SessionEnd { .. }) {
            self.saw_session_end = true;
        }
        self.inner.write(event)
    }

    fn read(&mut self) -> sim_flow::Result<Option<sim_flow::__internal::session::HostEvent>> {
        self.inner.read()
    }
}

/// Wrapper that adapts `Box<dyn CliAgent>` to satisfy `TerminalHost`'s
/// `A: CliAgent` bound. Cheap to inline.
struct BoxedAgent(Box<dyn sim_flow::__internal::session::CliAgent>);

impl sim_flow::__internal::session::CliAgent for BoxedAgent {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn dispatch(
        &self,
        messages: &[sim_flow::__internal::session::LlmMessage],
    ) -> sim_flow::Result<(String, sim_flow::__internal::session::agent::LlmCallMetrics)> {
        self.0.dispatch(messages)
    }
}

fn dot_dir(project: &Path) -> PathBuf {
    project.join(DOT_SIM_FLOW)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingHost {
        written: Vec<Event>,
    }

    impl Host for RecordingHost {
        fn write(&mut self, event: &Event) -> sim_flow::Result<()> {
            self.written.push(event.clone());
            Ok(())
        }

        fn read(&mut self) -> sim_flow::Result<Option<sim_flow::__internal::session::HostEvent>> {
            Ok(None)
        }
    }

    #[test]
    fn fallback_session_end_is_emitted_on_runtime_error() {
        let mut host = SessionEndTrackingHost::new(RecordingHost::default());
        let err = run_with_error_session_end(&mut host, |_host| {
            Err(sim_flow::Error::State("boom".into()))
        })
        .unwrap_err();

        assert!(format!("{err}").contains("boom"));
        assert_eq!(host.inner.written.len(), 1);
        match &host.inner.written[0] {
            Event::SessionEnd { reason, message } => {
                assert_eq!(*reason, SessionEndReason::Error);
                assert_eq!(
                    message.as_deref(),
                    Some("sim-flow session failed: state error: boom")
                );
            }
            other => panic!("expected SessionEnd, got {other:?}"),
        }
    }

    #[test]
    fn fallback_session_end_is_skipped_when_session_already_ended() {
        let mut host = SessionEndTrackingHost::new(RecordingHost::default());
        let err = run_with_error_session_end(&mut host, |host| {
            host.write(&Event::SessionEnd {
                reason: SessionEndReason::ProtocolMismatch,
                message: Some("bad hello".into()),
            })?;
            Err(sim_flow::Error::State("boom".into()))
        })
        .unwrap_err();

        assert!(format!("{err}").contains("boom"));
        assert_eq!(host.inner.written.len(), 1);
        match &host.inner.written[0] {
            Event::SessionEnd { reason, message } => {
                assert_eq!(*reason, SessionEndReason::ProtocolMismatch);
                assert_eq!(message.as_deref(), Some("bad hello"));
            }
            other => panic!("expected SessionEnd, got {other:?}"),
        }
    }
}

fn init(project: &Path, flow: Flow) -> sim_flow::Result<()> {
    let dot = dot_dir(project);
    std::fs::create_dir_all(&dot).map_err(|source| sim_flow::Error::Io {
        path: dot.clone(),
        source,
    })?;
    let initial_step = match flow {
        Flow::DirectModeling => "DM0",
        Flow::DesignStudy => "DS0",
    };
    let state = State::new(flow, initial_step);
    state.save(&dot)?;
    let config = Config::default();
    config.save(&dot)?;
    println!(
        "initialized {} at {} (current step: {})",
        flow.as_str(),
        dot.display(),
        initial_step
    );
    Ok(())
}

fn status(project: &Path, json: bool) -> sim_flow::Result<()> {
    let dot = dot_dir(project);
    let state = State::load(&dot)?;
    if json {
        let out = serde_json::to_string_pretty(&state)
            .map_err(|e| sim_flow::Error::State(format!("status --json serialize: {e}")))?;
        println!("{out}");
        return Ok(());
    }
    println!("flow:          {}", state.flow.as_str());
    println!("current step:  {}", state.current_step);
    if state.gates.is_empty() {
        println!("gates:         (none passed)");
    } else {
        println!("gates:");
        for (id, gate) in &state.gates {
            let marker = if gate.passed { "[x]" } else { "[ ]" };
            println!("  {marker} {id}");
            if !gate.candidates.is_empty() {
                for (cand, child) in &gate.candidates {
                    let cmark = if child.passed { "[x]" } else { "[ ]" };
                    println!("      {cmark} {cand}");
                }
            }
        }
    }
    Ok(())
}

fn run_step(
    cli: &Cli,
    project: &Path,
    step_id: Option<&str>,
    candidate: Option<&str>,
    gate_only: bool,
    json: bool,
) -> sim_flow::Result<()> {
    let dot = dot_dir(project);
    let mut state = State::load(&dot)?;
    let registry = registry_for(state.flow);
    let step_id_owned = step_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| state.current_step.clone());
    let step = registry.get(&step_id_owned).ok_or_else(|| {
        sim_flow::Error::InvalidStep(format!(
            "{} is not a {} step",
            step_id_owned,
            state.flow.as_str()
        ))
    })?;
    let config = Config::load(&dot)?;

    if gate_only {
        let checks = &step.gate_checks;
        let report = sim_flow::__internal::gate::evaluate(project, checks)?;
        if json {
            emit_gate_json(step.id, &report)?;
            if report.is_clean() {
                Ok(())
            } else {
                Err(sim_flow::Error::Gate(format!(
                    "{} failed {} checks",
                    step.id,
                    report.failures.len()
                )))
            }
        } else if report.is_clean() {
            println!("gate {}: clean", step.id);
            Ok(())
        } else {
            for failure in &report.failures {
                eprintln!(
                    "gate failure: {} -- {}",
                    failure.description, failure.reason
                );
            }
            Err(sim_flow::Error::Gate(format!(
                "{} failed {} checks",
                step.id,
                report.failures.len()
            )))
        }
    } else {
        let foundation = foundation_root::resolve(cli.foundation_root.as_deref())?;
        let runner = StepRunner::new(project, &foundation, &registry, &config);
        let outcome = runner.run(step, &mut state, candidate)?;
        state.save(&dot)?;
        if outcome.gate_report.is_clean() {
            println!("{} passed", step.id);
            Ok(())
        } else {
            for failure in &outcome.gate_report.failures {
                eprintln!(
                    "gate failure: {} -- {}",
                    failure.description, failure.reason
                );
            }
            Err(sim_flow::Error::Gate(format!(
                "{} failed {} checks",
                step.id,
                outcome.gate_report.failures.len()
            )))
        }
    }
}

#[derive(Debug, serde::Serialize)]
struct GateReportOut<'a> {
    step: &'a str,
    clean: bool,
    failures: &'a [sim_flow::__internal::gate::GateFailure],
}

fn emit_gate_json(
    step: &str,
    report: &sim_flow::__internal::gate::GateReport,
) -> sim_flow::Result<()> {
    let out = GateReportOut {
        step,
        clean: report.is_clean(),
        failures: &report.failures,
    };
    let text = serde_json::to_string_pretty(&out)
        .map_err(|e| sim_flow::Error::Gate(format!("gate --json serialize: {e}")))?;
    println!("{text}");
    Ok(())
}

#[derive(Debug, serde::Serialize)]
struct AdvanceOut<'a> {
    step: &'a str,
    clean: bool,
    advanced: bool,
    next_step: Option<&'a str>,
    failures: &'a [sim_flow::__internal::gate::GateFailure],
}

/// Validate the gate for a step and, if clean, mark it passed and bump
/// `current_step` to the next step in the flow's registry order. This
/// is the explicit state-progression primitive split out from the
/// agent-launching `sim-flow run` command, so hosts can drive their
/// own work + critique sessions and use this to record completion.
fn advance(
    project: &Path,
    step_id: Option<&str>,
    candidate: Option<&str>,
    json: bool,
) -> sim_flow::Result<()> {
    let dot = dot_dir(project);
    let mut state = State::load(&dot)?;
    let registry = registry_for(state.flow);
    let step_id_owned = step_id
        .map(String::from)
        .unwrap_or_else(|| state.current_step.clone());
    let step = registry.get(&step_id_owned).ok_or_else(|| {
        sim_flow::Error::InvalidStep(format!(
            "{} is not a {} step",
            step_id_owned,
            state.flow.as_str()
        ))
    })?;
    if step.per_candidate || candidate.is_some() {
        return Err(sim_flow::Error::InvalidStep(format!(
            "advance does not yet support per-candidate steps ({})",
            step.id
        )));
    }

    let report = sim_flow::__internal::gate::evaluate(project, &step.gate_checks)?;
    if !report.is_clean() {
        if json {
            let out = AdvanceOut {
                step: step.id,
                clean: false,
                advanced: false,
                next_step: None,
                failures: &report.failures,
            };
            let text = serde_json::to_string_pretty(&out)
                .map_err(|e| sim_flow::Error::Gate(format!("advance --json serialize: {e}")))?;
            println!("{text}");
        } else {
            for failure in &report.failures {
                eprintln!(
                    "gate failure: {} -- {}",
                    failure.description, failure.reason
                );
            }
        }
        return Err(sim_flow::Error::Gate(format!(
            "{} failed {} checks; not advancing",
            step.id,
            report.failures.len()
        )));
    }

    let order = registry.order_for(state.flow);
    let next: Option<&'static str> = order
        .iter()
        .position(|s| *s == step.id)
        .and_then(|idx| order.get(idx + 1).copied());

    // Commit the step's artifacts BEFORE persisting state so a
    // committed history reflects "this is what passing the gate
    // looked like." If git fails for any reason we keep going.
    let outcome = sim_flow::__internal::git_commit::commit_step_advance(project, step.id, next);
    if let Some(msg) = sim_flow::__internal::git_commit::outcome_message(&outcome) {
        eprintln!("{msg}");
    }

    state.mark_passed(step.id, current_iso8601());
    if let Some(next_step) = next {
        state.current_step = next_step.to_string();
    }
    state.save(&dot)?;

    if json {
        let out = AdvanceOut {
            step: step.id,
            clean: true,
            advanced: next.is_some(),
            next_step: next,
            failures: &[],
        };
        let text = serde_json::to_string_pretty(&out)
            .map_err(|e| sim_flow::Error::Gate(format!("advance --json serialize: {e}")))?;
        println!("{text}");
    } else if let Some(next_step) = next {
        println!(
            "advanced past {}; current step is now {}",
            step.id, next_step
        );
    } else {
        println!("advanced past {} (final step in this flow)", step.id);
    }
    Ok(())
}

#[derive(Debug, serde::Serialize)]
struct GateCheckOut<'a> {
    kind: &'static str,
    description: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cmd: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<&'a [String]>,
}

fn gate_check_to_out(check: &sim_flow::__internal::gate::GateCheck) -> GateCheckOut<'_> {
    use sim_flow::__internal::gate::GateCheck::*;

    match check {
        FileExists { path, description } => GateCheckOut {
            kind: "file-exists",
            description,
            path: Some(path.display().to_string()),
            pattern: None,
            cmd: None,
            args: None,
        },
        FileMatches {
            path,
            pattern,
            description,
        } => GateCheckOut {
            kind: "file-matches",
            description,
            path: Some(path.display().to_string()),
            pattern: Some(pattern.clone()),
            cmd: None,
            args: None,
        },
        Shell {
            cmd,
            args,
            description,
        } => GateCheckOut {
            kind: "shell",
            description,
            path: None,
            pattern: None,
            cmd: Some(cmd.as_str()),
            args: Some(args.as_slice()),
        },
        CritiqueClean { path, description } => GateCheckOut {
            kind: "critique-clean",
            description,
            path: Some(path.display().to_string()),
            pattern: None,
            cmd: None,
            args: None,
        },
        ExperimentsRecorded { description } => GateCheckOut {
            kind: "experiments-recorded",
            description,
            path: None,
            pattern: None,
            cmd: None,
            args: None,
        },
        MilestonesAllResolved {
            dir,
            file_prefixes,
            placeholder_marker,
            description,
            forbid_deferred,
        } => GateCheckOut {
            kind: if placeholder_marker.is_some() {
                "milestones-all-detailed"
            } else if *forbid_deferred {
                "milestones-all-implemented"
            } else {
                "milestones-all-resolved"
            },
            description,
            path: Some(dir.display().to_string()),
            pattern: Some(file_prefixes.join(" | ")),
            cmd: None,
            args: None,
        },
        AnyExists { paths, description } => GateCheckOut {
            kind: "any-exists",
            description,
            path: Some(
                paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" | "),
            ),
            pattern: None,
            cmd: None,
            args: None,
        },
        AnyMatches {
            paths,
            pattern,
            description,
        } => GateCheckOut {
            kind: "any-matches",
            description,
            path: Some(
                paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" | "),
            ),
            pattern: Some(pattern.clone()),
            cmd: None,
            args: None,
        },
    }
}

#[derive(Debug, serde::Serialize)]
struct DescribeOut<'a> {
    step: &'a str,
    kind: &'a str,
    flow: &'a str,
    prerequisite: Option<&'a str>,
    instruction_path: String,
    instruction_body: String,
    work_artifacts: &'a [&'static str],
    predecessor_inputs: &'a [&'static str],
    per_candidate: bool,
    gate_checks: Vec<GateCheckOut<'a>>,
}

/// Emit a step descriptor for hosts that drive sessions externally.
fn describe(cli: &Cli, project: &Path, step_kind: &str, json: bool) -> sim_flow::Result<()> {
    let (step_id, kind_str) = step_kind.split_once('.').ok_or_else(|| {
        sim_flow::Error::InvalidStep(format!(
            "expected `<step>.<kind>` (e.g. `DM0.work`), got `{step_kind}`"
        ))
    })?;
    let kind = match kind_str {
        "work" => sim_flow::__internal::client::SessionKind::Work,
        "critique" => sim_flow::__internal::client::SessionKind::Critique,
        other => {
            return Err(sim_flow::Error::InvalidStep(format!(
                "unknown session kind `{other}`; expected `work` or `critique`"
            )));
        }
    };

    let dot = dot_dir(project);
    let state = State::load(&dot)?;
    let registry = registry_for(state.flow);
    let step = registry.get(step_id).ok_or_else(|| {
        sim_flow::Error::InvalidStep(format!("{} is not a {} step", step_id, state.flow.as_str()))
    })?;

    let foundation = foundation_root::resolve(cli.foundation_root.as_deref())?;
    let instruction_path =
        sim_flow::__internal::prompts::instruction_path(&foundation, step.instruction_slug, kind);
    let instruction_body =
        sim_flow::__internal::prompts::load(&foundation, step.instruction_slug, kind)?;

    let kind_str = match kind {
        sim_flow::__internal::client::SessionKind::Work => "work",
        sim_flow::__internal::client::SessionKind::Critique => "critique",
    };

    let out = DescribeOut {
        step: step.id,
        kind: kind_str,
        flow: state.flow.as_str(),
        prerequisite: step.prerequisite,
        instruction_path: instruction_path.display().to_string(),
        instruction_body,
        work_artifacts: step.work_artifacts,
        predecessor_inputs: step.predecessor_inputs,
        per_candidate: step.per_candidate,
        gate_checks: step.gate_checks.iter().map(gate_check_to_out).collect(),
    };

    if json {
        let text = serde_json::to_string_pretty(&out)
            .map_err(|e| sim_flow::Error::State(format!("describe --json serialize: {e}")))?;
        println!("{text}");
    } else {
        println!("step:               {}", out.step);
        println!("kind:               {}", out.kind);
        println!("flow:               {}", out.flow);
        if let Some(prereq) = out.prerequisite {
            println!("prerequisite:       {prereq}");
        }
        println!("instruction:        {}", out.instruction_path);
        println!("per-candidate:      {}", out.per_candidate);
        println!("work artifacts:     {:?}", out.work_artifacts);
        println!("predecessor inputs: {:?}", out.predecessor_inputs);
        println!("gate checks:        {} entries", out.gate_checks.len());
    }
    Ok(())
}

fn current_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => format!("{}", d.as_secs()),
        Err(_) => "0".to_string(),
    }
}

fn reset(project: &Path, step_id: &str) -> sim_flow::Result<()> {
    let dot = dot_dir(project);
    let mut state = State::load(&dot)?;
    let registry = registry_for(state.flow);
    let order: Vec<&'static str> = registry.order_for(state.flow);
    let Some(idx) = order.iter().position(|s| *s == step_id) else {
        return Err(sim_flow::Error::InvalidStep(format!(
            "reset: `{step_id}` is not a {} step",
            state.flow.as_str()
        )));
    };
    // Delete every step's work artifacts AND critique file for the
    // reset target and downstream steps BEFORE we rewind state, so a
    // failure mid-cleanup leaves the gate flags intact (the user can
    // re-run `reset` cleanly). Same logic the in-session Reset event
    // uses, so `sim-flow reset` and the dashboard's reset button
    // produce identical disk state.
    let (deleted, failures) = sim_flow::__internal::session::auto::clear_step_collateral_forward(
        project, idx, &order, &registry,
    );
    state.reset(step_id, &order)?;
    state.save(&dot)?;
    let cleared = order.len() - idx;
    println!("reset to {step_id}; cleared {cleared} gate flag(s)");
    if deleted.is_empty() {
        println!("no generated collateral found to delete");
    } else {
        println!("deleted {} file(s) / directory(ies):", deleted.len());
        for path in &deleted {
            let rel = path.strip_prefix(project).unwrap_or(path).display();
            println!("  - {rel}");
        }
    }
    for (path, err) in &failures {
        let rel = path.strip_prefix(project).unwrap_or(path).display();
        eprintln!("warn: failed to delete {rel}: {err}");
    }
    Ok(())
}

fn new_cmd(cli: &Cli, cwd: &Path, kind: &NewKind) -> sim_flow::Result<()> {
    let foundation_root = foundation_root::resolve(cli.foundation_root.as_deref())?;
    match kind {
        NewKind::Model {
            name,
            destination,
            library_path,
            skip_cargo_check,
            json,
        } => {
            let dest = destination.clone().unwrap_or_else(|| cwd.to_path_buf());
            let options = sim_flow::__internal::new_project::NewModelOptions {
                project_name: name.clone(),
                destination: dest,
                foundation_root,
                library_path: library_path.clone(),
                skip_cargo_check: *skip_cargo_check,
            };
            let outcome = sim_flow::__internal::new_project::new_model(&options)?;
            if *json {
                let text = serde_json::to_string_pretty(&outcome)
                    .map_err(|e| sim_flow::Error::State(format!("new model --json: {e}")))?;
                println!("{text}");
            } else {
                println!("Model project created at {}", outcome.project_dir.display());
                println!("Crate name: {}", outcome.crate_name);
                println!(
                    "Next: cd {} && sim-flow run {}",
                    outcome.project_dir.display(),
                    outcome.next_step
                );
            }
            Ok(())
        }
        NewKind::Study { name: _ } => Err(sim_flow::Error::State(
            "sim-flow new study is not yet implemented; see Phase 5 plan".into(),
        )),
        NewKind::Candidate { name: _ } => Err(sim_flow::Error::State(
            "sim-flow new candidate is not yet implemented; see Phase 5 plan".into(),
        )),
    }
}

fn runs_cmd(
    project: &Path,
    workload: Option<&str>,
    candidate: Option<&str>,
    study: Option<&str>,
    sweep: Option<&str>,
    limit: usize,
    json: bool,
) -> sim_flow::Result<()> {
    let dot = dot_dir(project);
    let index = sim_flow::__internal::tracking::index::ExperimentIndex::open(&dot)?;
    let filter = sim_flow::__internal::tracking::index::RunFilter {
        workload: workload.map(str::to_string),
        candidate: candidate.map(str::to_string),
        study: study.map(str::to_string),
        parent_run_id: sweep.map(str::to_string),
        limit: Some(limit),
    };
    let rows = index.list_runs(&filter)?;
    if json {
        let text = serde_json::to_string_pretty(&rows)
            .map_err(|e| sim_flow::Error::State(format!("runs --json serialize: {e}")))?;
        println!("{text}");
        return Ok(());
    }
    if rows.is_empty() {
        println!("(no runs match the filter)");
        return Ok(());
    }
    for row in rows {
        println!(
            "{: <32}  {: <10}  commit={}{}  workload={}  study/candidate={}/{}",
            row.run_id,
            row.lifecycle,
            &row.git_commit[..row.git_commit.len().min(8)],
            if row.git_dirty { " (dirty)" } else { "" },
            row.workload.as_deref().unwrap_or("-"),
            row.study.as_deref().unwrap_or("-"),
            row.candidate.as_deref().unwrap_or("-"),
        );
    }
    Ok(())
}

fn record_run_cmd(
    project: &Path,
    description: &str,
    workload: Option<&str>,
    candidate: Option<&str>,
    study: Option<&str>,
    manifest: Option<&Path>,
    notes: Option<&str>,
) -> sim_flow::Result<()> {
    let dot = dot_dir(project);
    let options = sim_flow::__internal::tracking::RecordRunOptions {
        description: description.to_string(),
        workload: workload.map(str::to_string),
        candidate: candidate.map(str::to_string),
        study: study.map(str::to_string),
        manifest_path: manifest.map(Path::to_path_buf),
        notes: notes.map(str::to_string),
        parent_run_id: None,
        sweep_parameter: None,
        sweep_value: None,
        tags: Vec::new(),
    };
    let recorded =
        sim_flow::__internal::tracking::run_recording::record_run(project, &dot, &options)?;
    println!(
        "Recorded run {} (artifacts: {})",
        recorded.run_id,
        recorded.artifact_dir.display()
    );
    Ok(())
}

fn baseline_cmd(project: &Path, action: &BaselineAction) -> sim_flow::Result<()> {
    let dot = dot_dir(project);
    match action {
        BaselineAction::Create {
            name,
            run,
            notes,
            json,
        } => {
            let record = sim_flow::__internal::tracking::baseline::create(
                &dot,
                name,
                run.as_deref(),
                notes.as_deref(),
            )?;
            if *json {
                let text = serde_json::to_string_pretty(&record)
                    .map_err(|e| sim_flow::Error::State(format!("baseline create --json: {e}")))?;
                println!("{text}");
            } else {
                println!(
                    "Baseline {} pinned to run {} at {}",
                    record.name, record.run_id, record.timestamp
                );
            }
            Ok(())
        }
        BaselineAction::Compare {
            name,
            current,
            json,
        } => {
            let delta =
                sim_flow::__internal::tracking::baseline::compare(&dot, name, current.as_deref())?;
            if *json {
                let text = serde_json::to_string_pretty(&delta)
                    .map_err(|e| sim_flow::Error::State(format!("baseline compare --json: {e}")))?;
                println!("{text}");
                return Ok(());
            }
            println!(
                "Comparing {} (baseline) vs {} (current)",
                delta.baseline_run_id, delta.current_run_id
            );
            println!(
                "  {: <24}  {: <12}  {: <12}  {: <10}",
                "metric", "baseline", "current", "delta%"
            );
            for entry in delta.entries {
                let bf = entry
                    .baseline
                    .map(|v| format!("{v:.4}"))
                    .unwrap_or_else(|| "-".into());
                let cf = entry
                    .current
                    .map(|v| format!("{v:.4}"))
                    .unwrap_or_else(|| "-".into());
                let pf = entry
                    .delta_pct
                    .map(|v| format!("{v:+.1}%"))
                    .unwrap_or_else(|| "-".into());
                println!(
                    "  {: <24}  {: <12}  {: <12}  {: <10}",
                    entry.metric, bf, cf, pf
                );
            }
            Ok(())
        }
        BaselineAction::List { json } => {
            let records = sim_flow::__internal::tracking::baseline::list(&dot)?;
            if *json {
                let text = serde_json::to_string_pretty(&records)
                    .map_err(|e| sim_flow::Error::State(format!("baseline list --json: {e}")))?;
                println!("{text}");
                return Ok(());
            }
            for record in records {
                println!(
                    "{: <24}  -> {}  @ {}",
                    record.name, record.run_id, record.timestamp
                );
            }
            Ok(())
        }
    }
}

fn sweep_cmd(project: &Path, file: &Path) -> sim_flow::Result<()> {
    let dot = dot_dir(project);
    let def = sim_flow::__internal::tracking::sweep::load(file)?;
    let results = sim_flow::__internal::tracking::sweep::run(project, &dot, &def)?;
    println!(
        "Sweep {} complete: parent={}  children={}",
        def.sweep.name,
        results.parent_run_id,
        results.child_run_ids.len()
    );
    for child in results.child_run_ids {
        println!("  {child}");
    }
    Ok(())
}

fn sweep_results_cmd(project: &Path, parent: &str) -> sim_flow::Result<()> {
    let dot = dot_dir(project);
    let children = sim_flow::__internal::tracking::sweep::results(&dot, parent)?;
    if children.is_empty() {
        println!("(no children recorded for {parent})");
        return Ok(());
    }
    println!("{: <32}  {: <24}  metrics_summary", "run_id", "sweep_value");
    for child in children {
        println!(
            "{: <32}  {: <24}  {}",
            child.run_id,
            child.sweep_value.as_deref().unwrap_or("-"),
            child.metrics_summary.as_deref().unwrap_or("{}"),
        );
    }
    Ok(())
}

fn config_cmd(project: &Path, action: &ConfigAction) -> sim_flow::Result<()> {
    let dot = dot_dir(project);
    let config = Config::load(&dot)?;
    match action {
        ConfigAction::Show => {
            let text = toml::to_string_pretty(&config)?;
            println!("{text}");
            Ok(())
        }
    }
}

fn keys_cmd(action: &KeysAction) -> sim_flow::Result<()> {
    use sim_flow::__internal::keys::{
        self, KeySource, Provider, SOURCE_CODE_CONFIG_FILE, SOURCE_CODE_ENV, SOURCE_CODE_NONE,
        config_file_path, list_status,
    };

    /// Width used by the human-readable `keys list` output for the
    /// provider column. Long enough to fit `lmstudio` plus padding.
    const PROVIDER_COLUMN_WIDTH: usize = 9;

    /// Label shown in `keys list` (non-JSON) when no source has a
    /// key for a provider.
    const NO_KEY_LABEL: &str = "(unset)";

    fn parse_provider(raw: &str) -> sim_flow::Result<Provider> {
        Provider::from_str_ci(raw).ok_or_else(|| {
            sim_flow::Error::Config(format!(
                "unknown provider `{raw}`; expected one of: anthropic, openai, ollama, lmstudio"
            ))
        })
    }

    match action {
        KeysAction::Set { provider, from_env } => {
            let p = parse_provider(provider)?;
            let key = if let Some(var) = from_env {
                std::env::var(var).map_err(|_| {
                    sim_flow::Error::Config(format!(
                        "--from-env: env var `{var}` is not set in this shell"
                    ))
                })?
            } else {
                read_key_from_stdin(p.config_key())?
            };
            let trimmed = key.trim();
            if trimmed.is_empty() {
                return Err(sim_flow::Error::Config("api key cannot be empty".into()));
            }
            let path = keys::write_api_key(p, trimmed)?;
            println!("sim-flow: stored {p} key in {}", path.display());
            Ok(())
        }
        KeysAction::Clear { provider } => {
            let p = parse_provider(provider)?;
            let removed = keys::clear_api_key(p)?;
            if removed {
                println!("sim-flow: cleared {p} entry from credentials.toml");
            } else {
                println!("sim-flow: no {p} entry found to clear");
            }
            Ok(())
        }
        KeysAction::List { json } => {
            let statuses = list_status()?;
            let path = config_file_path();
            if *json {
                let rows: Vec<serde_json::Value> = statuses
                    .iter()
                    .map(|s| {
                        let source_code =
                            s.source.map(KeySource::as_str).unwrap_or(SOURCE_CODE_NONE);
                        serde_json::json!({
                            "provider": s.provider.config_key(),
                            "env_var": s.provider.env_var(),
                            "source": source_code,
                        })
                    })
                    .collect();
                let body = serde_json::json!({
                    "config_file": path.as_ref().map(|p| p.display().to_string()),
                    "providers": rows,
                });
                let text = serde_json::to_string_pretty(&body).map_err(|e| {
                    sim_flow::Error::State(format!("keys list --json serialize: {e}"))
                })?;
                println!("{text}");
                return Ok(());
            }
            if let Some(path) = path {
                println!("credentials file: {}", path.display());
            } else {
                println!("credentials file: (no usable config dir on this platform)");
            }
            for s in statuses {
                let label = match s.source {
                    Some(KeySource::Env) => {
                        format!("{SOURCE_CODE_ENV} (${})", s.provider.env_var())
                    }
                    Some(KeySource::ConfigFile) => SOURCE_CODE_CONFIG_FILE.to_string(),
                    None => NO_KEY_LABEL.to_string(),
                };
                println!(
                    "  {provider:width$}  {label}",
                    provider = s.provider.to_string(),
                    width = PROVIDER_COLUMN_WIDTH,
                );
            }
            Ok(())
        }
        KeysAction::Path => {
            match config_file_path() {
                Some(path) => println!("{}", path.display()),
                None => {
                    return Err(sim_flow::Error::Config(
                        "no usable config directory on this platform".into(),
                    ));
                }
            }
            Ok(())
        }
    }
}

/// Prompt the user for an API key on stdin. When stdin is a TTY we
/// suppress echo via the standard libc tcgetattr / tcsetattr dance
/// (no extra crate; small enough to inline). On a non-TTY stdin
/// (CI, piped input), we read normally — the caller decided to
/// provide the key non-interactively.
fn read_key_from_stdin(label: &str) -> sim_flow::Result<String> {
    use std::io::{BufRead, Write};

    let mut stderr = std::io::stderr();
    write!(
        stderr,
        "Paste {label} API key (input will be hidden if stdin is a TTY): "
    )
    .map_err(|source| sim_flow::Error::Io {
        path: PathBuf::from("<stderr>"),
        source,
    })?;
    stderr.flush().ok();

    let stdin = std::io::stdin();
    let is_tty = is_stdin_tty();
    let mut line = String::new();
    let result = if is_tty {
        with_echo_suppressed(|| stdin.lock().read_line(&mut line))
    } else {
        stdin.lock().read_line(&mut line)
    };
    if is_tty {
        // The user's enter keystroke wasn't echoed; print a newline
        // so the next shell prompt isn't crammed up against our text.
        let _ = writeln!(stderr);
    }
    result.map_err(|source| sim_flow::Error::Io {
        path: PathBuf::from("<stdin>"),
        source,
    })?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

#[cfg(unix)]
fn is_stdin_tty() -> bool {
    // SAFETY: isatty(3) is reentrant and side-effect-free.
    unsafe { libc::isatty(libc::STDIN_FILENO) != 0 }
}

#[cfg(not(unix))]
fn is_stdin_tty() -> bool {
    // Non-POSIX: be conservative and assume non-tty so we don't try
    // termios. Users on Windows can pipe input or use --from-env.
    false
}

#[cfg(unix)]
fn with_echo_suppressed<F, T>(f: F) -> T
where
    F: FnOnce() -> T,
{
    use libc::{ECHO, STDIN_FILENO, TCSANOW, tcgetattr, tcsetattr, termios};
    use std::mem::MaybeUninit;

    // Read current termios.
    let mut original: MaybeUninit<termios> = MaybeUninit::uninit();
    // SAFETY: tcgetattr writes a complete termios on success.
    let ok = unsafe { tcgetattr(STDIN_FILENO, original.as_mut_ptr()) };
    if ok != 0 {
        // Couldn't read termios (rare — e.g. stdin redirected since
        // is_stdin_tty()). Fall back to plain read; key may echo.
        return f();
    }
    let original = unsafe { original.assume_init() };
    let mut quiet = original;
    quiet.c_lflag &= !ECHO;
    // SAFETY: quiet is a valid termios derived from a successful
    // tcgetattr above.
    if unsafe { tcsetattr(STDIN_FILENO, TCSANOW, &quiet) } != 0 {
        // Couldn't disable echo; run without it.
        return f();
    }
    let result = f();
    // Restore. Best-effort -- if this fails the user's terminal is
    // stuck without echo until they `stty echo`. Surface a warning
    // so they know what happened; we've already taken their input
    // by this point so there's nothing else to do.
    // SAFETY: `original` was returned by a successful `tcgetattr`
    // above and we haven't mutated it since.
    let restored = unsafe { tcsetattr(STDIN_FILENO, TCSANOW, &original) };
    if restored != 0 {
        eprintln!(
            "sim-flow: warning: terminal echo could not be restored. \
             Run `stty echo` to re-enable echoing.",
        );
    }
    result
}

#[cfg(not(unix))]
fn with_echo_suppressed<F, T>(f: F) -> T
where
    F: FnOnce() -> T,
{
    f()
}
