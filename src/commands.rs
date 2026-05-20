use std::path::{Path, PathBuf};

use sim_flow::__internal::config::Config;
use sim_flow::__internal::foundation_root;
use sim_flow::__internal::runner::{DOT_SIM_FLOW, StepRunner};
use sim_flow::__internal::session::protocol::SessionEndReason;
use sim_flow::__internal::session::{Event, Presenter};
use sim_flow::__internal::state::{Flow, State};
use sim_flow::__internal::steps::registry_for;

use crate::cli::{
    BaselineAction, BugsAction, Cli, Command, ConfigAction, CoverageAction, DbAction,
    EmbedderAction, KeysAction, NewKind, PlanKindArg, PromptResetScope, PromptScopeArg,
    PromptsAction, SessionMode, WatchersAction,
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
        Command::Reset { step, force } => reset(&project_dir, step, *force),
        Command::ConvertSv { force } => convert_sv(&project_dir, *force),
        Command::Bugs { action } => bugs_cmd(&project_dir, action),
        Command::Config { action } => config_cmd(&project_dir, action),
        Command::Db { action } => db_cmd(&project_dir, action),
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
        Command::PerfRun { file } => perf_run_cmd(&project_dir, file.as_deref()),
        Command::Diff { lhs, rhs } => diff_cmd(&project_dir, lhs, rhs),
        Command::PlanProgress {
            kind,
            current_step,
            all,
        } => plan_progress_cmd(&project_dir, *kind, current_step.as_deref(), *all),
        Command::Critiques { step } => critiques_cmd(&project_dir, step.as_deref()),
        Command::Documents { flow } => documents_cmd(&project_dir, flow),
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
            llm_model_family,
            llm_runtime_profile,
            llm_debug_adaptation,
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
            llm_model_family.as_deref(),
            llm_runtime_profile.as_deref(),
            *llm_debug_adaptation,
            ollama_base_url.as_deref(),
            openai_base_url.as_deref(),
            llm_base_url.as_deref(),
            candidate.as_deref(),
        ),
        Command::Auto {
            llm_backend,
            llm_model,
            llm_model_family,
            llm_runtime_profile,
            llm_debug_adaptation,
            llm_base_url,
            critique_llm_backend,
            critique_llm_model,
            critique_llm_model_family,
            critique_llm_runtime_profile,
            critique_llm_base_url,
            qa_llm_backend,
            qa_llm_model,
            qa_llm_model_family,
            qa_llm_runtime_profile,
            qa_llm_base_url,
            max_auto_iters,
            max_critique_iters,
            max_critique_no_progress_iters,
            dm0_interactive,
            spec,
            transport_socket,
            watch_socket,
            session_mode,
            step_mode,
            max_llm_requests,
            max_identical_responses,
            max_parallel_requests,
            no_preamble,
            llm_retry_budget_secs,
        } => auto_cmd(
            cli,
            &project_dir,
            llm_backend,
            llm_model.as_deref(),
            llm_model_family.as_deref(),
            llm_runtime_profile.as_deref(),
            *llm_debug_adaptation,
            llm_base_url.as_deref(),
            critique_llm_backend.as_deref(),
            critique_llm_model.as_deref(),
            critique_llm_model_family.as_deref(),
            critique_llm_runtime_profile.as_deref(),
            critique_llm_base_url.as_deref(),
            qa_llm_backend.as_deref(),
            qa_llm_model.as_deref(),
            qa_llm_model_family.as_deref(),
            qa_llm_runtime_profile.as_deref(),
            qa_llm_base_url.as_deref(),
            *max_auto_iters,
            *max_critique_iters,
            *max_critique_no_progress_iters,
            *dm0_interactive,
            spec.as_deref(),
            transport_socket.as_deref(),
            watch_socket.as_deref(),
            *session_mode,
            (*step_mode).into(),
            *max_llm_requests,
            *max_identical_responses,
            *max_parallel_requests,
            *no_preamble,
            *llm_retry_budget_secs,
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
        Command::InstallExtension {
            package_only,
            profile,
            prebuilt_binary,
            vscode_bin,
        } => install_extension_cmd(
            *package_only,
            profile,
            prebuilt_binary.as_deref(),
            vscode_bin.as_deref(),
        ),
        Command::Keys { action } => keys_cmd(action),
        Command::Watchers { action } => watchers_cmd(action),
        Command::Metrics { group_by, json } => metrics_cmd(&project_dir, *group_by, *json),
        Command::Ingest {
            source,
            peers,
            config,
            out,
            rebuild,
            status,
            rediscover_format,
            format,
            no_format_discovery,
            llm_backend,
            llm_model,
            llm_model_family,
            llm_runtime_profile,
            llm_base_url,
        } => ingest_cmd(
            &project_dir,
            source.as_deref(),
            peers.as_slice(),
            config.as_deref(),
            out.as_deref(),
            *rebuild,
            *status,
            *rediscover_format,
            format.as_deref(),
            *no_format_discovery,
            llm_backend.as_deref(),
            llm_model.as_deref(),
            llm_model_family.as_deref(),
            llm_runtime_profile.as_deref(),
            llm_base_url.as_deref(),
        ),
        Command::BuildFrameworkIndex {
            framework_root,
            out,
            embedder,
            force,
        } => crate::cli::lance_index::build_framework_index_cmd(
            &project_dir,
            framework_root.as_deref(),
            out.as_deref(),
            embedder.as_deref(),
            *force,
        ),
        Command::BuildSpecIndex {
            project,
            embedder,
            force,
            check,
        } => crate::cli::lance_index::build_spec_index_cmd(
            &project_dir,
            project.as_deref(),
            embedder.as_deref(),
            *force,
            *check,
        ),
        Command::RefreshSpec { project } => {
            crate::cli::lance_index::refresh_spec_cmd(&project_dir, project.as_deref())
        }
        Command::Embedder { action } => embedder_cmd(action),
    }
}

/// Dispatch for `sim-flow embedder <action>`. Currently only
/// `check`; new actions land as additional match arms.
fn embedder_cmd(action: &EmbedderAction) -> sim_flow::Result<()> {
    match action {
        EmbedderAction::Check { config, verbose } => {
            crate::cli::embedder::check(config.as_deref(), *verbose)
        }
    }
}

/// Driver for `sim-flow ingest`. Resolves project root, optional
/// config, and primary/peer paths into an `IngestRequest` and runs
/// the pipeline. Supports the `--rebuild` and `--status` flavours
/// per chapter 1.8 plus the Phase 9 milestone 9.6 format-discovery
/// flags (`--rediscover-format`, `--format`, `--no-format-discovery`).
#[allow(clippy::too_many_arguments)]
fn ingest_cmd(
    project_dir: &Path,
    source: Option<&Path>,
    peers: &[(String, std::path::PathBuf)],
    config: Option<&Path>,
    out: Option<&Path>,
    rebuild: bool,
    status: bool,
    rediscover_format: bool,
    format_path: Option<&Path>,
    no_format_discovery: bool,
    llm_backend: Option<&str>,
    llm_model: Option<&str>,
    llm_model_family: Option<&str>,
    llm_runtime_profile: Option<&str>,
    llm_base_url: Option<&str>,
) -> sim_flow::Result<()> {
    // Build the LLM-config view once so call sites that resolve the
    // format descriptor can hand it to `build_format_descriptor`.
    // `None` means "no LLM endpoint requested" — the discovery
    // pipeline falls back to the first-cut classifier.
    let llm_config: Option<IngestLlmConfig<'_>> = llm_backend.map(|backend| IngestLlmConfig {
        backend,
        model: llm_model,
        model_family: llm_model_family,
        runtime_profile: llm_runtime_profile,
        base_url: llm_base_url,
    });
    use sim_flow::__internal::session::spec_ingest::{
        IngestConfig, IngestRequest, PeerSpec, SourceSpec,
    };

    let project_root: std::path::PathBuf = out
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| project_dir.to_path_buf());

    if status {
        let manifest_path = project_root
            .join(".sim-flow")
            .join("spec-ingest")
            .join("manifest.toml");
        if !manifest_path.exists() {
            println!(
                "sim-flow ingest --status: no manifest at {}",
                manifest_path.display()
            );
            return Ok(());
        }
        let body = std::fs::read_to_string(&manifest_path).map_err(|e| {
            sim_flow::Error::State(format!("read {}: {e}", manifest_path.display()))
        })?;
        println!("{body}");
        return Ok(());
    }

    if format_path.is_some() && rediscover_format {
        return Err(sim_flow::Error::State(
            "sim-flow ingest: --format and --rediscover-format are mutually exclusive".into(),
        ));
    }
    if format_path.is_some() && no_format_discovery {
        return Err(sim_flow::Error::State(
            "sim-flow ingest: --format and --no-format-discovery are mutually exclusive".into(),
        ));
    }

    let cfg = match config {
        Some(p) => {
            let body = std::fs::read_to_string(p)
                .map_err(|e| sim_flow::Error::State(format!("read config {}: {e}", p.display())))?;
            IngestConfig::parse(&body)
                .map_err(|e| sim_flow::Error::State(format!("parse config: {e}")))?
        }
        None => IngestConfig::load(&project_root)?,
    };

    let primary_path: Option<std::path::PathBuf> = if rebuild {
        // Pull the source path from the existing manifest.
        let manifest_path = project_root
            .join(".sim-flow")
            .join("spec-ingest")
            .join("manifest.toml");
        let body = std::fs::read_to_string(&manifest_path).map_err(|e| {
            sim_flow::Error::State(format!(
                "rebuild: read manifest {}: {e}",
                manifest_path.display()
            ))
        })?;
        parse_manifest_source_path(&body)
    } else {
        source.map(std::path::PathBuf::from)
    };

    let request = IngestRequest {
        primary: primary_path.map(SourceSpec::new),
        peers: peers
            .iter()
            .map(|(id, path)| PeerSpec {
                id: id.clone(),
                source: SourceSpec::new(path.clone()),
            })
            .collect(),
        config: cfg,
        project_root: project_root.clone(),
    };

    let outcome = run_ingest_with_format_resolution(
        request,
        rediscover_format,
        format_path,
        no_format_discovery,
        llm_config.as_ref(),
    )?;
    println!(
        "sim-flow ingest: wrote {} ({} chunks, {} figures, {} signal tables, {} stubs, {} TBDs)",
        outcome.manifest_path.display(),
        outcome.primary_chunk_count,
        outcome.primary_figure_count,
        outcome.primary_signal_table_count,
        outcome.primary_stub_count,
        outcome.primary_tbd_count,
    );
    if !outcome.warnings.is_empty() {
        eprintln!("sim-flow ingest: {} warning(s):", outcome.warnings.len());
        for w in &outcome.warnings {
            eprintln!("  [stage {}] {}: {}", w.stage, w.code, w.message);
        }
    }
    Ok(())
}

/// Where the resolved `format.json` descriptor came from. Drives the
/// stderr diagnostic line emitted after a successful ingest.
enum FormatProvenance {
    /// Operator supplied an explicit `--format <path>`; the cache was
    /// not consulted and is not written.
    Explicit,
    /// Loaded from `.sim-flow/spec-ingest/format.json` because its
    /// `source_sha256` matched the current input.
    CacheHit,
    /// Built fresh from the deterministic first-cut classifier (LLM
    /// critique skipped: `--no-format-discovery`, no LLM resolver, or
    /// LLM endpoint unavailable). Cache file was written.
    FirstCutOnly,
    /// Built fresh through skeleton → first-cut → LLM critique → cache
    /// write. This path is not reachable today because no LLM resolver
    /// is wired into `sim-flow ingest`; the variant is retained for
    /// the follow-up milestone that lights up the LLM path.
    #[allow(dead_code)]
    Discovered,
    /// No descriptor used at all (markdown / text / empty corpus, or
    /// the source path couldn't be hashed). Pipeline runs the legacy
    /// heuristic path.
    #[allow(dead_code)]
    None,
}

/// Resolve `format.json` per the milestone 9.6 precedence and run the
/// ingest pipeline. The split mirrors the prompt's specification:
///
///   `--format <path>` > cached descriptor (matching `source_sha256`) >
///   `--rediscover-format` > built-in first-cut (when discovery is
///   skipped) > legacy heuristic path (when no source is available).
///
/// The resolved descriptor (when any) is threaded through phase B of
/// the pipeline via `pipeline::run_phase_b`, which passes it to
/// `classify_with_format` + `emit::run_with_format`. Phase A also
/// honours the descriptor when one is already known, so chrome
/// stripping picks up the descriptor's regex filters per milestone
/// 9.10.
fn run_ingest_with_format_resolution(
    request: sim_flow::__internal::session::spec_ingest::IngestRequest,
    rediscover_format: bool,
    format_path: Option<&Path>,
    no_format_discovery: bool,
    llm_config: Option<&IngestLlmConfig<'_>>,
) -> sim_flow::Result<sim_flow::__internal::session::spec_ingest::IngestOutcome> {
    use sim_flow::__internal::session::spec_ingest::format::FormatJson;
    use sim_flow::__internal::session::spec_ingest::pipeline::{run_phase_a, run_phase_b};

    // ---- Branch 1: explicit --format <path>. -------------------------
    if let Some(path) = format_path {
        let format = FormatJson::load(path)?;
        if format.schema_version != FormatJson::current_schema_version() {
            return Err(sim_flow::Error::State(format!(
                "--format {}: schema_version {} unsupported (expected {})",
                path.display(),
                format.schema_version,
                FormatJson::current_schema_version()
            )));
        }
        eprintln!(
            "sim-flow ingest: using format descriptor from {} (model={}, prompt_version={})",
            path.display(),
            format.model,
            format.prompt_version
        );
        let mut warnings = Vec::new();
        let phase_a = run_phase_a(&request, Some(&format), &mut warnings)?;
        let outcome = run_phase_b(&request, phase_a, Some(&format), warnings)?;
        emit_format_summary(&format, &FormatProvenance::Explicit, None);
        return Ok(outcome);
    }

    // ---- Branch 2: cache lookup + (maybe) discovery. -----------------
    let cache_path = request
        .project_root
        .join(".sim-flow")
        .join("spec-ingest")
        .join("format.json");

    // Compute the new source's SHA-256 if one was supplied. Markdown /
    // text / empty-corpus sources skip format discovery entirely; the
    // pipeline falls through to the heuristic path.
    let primary_path = request.primary.as_ref().map(|s| s.path.clone());
    let source_sha256 = match &primary_path {
        Some(p) if is_format_eligible(p) => sha256_of_path(p).ok(),
        _ => None,
    };

    if source_sha256.is_none() {
        // No PDF source: skip format discovery. The legacy
        // heuristic path produces the same outputs as today.
        return run_format_aware(request, None);
    }
    let source_sha256 = source_sha256.unwrap();

    // Cache hit? Reuse iff sha matches AND --rediscover-format is off.
    // The cache lives INSIDE the spec-ingest directory that emit
    // atomically replaces, so we have to re-write it after phase B
    // even on a cache hit (otherwise the old file gets wiped by the
    // directory swap and subsequent runs re-discover unnecessarily).
    if cache_path.exists() && !rediscover_format {
        match FormatJson::load(&cache_path) {
            Ok(format) if format.source_sha256 == source_sha256 => {
                eprintln!(
                    "sim-flow ingest: reusing cached format.json (model={}, prompt_version={})",
                    format.model, format.prompt_version
                );
                let outcome = run_format_aware(request, Some(&format))?;
                // Re-write the cache so emit's directory swap
                // doesn't strand us. The body is byte-identical to
                // what we just loaded so `discovered_at` doesn't
                // advance on a cache-hit run.
                if let Err(e) = format.write(&cache_path) {
                    eprintln!(
                        "sim-flow ingest: failed to re-write {} after cache hit ({e})",
                        cache_path.display()
                    );
                }
                emit_format_summary(&format, &FormatProvenance::CacheHit, Some(&cache_path));
                return Ok(outcome);
            }
            Ok(_) => {
                eprintln!(
                    "sim-flow ingest: cached format.json source_sha256 mismatch; rediscovering"
                );
            }
            Err(e) => {
                eprintln!("sim-flow ingest: cached format.json unreadable ({e}); rediscovering");
            }
        }
    }

    // Cache miss (or --rediscover-format). Build a new descriptor.
    // Run phase A first so we have a `LoadedSource` to feed the
    // skeleton builder.
    let mut warnings = Vec::new();
    let phase_a = run_phase_a(&request, None, &mut warnings)?;

    let FormatBuildOutcome { format, provenance } = build_format_descriptor(
        &phase_a.loaded,
        &request.config,
        &source_sha256,
        no_format_discovery,
        llm_config,
        Some(&request.project_root),
        &mut warnings,
    );

    // Note: phase B's emit stage replaces the entire
    // `.sim-flow/spec-ingest/` directory atomically (it removes the
    // existing dir then renames a `.tmp` sibling over it). We
    // therefore have to write the cache file AFTER phase B runs so
    // it isn't wiped by emit's directory swap.
    let outcome = run_phase_b(&request, phase_a, Some(&format), warnings)?;

    if let Some(parent) = cache_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!(
            "sim-flow ingest: failed to mkdir {} ({e}); descriptor not cached",
            parent.display()
        );
    }
    if let Err(e) = format.write(&cache_path) {
        eprintln!(
            "sim-flow ingest: failed to write {} ({e}); descriptor not cached",
            cache_path.display()
        );
    }

    emit_format_summary(&format, &provenance, Some(&cache_path));
    Ok(outcome)
}

/// Run phase A + phase B with the supplied descriptor (or `None`).
/// Helper for the cache-hit / no-source paths where phase A doesn't
/// need to be reused for skeleton building.
fn run_format_aware(
    request: sim_flow::__internal::session::spec_ingest::IngestRequest,
    format: Option<&sim_flow::__internal::session::spec_ingest::format::FormatJson>,
) -> sim_flow::Result<sim_flow::__internal::session::spec_ingest::IngestOutcome> {
    use sim_flow::__internal::session::spec_ingest::pipeline::{run_phase_a, run_phase_b};
    let mut warnings = Vec::new();
    let phase_a = run_phase_a(&request, format, &mut warnings)?;
    run_phase_b(&request, phase_a, format, warnings)
}

/// Resolved LLM-adapter configuration the ingest command builds from
/// the `--llm-*` flags / `SIM_FLOW_INGEST_LLM_*` env vars. `None` for
/// the whole struct (returned by `resolve_ingest_llm_config`) means
/// "no LLM endpoint requested" — the format-discovery pipeline falls
/// back to the first-cut classifier with a stderr hint.
struct IngestLlmConfig<'a> {
    backend: &'a str,
    model: Option<&'a str>,
    model_family: Option<&'a str>,
    runtime_profile: Option<&'a str>,
    base_url: Option<&'a str>,
}

/// Output of `build_format_descriptor` — the resolved `FormatJson`
/// plus a tag indicating which path produced it. The caller uses
/// the tag to drive the stderr diagnostic so "first-cut" only
/// appears in the diagnostic when the LLM critique actually didn't
/// run.
struct FormatBuildOutcome {
    format: sim_flow::__internal::session::spec_ingest::format::FormatJson,
    provenance: FormatProvenance,
}

/// Build a fresh `format.json` descriptor from the loaded source.
/// Always runs the deterministic first-cut classifier. The LLM
/// critique pass fires only when `no_format_discovery == false` AND
/// the caller resolved an `IngestLlmConfig` from the
/// `--llm-backend` / env vars. Critique failures (network, malformed
/// JSON, immutable-field violations) are non-fatal: the warnings
/// surface in the returned descriptor's `validation.warnings` and the
/// first-cut entries pass through unchanged.
fn build_format_descriptor(
    loaded: &sim_flow::__internal::session::spec_ingest::stages::loading::LoadedSource,
    config: &sim_flow::__internal::session::spec_ingest::IngestConfig,
    source_sha256: &str,
    no_format_discovery: bool,
    llm_config: Option<&IngestLlmConfig<'_>>,
    project_root: Option<&std::path::Path>,
    warnings: &mut Vec<sim_flow::__internal::session::spec_ingest::IngestWarning>,
) -> FormatBuildOutcome {
    use sim_flow::__internal::session::spec_ingest::format::{skeleton, validate};
    let skeleton = skeleton::build_skeleton_with(loaded, config);
    let mut outcome = build_format_descriptor_inner(
        &skeleton,
        source_sha256,
        no_format_discovery,
        llm_config,
        project_root,
        warnings,
    );
    // Phase 9.5b: deterministic validation post-pass. Re-verifies
    // the descriptor against the skeleton it was derived from and
    // populates the `validation` block with counts + structured
    // warnings. Always fires regardless of which build path
    // (default / first-cut / LLM-critiqued) produced the
    // descriptor, so the cached descriptor's `validation` block
    // matches the descriptor body.
    outcome.format.validation = validate::validate(&outcome.format, &skeleton);
    outcome
}

fn build_format_descriptor_inner(
    skeleton: &sim_flow::__internal::session::spec_ingest::format::skeleton::Skeleton,
    source_sha256: &str,
    no_format_discovery: bool,
    llm_config: Option<&IngestLlmConfig<'_>>,
    project_root: Option<&std::path::Path>,
    warnings: &mut Vec<sim_flow::__internal::session::spec_ingest::IngestWarning>,
) -> FormatBuildOutcome {
    use sim_flow::__internal::session::spec_ingest::format::{discover, first_cut};

    let mut first_cut = first_cut::classify(skeleton);
    first_cut.source_sha256 = source_sha256.to_string();

    if no_format_discovery {
        eprintln!(
            "sim-flow ingest: format discovery skipped (--no-format-discovery); \
             using first-cut classifier only"
        );
        return FormatBuildOutcome {
            format: first_cut,
            provenance: FormatProvenance::FirstCutOnly,
        };
    }

    let Some(llm) = llm_config else {
        eprintln!(
            "sim-flow ingest: no --llm-backend configured; using first-cut classifier. \
             Pass --llm-backend / --llm-base-url / --llm-model (or set \
             SIM_FLOW_INGEST_LLM_BACKEND etc.) to enable the LLM critique pass. \
             Use --no-format-discovery to suppress this warning."
        );
        return FormatBuildOutcome {
            format: first_cut,
            provenance: FormatProvenance::FirstCutOnly,
        };
    };

    let agent_config = sim_flow::__internal::session::AgentConfig {
        model: llm.model.map(String::from),
        model_family_id: llm.model_family.map(String::from),
        runtime_profile_id: llm.runtime_profile.map(String::from),
        debug_adaptation: false,
        base_url: llm.base_url.map(String::from),
        ollama_base_url: None,
        openai_base_url: None,
        cancel_flag: None,
    };
    let mut agent = match sim_flow::__internal::session::build_cli_agent(llm.backend, agent_config)
    {
        Some(a) => a,
        None => {
            eprintln!(
                "sim-flow ingest: unknown LLM backend `{}`. Available: {}. \
                 Falling back to first-cut classifier.",
                llm.backend,
                sim_flow::__internal::session::KNOWN_AGENTS.join(", ")
            );
            return FormatBuildOutcome {
                format: first_cut,
                provenance: FormatProvenance::FirstCutOnly,
            };
        }
    };

    eprintln!(
        "sim-flow ingest: running LLM critique pass via backend={} model={} base_url={}",
        llm.backend,
        llm.model.unwrap_or("(default)"),
        llm.base_url.unwrap_or("(default)"),
    );

    let first_cut_for_fallback = first_cut.clone();
    // Debug dump dir for format-discovery failures. On a malformed
    // `<patch>` block (`discover_no_patch_parsed`) or absent block
    // (`discover_failed`), the raw LLM response(s) land here so the
    // operator can inspect what the model emitted.
    //
    // Lives at `.sim-flow/debug/` — a sibling of `spec-ingest/`,
    // NOT inside it. The emit stage atomically replaces the entire
    // `spec-ingest/` directory at the end of a successful run, so
    // anything written under `spec-ingest/` during discover would be
    // overwritten before the operator could read it.
    let debug_dump_dir = project_root.map(|root| root.join(".sim-flow").join("debug"));
    match discover::discover_with_debug(
        skeleton,
        &first_cut,
        agent.as_mut(),
        warnings,
        debug_dump_dir.as_deref(),
    ) {
        Ok(mut refined) => {
            refined.source_sha256 = source_sha256.to_string();
            FormatBuildOutcome {
                format: refined,
                provenance: FormatProvenance::Discovered,
            }
        }
        Err(err) => {
            eprintln!(
                "sim-flow ingest: format-discovery LLM call failed ({err}); \
                 falling back to first-cut classifier"
            );
            FormatBuildOutcome {
                format: first_cut_for_fallback,
                provenance: FormatProvenance::FirstCutOnly,
            }
        }
    }
}

/// Whether a primary source path is a candidate for format discovery.
/// Only PDF inputs carry the structural signal the descriptor models;
/// markdown / text / unknown extensions fall through to the legacy
/// heuristic path.
fn is_format_eligible(path: &Path) -> bool {
    use sim_flow::__internal::session::spec_ingest::SourceKind;
    matches!(SourceKind::from_path(path), Some(SourceKind::Pdf))
}

/// SHA-256 of a file's bytes. Matches the digest emit.rs uses for
/// the manifest's `source_sha256` field so a cache hit lines up with
/// the manifest the same ingest run produces.
fn sha256_of_path(path: &Path) -> sim_flow::Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)
        .map_err(|e| sim_flow::Error::State(format!("sha256 read {}: {e}", path.display())))?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(format!("{:x}", h.finalize()))
}

/// Emit the milestone 9.6 stderr diagnostic block summarising the
/// resolved descriptor's classification counts. `cached_at` is the
/// path the descriptor was persisted to (or `None` when the caller
/// supplied `--format <path>` and the cache wasn't touched).
fn emit_format_summary(
    format: &sim_flow::__internal::session::spec_ingest::format::FormatJson,
    provenance: &FormatProvenance,
    cached_at: Option<&Path>,
) {
    let provenance_str = match provenance {
        FormatProvenance::Explicit => format!("explicit (model={})", format.model),
        FormatProvenance::CacheHit => format!("cache (model={})", format.model),
        FormatProvenance::FirstCutOnly => format!("first-cut (model={})", format.model),
        FormatProvenance::Discovered => format!("discovered from {}", format.model),
        FormatProvenance::None => "none".to_string(),
    };
    eprintln!("sim-flow ingest: format descriptor: {provenance_str}");
    if let Some(path) = cached_at {
        let display = path
            .strip_prefix(std::env::current_dir().unwrap_or_default())
            .unwrap_or(path);
        eprintln!("                 (cached at {})", display.display());
    }

    let mut classified: Vec<(String, u32)> = Vec::new();
    let mut unknown_tables: u32 = 0;
    for table in &format.tables {
        let kind_name = serde_json::to_value(table.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string());
        if kind_name == "unknown" {
            unknown_tables += 1;
        } else if let Some(slot) = classified.iter_mut().find(|(k, _)| k == &kind_name) {
            slot.1 += 1;
        } else {
            classified.push((kind_name, 1));
        }
    }
    let tables_classified_str = classified
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ");

    eprintln!("  section_roles: {} assigned", format.section_roles.len());
    if classified.is_empty() {
        eprintln!("  tables_classified: (none)");
    } else {
        eprintln!("  tables_classified: {tables_classified_str}");
    }
    eprintln!("  tables_unknown: {unknown_tables}");
    eprintln!("  glossary_entries: {}", format.glossary.len());
    let chrome_match_count: u32 = format.chrome.iter().map(|c| c.match_count).sum();
    eprintln!("  chrome_lines_stripped: {chrome_match_count}");
    eprintln!("  warnings: {}", format.validation.warnings.len());
}

/// Pull `source_path = "..."` out of a manifest TOML body. Returns
/// None if the field is missing or empty.
fn parse_manifest_source_path(body: &str) -> Option<std::path::PathBuf> {
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("source_path") {
            let v = rest.trim().trim_start_matches('=').trim();
            let v = v.trim_matches('"');
            if v.is_empty() {
                return None;
            }
            return Some(std::path::PathBuf::from(v));
        }
    }
    None
}

fn metrics_cmd(
    project_dir: &Path,
    group_by: crate::cli::MetricsGroupBy,
    json: bool,
) -> sim_flow::Result<()> {
    use sim_flow::__internal::session::llm_metrics::LlmMetricsRecord;
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader};

    let path = project_dir
        .join(".sim-flow")
        .join("logs")
        .join("llm-metrics.jsonl");
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "sim-flow metrics: no metrics yet at {}\n  run an `sim-flow auto` first; the orchestrator appends one row per LLM round-trip.",
                path.display()
            );
            return Ok(());
        }
        Err(err) => {
            return Err(sim_flow::Error::State(format!(
                "failed to open {}: {err}",
                path.display()
            )));
        }
    };

    let mut rows: Vec<LlmMetricsRecord> = Vec::new();
    let mut skipped = 0u64;
    for (idx, line) in BufReader::new(file).lines().enumerate() {
        let Ok(line) = line else {
            skipped += 1;
            continue;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<LlmMetricsRecord>(trimmed) {
            Ok(rec) => rows.push(rec),
            Err(err) => {
                eprintln!(
                    "sim-flow metrics: skipping malformed row {} ({err}); first 80 chars: {}",
                    idx + 1,
                    &trimmed.chars().take(80).collect::<String>(),
                );
                skipped += 1;
            }
        }
    }
    if skipped > 0 {
        eprintln!("sim-flow metrics: dropped {skipped} malformed row(s).");
    }
    if rows.is_empty() {
        println!("(no llm-metrics rows yet)");
        return Ok(());
    }

    if matches!(group_by, crate::cli::MetricsGroupBy::Raw) {
        for rec in &rows {
            println!("{}", serde_json::to_string(rec).unwrap_or_default());
        }
        return Ok(());
    }

    // Aggregate by the chosen axis. The aggregator is tiny on
    // purpose -- a single per-row pass and a BTreeMap keyed on the
    // group field. Adding more dimensions later means another key
    // tuple, not a query language.
    struct Agg {
        count: u64,
        wall_ms_total: u64,
        wall_ms_samples: Vec<u64>,
        tokens_in_total: u64,
        tokens_out_total: u64,
        prompt_bytes_total: u64,
        completion_bytes_total: u64,
        errors: u64,
    }
    impl Agg {
        fn new() -> Self {
            Self {
                count: 0,
                wall_ms_total: 0,
                wall_ms_samples: Vec::new(),
                tokens_in_total: 0,
                tokens_out_total: 0,
                prompt_bytes_total: 0,
                completion_bytes_total: 0,
                errors: 0,
            }
        }
        fn fold(&mut self, rec: &LlmMetricsRecord) {
            self.count += 1;
            self.wall_ms_total += rec.wall_ms;
            self.wall_ms_samples.push(rec.wall_ms);
            self.tokens_in_total += rec.tokens_in;
            self.tokens_out_total += rec.tokens_out;
            self.prompt_bytes_total += rec.prompt_bytes;
            self.completion_bytes_total += rec.completion_bytes;
            if rec.finish_reason.as_deref().is_some_and(|r| r == "error") {
                self.errors += 1;
            }
        }
        fn percentile(samples: &mut [u64], pct: f64) -> u64 {
            if samples.is_empty() {
                return 0;
            }
            samples.sort_unstable();
            let n = samples.len();
            // Nearest-rank, 1-indexed: ceil(pct * n) - 1
            let rank = ((pct * n as f64).ceil() as usize).clamp(1, n) - 1;
            samples[rank]
        }
    }

    let mut buckets: BTreeMap<String, Agg> = BTreeMap::new();
    for rec in &rows {
        let key = match group_by {
            crate::cli::MetricsGroupBy::Step => rec.step.clone(),
            crate::cli::MetricsGroupBy::Kind => format!("{:?}", rec.kind).to_lowercase(),
            crate::cli::MetricsGroupBy::Backend => rec.backend.clone(),
            crate::cli::MetricsGroupBy::Model => {
                rec.model.clone().unwrap_or_else(|| "(default)".to_string())
            }
            crate::cli::MetricsGroupBy::Raw => unreachable!("handled above"),
        };
        buckets.entry(key).or_insert_with(Agg::new).fold(rec);
    }

    if json {
        let rows_json: Vec<_> = buckets
            .iter_mut()
            .map(|(key, agg)| {
                let mut samples = agg.wall_ms_samples.clone();
                serde_json::json!({
                    "group": key,
                    "count": agg.count,
                    "errors": agg.errors,
                    "wall_ms_total": agg.wall_ms_total,
                    "wall_ms_p50": Agg::percentile(&mut samples, 0.50),
                    "wall_ms_p95": Agg::percentile(&mut samples, 0.95),
                    "tokens_in_total": agg.tokens_in_total,
                    "tokens_out_total": agg.tokens_out_total,
                    "prompt_bytes_total": agg.prompt_bytes_total,
                    "completion_bytes_total": agg.completion_bytes_total,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&rows_json)
                .map_err(|e| sim_flow::Error::State(format!("metrics json: {e}")))?
        );
        return Ok(());
    }

    // Human table. Columns are derived (e.g. avg ms = total / count)
    // because the row data is cheap to compute and easier to read at
    // a glance than three separate breakdown lines.
    let label = match group_by {
        crate::cli::MetricsGroupBy::Step => "STEP",
        crate::cli::MetricsGroupBy::Kind => "KIND",
        crate::cli::MetricsGroupBy::Backend => "BACKEND",
        crate::cli::MetricsGroupBy::Model => "MODEL",
        crate::cli::MetricsGroupBy::Raw => unreachable!(),
    };
    println!(
        "{:<16} {:>6} {:>7} {:>9} {:>9} {:>11} {:>11}",
        label, "TURNS", "ERRORS", "WALL_S", "AVG_S", "TOK_IN", "TOK_OUT",
    );
    println!("{}", "-".repeat(76));
    for (key, agg) in buckets {
        let avg_s = if agg.count == 0 {
            0.0
        } else {
            agg.wall_ms_total as f64 / agg.count as f64 / 1000.0
        };
        println!(
            "{:<16} {:>6} {:>7} {:>9.1} {:>9.2} {:>11} {:>11}",
            truncate_field(&key, 16),
            agg.count,
            agg.errors,
            agg.wall_ms_total as f64 / 1000.0,
            avg_s,
            agg.tokens_in_total,
            agg.tokens_out_total,
        );
    }
    Ok(())
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

fn install_extension_cmd(
    package_only: bool,
    profile: &str,
    prebuilt_binary: Option<&Path>,
    vscode_bin: Option<&Path>,
) -> sim_flow::Result<()> {
    let profile = match profile {
        "release" => sim_flow::install::Profile::Release,
        "dev" | "debug" => sim_flow::install::Profile::Dev,
        other => {
            return Err(sim_flow::Error::Config(format!(
                "unknown --profile `{other}`; expected `release` or `dev`",
            )));
        }
    };
    let sim_flow_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let opts = sim_flow::install::Options {
        sim_flow_root,
        profile,
        package_only,
        prebuilt_binary: prebuilt_binary.map(Path::to_path_buf),
        vscode_bin: vscode_bin.map(Path::to_path_buf),
    };
    sim_flow::install::install_extension(opts)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn auto_cmd(
    cli: &Cli,
    project: &Path,
    llm_backend: &str,
    llm_model: Option<&str>,
    llm_model_family: Option<&str>,
    llm_runtime_profile: Option<&str>,
    llm_debug_adaptation: bool,
    llm_base_url: Option<&str>,
    critique_llm_backend: Option<&str>,
    critique_llm_model: Option<&str>,
    critique_llm_model_family: Option<&str>,
    critique_llm_runtime_profile: Option<&str>,
    critique_llm_base_url: Option<&str>,
    qa_llm_backend: Option<&str>,
    qa_llm_model: Option<&str>,
    qa_llm_model_family: Option<&str>,
    qa_llm_runtime_profile: Option<&str>,
    qa_llm_base_url: Option<&str>,
    max_auto_iters: u32,
    max_critique_iters: u32,
    max_critique_no_progress_iters: u32,
    dm0_interactive: bool,
    spec: Option<&Path>,
    transport_socket: Option<&Path>,
    watch_socket: Option<&Path>,
    session_mode: SessionMode,
    step_mode: sim_flow::__internal::session::protocol::StepMode,
    max_llm_requests: u32,
    max_identical_responses: u32,
    max_parallel_requests: Option<u32>,
    no_preamble: bool,
    llm_retry_budget_secs: u32,
) -> sim_flow::Result<()> {
    // The transport reads the retry budget from this env var at every
    // OpenAiCompatibleRequest construction site. Forwarding the CLI
    // flag here means a single `--llm-retry-budget-secs N` covers
    // every backend invocation in the run (orchestrator, format
    // discovery, ingest) without each call site having to plumb the
    // value separately. SAFETY: set_var is unsafe in Rust 2024 because
    // env mutation isn't thread-safe; we're at single-threaded
    // process startup before any worker spawns. Running before the
    // tokio runtime / cancel-channel listener / orchestrator threads
    // start is the established pattern for env-var injection in this
    // file (matched by the existing SIM_FLOW_NO_PREAMBLE path).
    unsafe {
        std::env::set_var(
            "SIM_FLOW_RETRY_BUDGET_SECS",
            llm_retry_budget_secs.to_string(),
        );
    }
    let foundation = foundation_root::resolve(cli.foundation_root.as_deref())?;
    // Tooling preflight: DM3c shells out to `cargo llvm-cov` from
    // inside the agent's work session. If the binary isn't on PATH,
    // the agent gets a "no such command: llvm-cov" error mid-run
    // and burns LLM budget retrying. Install once at startup so the
    // tool is ready by the time the flow reaches DM3c. Failures
    // are non-fatal -- DS / DM0..DM3b don't touch llvm-cov and a
    // network outage shouldn't block them.
    ensure_llvm_cov_available();
    // SV3 (SystemVerilog Convert, Build + Validate) shells out to
    // `verilator --binary`. Probe early so a missing install surfaces
    // a copy-pasteable platform-appropriate `brew install verilator`
    // / `apt-get install verilator` hint before the agent burns
    // turns. We deliberately do NOT auto-install -- platform-specific
    // package managers vary and the user should pick.
    probe_verilator_and_warn();
    // Pre-DM0 ingestion hook used to live here, populating the
    // legacy `.sim-flow/source-spec.<ext>` + `.sim-flow/spec-pages/`
    // tree via `ingest_spec_file`. Removed once the format-discovery
    // pipeline at `.sim-flow/spec-ingest/` became the sole source-spec
    // representation. Users now run `sim-flow ingest` explicitly to
    // build the corpus before invoking `sim-flow auto`; the DM0
    // prelude reads from `.sim-flow/spec-ingest/` directly.
    if spec.is_some() {
        eprintln!(
            "sim-flow auto: --spec is no longer honored. Run `sim-flow ingest \
             --project <project> --source <spec>` first to build the \
             `.sim-flow/spec-ingest/` corpus, then invoke `sim-flow auto`."
        );
    }

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
        // Not used in interactive mode (the PTY-driven CLI agent
        // doesn't go through the JSONL host loop where the caps
        // fire).
        let _ = (
            max_auto_iters,
            max_critique_iters,
            max_critique_no_progress_iters,
        );
        return match session_mode {
            SessionMode::PerStep => sim_flow::__internal::session::run_auto_interactive(opts),
            SessionMode::Single => {
                sim_flow::__internal::session::auto_interactive::run_auto_interactive_single(opts)
            }
        };
    }

    // Shared cancellation flag, constructed BEFORE AutoOptions so
    // the orchestrator and the agent both hold clones of the same
    // `Arc`. The control-socket listener flips it on `cancel`; the
    // agent polls it during dispatch; the auto driver clears it at
    // sub-session boundaries so a Stop on one turn doesn't bleed
    // into the next.
    let cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // JSONL host path: extension drives sim-flow over stdin/stdout.
    let opts = sim_flow::__internal::session::AutoOptions {
        project_dir: project.to_path_buf(),
        foundation_root: foundation,
        llm_backend: llm_backend.to_string(),
        llm_model: llm_model.map(String::from),
        llm_model_family_id: llm_model_family.map(String::from),
        llm_runtime_profile_id: llm_runtime_profile.map(String::from),
        llm_debug_adaptation,
        llm_base_url: llm_base_url.map(String::from),
        critique_llm_backend: critique_llm_backend.map(String::from),
        critique_llm_model: critique_llm_model.map(String::from),
        critique_llm_model_family_id: critique_llm_model_family.map(String::from),
        critique_llm_runtime_profile_id: critique_llm_runtime_profile.map(String::from),
        critique_llm_base_url: critique_llm_base_url.map(String::from),
        qa_llm_backend: qa_llm_backend.map(String::from),
        qa_llm_model: qa_llm_model.map(String::from),
        qa_llm_model_family_id: qa_llm_model_family.map(String::from),
        qa_llm_runtime_profile_id: qa_llm_runtime_profile.map(String::from),
        qa_llm_base_url: qa_llm_base_url.map(String::from),
        max_auto_iters,
        max_critique_iters,
        max_critique_no_progress_iters,
        dm0_interactive,
        max_llm_requests,
        max_identical_responses,
        max_parallel_requests: resolve_max_parallel_requests(max_parallel_requests, project)?,
        step_mode,
        no_preamble,
        cancel_flag: Some(cancel_flag.clone()),
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

    // After the Presenter / LlmAdapter split the orchestrator owns
    // LLM dispatch in-process; build the agent here regardless of
    // transport. `critique_llm_*` and `qa_llm_*` overrides are
    // informational metadata only -- per-kind routing requires
    // multiple agents, which v1 of the split doesn't model.
    //
    // Cancel flag: shared between the agent (which polls it during
    // its blocking LLM call), the SocketPresenter's control-socket
    // listener (which sets it on a dashboard Stop click), and the
    // auto driver (which clears it at sub-session boundaries -- see
    // `AutoOptions::cancel_flag`). All three clone the same Arc
    // constructed earlier in this function; the reset path lives in
    // the driver because that's the only component with a coherent
    // "new sub-session is starting now" boundary. Only the socket
    // transport opens a control socket; the JSONL stdio transport
    // leaves the flag false and the agent's polling is a no-op
    // until the wire-level Cancel arrives between turns.
    let agent_config = sim_flow::__internal::session::AgentConfig {
        model: llm_model.map(String::from),
        model_family_id: llm_model_family.map(String::from),
        runtime_profile_id: llm_runtime_profile.map(String::from),
        debug_adaptation: llm_debug_adaptation,
        base_url: llm_base_url.map(String::from),
        // The auto driver's CLI surface doesn't expose the
        // per-backend ollama/openai URL knobs; `--llm-base-url`
        // covers both via `AgentConfig::base_url` precedence.
        ollama_base_url: None,
        openai_base_url: None,
        cancel_flag: Some(cancel_flag.clone()),
    };
    let mut agent = match sim_flow::__internal::session::build_cli_agent(llm_backend, agent_config)
    {
        Some(a) => a,
        None => {
            return Err(sim_flow::Error::State(format!(
                "unknown LLM backend `{llm_backend}`. Available: {}.",
                sim_flow::__internal::session::KNOWN_AGENTS.join(", "),
            )));
        }
    };
    if let Some(socket_path) = transport_socket {
        run_with_socket_session_end_cancel(socket_path, cancel_flag, |host| match watch_tap {
            Some(tap) => {
                let mut tapped = sim_flow::__internal::session::TappedPresenter::new(host, tap);
                sim_flow::__internal::session::run_auto(opts, &mut tapped, agent.as_mut())
            }
            None => sim_flow::__internal::session::run_auto(opts, host, agent.as_mut()),
        })
    } else {
        let host = sim_flow::__internal::session::JsonlHost::stdio();
        match watch_tap {
            Some(tap) => {
                let mut tapped = sim_flow::__internal::session::TappedPresenter::new(host, tap);
                sim_flow::__internal::session::run_auto(opts, &mut tapped, agent.as_mut())
            }
            None => {
                let mut host = host;
                sim_flow::__internal::session::run_auto(opts, &mut host, agent.as_mut())
            }
        }
    }
}

/// Probe `verilator --version` and emit a copy-pasteable install
/// hint when missing. Used by the SV-Convert flow's SV3 step.
/// Pure probe -- no auto-install -- because verilator package
/// names diverge across platforms (`brew install verilator` on
/// macOS, `apt-get install verilator` on debian-likes, etc.) and
/// picking wrong would surprise the user. Surfaces a warning to
/// stderr so a fresh project setup catches the gap before SV3 runs.
fn probe_verilator_and_warn() {
    use sim_flow::__internal::preflight::{
        VerilatorStatus, probe_verilator, verilator_install_hint,
    };

    match probe_verilator() {
        VerilatorStatus::Installed { version } => {
            eprintln!("sim-flow: verilator OK ({version}).");
        }
        VerilatorStatus::NotFound => {
            eprintln!(
                "sim-flow: verilator not on PATH; the SV-Convert flow's SV3 step needs it. Install with: {}",
                verilator_install_hint(),
            );
        }
    }
}

/// Resolve the effective `max_parallel_requests` knob: CLI flag wins
/// when present, else the project's `.sim-flow/config.toml::[llm]`
/// value, else 0. Surfaces a config-load failure as an error so a
/// malformed TOML fails loud rather than silently falling back to
/// the default (mirrors the rest of the orchestrator's config-load
/// posture).
fn resolve_max_parallel_requests(cli_value: Option<u32>, project: &Path) -> sim_flow::Result<u32> {
    if let Some(v) = cli_value {
        return Ok(v);
    }
    let dot = dot_dir(project);
    if !dot.join(sim_flow::__internal::config::CONFIG_FILE).exists() {
        return Ok(0);
    }
    let cfg = Config::load(&dot)?;
    Ok(cfg.llm.max_parallel_requests)
}

/// Ensure `cargo llvm-cov` is on PATH; install it (via `cargo
/// install cargo-llvm-cov --locked`) if not. The agent runs
/// `cargo llvm-cov` during DM3c (Test Execution and Coverage);
/// pre-installing here means we don't waste an LLM turn on a
/// "command not found" error and a retry. Failures are non-fatal
/// -- not every flow uses llvm-cov (DS doesn't), and a transient
/// network outage shouldn't block flows that don't need it. We
/// log a warning either way so the user can spot the problem if
/// DM3c does come around.
fn ensure_llvm_cov_available() {
    use sim_flow::__internal::preflight::{LlvmCovStatus, SystemRunner, ensure_llvm_cov_installed};

    let mut runner = SystemRunner;
    match ensure_llvm_cov_installed(&mut runner, |line| eprintln!("{line}")) {
        Ok(LlvmCovStatus::AlreadyInstalled { version }) => {
            // First-line slice is enough; some installs print
            // multi-line metadata that we don't need to spam.
            let first = version.lines().next().unwrap_or(version.as_str());
            eprintln!("sim-flow: cargo-llvm-cov OK ({first}).");
        }
        Ok(LlvmCovStatus::JustInstalled) => {
            eprintln!("sim-flow: cargo-llvm-cov installed.");
        }
        Err(reason) => {
            eprintln!(
                "sim-flow: cargo-llvm-cov install failed ({reason}); DM3c will surface a `command not found` error if/when it runs. Install manually with `cargo install cargo-llvm-cov --locked` to recover.",
            );
        }
    }
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
    llm_model_family: Option<&str>,
    llm_runtime_profile: Option<&str>,
    llm_debug_adaptation: bool,
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
        llm_model_family_id: llm_model_family.map(String::from),
        llm_runtime_profile_id: llm_runtime_profile.map(String::from),
        llm_debug_adaptation,
        ..Default::default()
    };
    // After the Presenter / LlmAdapter split the orchestrator
    // dispatches LLM calls in-process; every transport (socket /
    // jsonl / terminal) needs the agent built here from `--llm-*`.
    // Cancel flag: see `auto_cmd` for the rationale; only the socket
    // transport wires it to a control socket, the others leave it as
    // a permanently-false flag.
    let cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let agent_config = sim_flow::__internal::session::AgentConfig {
        model: llm_model.map(String::from),
        model_family_id: llm_model_family.map(String::from),
        runtime_profile_id: llm_runtime_profile.map(String::from),
        debug_adaptation: llm_debug_adaptation,
        base_url: llm_base_url.map(String::from),
        ollama_base_url: ollama_base_url.map(String::from),
        openai_base_url: openai_base_url.map(String::from),
        cancel_flag: Some(cancel_flag.clone()),
    };
    let mut agent = match sim_flow::__internal::session::build_cli_agent(llm_backend, agent_config)
    {
        Some(a) => a,
        None => {
            return Err(sim_flow::Error::State(format!(
                "unknown LLM backend `{llm_backend}`. Available: {}.",
                sim_flow::__internal::session::KNOWN_AGENTS.join(", "),
            )));
        }
    };
    if let Some(socket_path) = transport_socket {
        run_with_socket_session_end_cancel(socket_path, cancel_flag, |host| {
            sim_flow::__internal::session::run_session(opts, host, agent.as_mut())
        })
    } else if jsonl {
        let mut host = sim_flow::__internal::session::JsonlHost::stdio();
        sim_flow::__internal::session::run_session(opts, &mut host, agent.as_mut())
    } else {
        let stdin = std::io::stdin();
        let stdin_lock = stdin.lock();
        let stdout = std::io::stdout();
        let stderr = std::io::stderr();
        let mut presenter = sim_flow::__internal::session::StderrPresenter::new(
            llm_backend,
            stdin_lock,
            stdout,
            stderr,
        );
        sim_flow::__internal::session::run_session(opts, &mut presenter, agent.as_mut())
    }
}

/// Bind a SocketPresenter with a side-channel control socket at
/// `<socket_path>.control`. The dashboard can connect to that
/// out-of-band path and write `cancel\n` while the orchestrator is
/// mid-LLM-dispatch; the listener flips the shared `cancel_flag`,
/// which the agents (built in the caller's `AgentConfig`) poll on a
/// 50 ms cadence and use to abort their blocking call with
/// `Error::Cancelled`. The wrapper emits a clarifying `SessionEnd`
/// on error so terminating bugs don't leave the dashboard hanging
/// without a cause.
fn run_with_socket_session_end_cancel<F>(
    socket_path: &Path,
    cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    run: F,
) -> sim_flow::Result<()>
where
    F: FnOnce(
        &mut SessionEndTrackingPresenter<sim_flow::__internal::session::SocketPresenter>,
    ) -> sim_flow::Result<()>,
{
    let socket_host = sim_flow::__internal::session::SocketPresenter::bind_with_cancel(
        socket_path.to_path_buf(),
        cancel_flag,
    )?;
    let mut host = SessionEndTrackingPresenter::new(socket_host);
    run_with_error_session_end(&mut host, run)
}

fn run_with_error_session_end<P: Presenter, F>(
    host: &mut SessionEndTrackingPresenter<P>,
    run: F,
) -> sim_flow::Result<()>
where
    F: FnOnce(&mut SessionEndTrackingPresenter<P>) -> sim_flow::Result<()>,
{
    match run(host) {
        Ok(()) => Ok(()),
        Err(err) => {
            if !host.saw_session_end {
                let message = format!("sim-flow session failed: {err}");
                let _ = host.send(&Event::SessionEnd {
                    reason: SessionEndReason::Error,
                    message: Some(message),
                });
            }
            Err(err)
        }
    }
}

struct SessionEndTrackingPresenter<P> {
    inner: P,
    saw_session_end: bool,
}

impl<P> SessionEndTrackingPresenter<P> {
    fn new(inner: P) -> Self {
        Self {
            inner,
            saw_session_end: false,
        }
    }
}

impl<P: Presenter> Presenter for SessionEndTrackingPresenter<P> {
    fn send(&mut self, event: &Event) -> sim_flow::Result<()> {
        if matches!(event, Event::SessionEnd { .. }) {
            self.saw_session_end = true;
        }
        self.inner.send(event)
    }

    fn recv(&mut self) -> sim_flow::Result<Option<sim_flow::__internal::session::HostEvent>> {
        self.inner.recv()
    }
}

fn dot_dir(project: &Path) -> PathBuf {
    project.join(DOT_SIM_FLOW)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingPresenter {
        written: Vec<Event>,
    }

    impl Presenter for RecordingPresenter {
        fn send(&mut self, event: &Event) -> sim_flow::Result<()> {
            self.written.push(event.clone());
            Ok(())
        }

        fn recv(&mut self) -> sim_flow::Result<Option<sim_flow::__internal::session::HostEvent>> {
            Ok(None)
        }
    }

    #[test]
    fn fallback_session_end_is_emitted_on_runtime_error() {
        let mut host = SessionEndTrackingPresenter::new(RecordingPresenter::default());
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
        let mut host = SessionEndTrackingPresenter::new(RecordingPresenter::default());
        let err = run_with_error_session_end(&mut host, |host| {
            host.send(&Event::SessionEnd {
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

    // --- pure-helper tests ---

    #[test]
    fn truncate_field_passes_through_when_short() {
        assert_eq!(truncate_field("hi", 10), "hi");
        assert_eq!(truncate_field("", 10), "");
    }

    #[test]
    fn truncate_field_truncates_with_ellipsis() {
        let out = truncate_field("abcdefghij", 5);
        // Takes n-1=4 chars then appends ellipsis (U+2026, 3 bytes).
        assert_eq!(out, "abcd…");
    }

    #[test]
    fn truncate_field_counts_chars_not_bytes_for_multibyte() {
        // Two emojis = 2 chars but several bytes each. We should
        // pass through, not slice mid-codepoint.
        let out = truncate_field("🚀🎯", 5);
        assert_eq!(out, "🚀🎯");
    }

    #[test]
    fn kind_str_round_trips_the_session_kind_enum() {
        use sim_flow::__internal::client::SessionKind;
        assert_eq!(kind_str(SessionKind::Work), "work");
        assert_eq!(kind_str(SessionKind::Critique), "critique");
    }

    #[test]
    fn parse_slug_kind_splits_at_dot() {
        let (slug, kind) = parse_slug_kind("dm2d.work").unwrap();
        assert_eq!(slug, "dm2d");
        assert_eq!(kind_str(kind), "work");
        let (slug, kind) = parse_slug_kind("dm0.critique").unwrap();
        assert_eq!(slug, "dm0");
        assert_eq!(kind_str(kind), "critique");
    }

    #[test]
    fn parse_slug_kind_rejects_missing_dot() {
        let err = parse_slug_kind("dm2d-work").unwrap_err();
        assert!(format!("{err}").contains("expected `<slug>.work` or `<slug>.critique`"));
    }

    #[test]
    fn parse_slug_kind_rejects_unknown_kind() {
        let err = parse_slug_kind("dm0.review").unwrap_err();
        assert!(format!("{err}").contains("unknown kind `review`"));
    }

    #[test]
    fn dot_dir_appends_dot_sim_flow() {
        let p = std::path::Path::new("/some/project");
        assert_eq!(dot_dir(p), p.join(".sim-flow"));
    }

    #[test]
    fn current_iso8601_returns_non_empty_string() {
        let s = current_iso8601();
        assert!(!s.is_empty());
        // Should parse as a u64-ish second count (we render as
        // seconds-since-epoch, not literal ISO 8601 -- the name is
        // legacy).
        assert!(s.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn resolve_max_parallel_requests_cli_arg_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let resolved = resolve_max_parallel_requests(Some(4), tmp.path()).unwrap();
        assert_eq!(resolved, 4);
    }

    #[test]
    fn resolve_max_parallel_requests_returns_zero_when_no_config() {
        let tmp = tempfile::tempdir().unwrap();
        let resolved = resolve_max_parallel_requests(None, tmp.path()).unwrap();
        assert_eq!(resolved, 0, "no config file -> 0 default");
    }

    #[test]
    fn gate_check_to_out_renders_file_exists_variant() {
        use sim_flow::__internal::gate::GateCheck;
        let check = GateCheck::FileExists {
            path: std::path::PathBuf::from("docs/spec.md"),
            description: "spec exists".to_string(),
        };
        let out = gate_check_to_out(&check);
        assert_eq!(out.kind, "file-exists");
        assert_eq!(out.description, "spec exists");
        assert_eq!(out.path.as_deref(), Some("docs/spec.md"));
        assert!(out.pattern.is_none());
        assert!(out.cmd.is_none());
    }

    // --- init / status / advance end-to-end against a tempdir ---

    #[test]
    fn init_creates_state_toml_at_dm0() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let state_path = tmp.path().join(".sim-flow/state.toml");
        assert!(state_path.is_file());
        let body = std::fs::read_to_string(&state_path).unwrap();
        assert!(body.contains("current_step"));
        assert!(
            body.contains("DM0"),
            "DM is the head step for DMF; got:\n{body}"
        );
    }

    #[test]
    fn init_creates_state_toml_at_ds0_for_design_study() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DesignStudy).unwrap();
        let body = std::fs::read_to_string(tmp.path().join(".sim-flow/state.toml")).unwrap();
        assert!(
            body.contains("DS0"),
            "DS0 is the head step for DSF; got:\n{body}"
        );
    }

    // --- status, advance, convert_sv, reset E2E against tempdir ---

    #[test]
    fn status_after_init_succeeds() {
        // Smoke test: init then status without --json must not error.
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        status(tmp.path(), false).unwrap();
    }

    #[test]
    fn status_json_after_init_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        status(tmp.path(), true).unwrap();
    }

    #[test]
    fn status_without_state_returns_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        let result = status(tmp.path(), false);
        assert!(result.is_err());
    }

    #[test]
    fn convert_sv_without_dm4b_pass_requires_force() {
        // DM4b unpassed -> convert-sv refuses without --force.
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let err = convert_sv(tmp.path(), /*force=*/ false).unwrap_err();
        assert!(format!("{err}").contains("DM4b has not passed"));
    }

    #[test]
    fn convert_sv_no_op_when_already_in_sv_flow() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        // Mark DM4b passed and flip via convert-sv --force; then a
        // re-invocation must short-circuit without error.
        {
            let dot = tmp.path().join(".sim-flow");
            let mut state = State::load(&dot).unwrap();
            state.mark_passed("DM4b", "1");
            state.save(&dot).unwrap();
        }
        convert_sv(tmp.path(), /*force=*/ false).unwrap();
        // Second call is a no-op (already in SV flow).
        convert_sv(tmp.path(), /*force=*/ false).unwrap();
        let body = std::fs::read_to_string(tmp.path().join(".sim-flow/state.toml")).unwrap();
        // serde rename_all="kebab-case" turns SystemVerilogConvert
        // into "system-verilog-convert" in the TOML (not the
        // "systemverilog-convert" string Flow::as_str returns,
        // which is a separate display label). Match the kebab
        // form here.
        assert!(body.contains("system-verilog-convert"), "{body}");
        assert!(body.contains("SV0"));
    }

    #[test]
    fn convert_sv_from_design_study_requires_force_then_succeeds_to_caller() {
        // DS flow -> convert-sv refuses without --force.
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DesignStudy).unwrap();
        let err = convert_sv(tmp.path(), /*force=*/ false).unwrap_err();
        assert!(format!("{err}").contains("design-study flow"));

        // Known issue: with --force we return Ok and print
        // "flipped to systemverilog-convert", but the underlying
        // `state.flip_to_sv_convert` helper has an internal guard
        // that early-returns when state.flow != DirectModeling.
        // So the on-disk state.toml stays as design-study even
        // though the CLI reports success. Test asserts the caller-
        // visible contract (Ok) and notes the disk mismatch via
        // an inline TODO. Fixing requires either (a) relaxing the
        // helper's guard so DSF -> SVC works under --force, or
        // (b) erroring out of convert_sv when the helper would
        // no-op. Either way it's a behavior change worth its own
        // audit pass; out of scope for this coverage commit.
        convert_sv(tmp.path(), /*force=*/ true).unwrap();
        let body = std::fs::read_to_string(tmp.path().join(".sim-flow/state.toml")).unwrap();
        // TODO: when convert_sv/flip_to_sv_convert is reconciled,
        // flip this assertion to `assert!(body.contains("system-verilog-convert"))`.
        assert!(
            body.contains("design-study"),
            "documented current (buggy) behavior: DSF + --force prints success but state stays as design-study; got:\n{body}"
        );
    }

    #[test]
    fn reset_without_force_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let err = reset(tmp.path(), "DM0", /*force=*/ false).unwrap_err();
        assert!(format!("{err}").contains("--force"));
    }

    #[test]
    fn reset_rejects_unknown_step() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let err = reset(tmp.path(), "NotARealStep", /*force=*/ true).unwrap_err();
        assert!(format!("{err}").contains("not a"));
    }

    #[test]
    fn reset_force_clears_downstream_gates() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        // Manually mark DM0, DM1, DM2a passed, then reset to DM1.
        {
            let dot = tmp.path().join(".sim-flow");
            let mut state = State::load(&dot).unwrap();
            state.mark_passed("DM0", "1");
            state.mark_passed("DM1", "2");
            state.mark_passed("DM2a", "3");
            state.save(&dot).unwrap();
        }
        reset(tmp.path(), "DM1", /*force=*/ true).unwrap();
        let state = State::load(&tmp.path().join(".sim-flow")).unwrap();
        // DM0 (before the reset point) should still be passed;
        // DM1 + DM2a (at/after) should be cleared.
        assert!(state.gates.get("DM0").map(|g| g.passed).unwrap_or(false));
        assert!(!state.gates.get("DM1").map(|g| g.passed).unwrap_or(false));
        assert!(!state.gates.get("DM2a").map(|g| g.passed).unwrap_or(false));
    }

    #[test]
    fn gate_check_to_out_renders_every_remaining_variant() {
        use sim_flow::__internal::gate::GateCheck;

        // FileMatches: kind=file-matches, has pattern.
        let fm = GateCheck::FileMatches {
            path: std::path::PathBuf::from("docs/plan.md"),
            pattern: "(?m)^- \\[x\\]".to_string(),
            description: "plan items all checked".to_string(),
        };
        let out = gate_check_to_out(&fm);
        assert_eq!(out.kind, "file-matches");
        assert_eq!(out.path.as_deref(), Some("docs/plan.md"));
        assert_eq!(out.pattern.as_deref(), Some("(?m)^- \\[x\\]"));

        // Shell: kind=shell, has cmd + args.
        let sh = GateCheck::Shell {
            cmd: "cargo".to_string(),
            args: vec!["check".to_string(), "--manifest-path".to_string()],
            description: "cargo check is green".to_string(),
        };
        let out = gate_check_to_out(&sh);
        assert_eq!(out.kind, "shell");
        assert!(out.path.is_none());
        assert_eq!(out.cmd, Some("cargo"));
        assert_eq!(
            out.args
                .as_ref()
                .map(|a| a.iter().map(String::as_str).collect::<Vec<_>>()),
            Some(vec!["check", "--manifest-path"]),
        );

        // CritiqueClean: kind=critique-clean.
        let cc = GateCheck::CritiqueClean {
            path: std::path::PathBuf::from("docs/critiques/DM0-critique.md"),
            description: "DM0 critique has no blockers".to_string(),
        };
        let out = gate_check_to_out(&cc);
        assert_eq!(out.kind, "critique-clean");
        assert_eq!(out.path.as_deref(), Some("docs/critiques/DM0-critique.md"));

        // ExperimentsRecorded: kind=experiments-recorded; no path.
        let er = GateCheck::ExperimentsRecorded {
            description: "perf runs recorded".to_string(),
        };
        let out = gate_check_to_out(&er);
        assert_eq!(out.kind, "experiments-recorded");
        assert!(out.path.is_none());

        // MilestonesAllResolved: kind depends on placeholder_marker /
        // forbid_deferred flags.
        let mar = GateCheck::MilestonesAllResolved {
            dir: std::path::PathBuf::from("docs/plan/DM3a"),
            file_prefixes: vec!["impl-milestone-".to_string()],
            placeholder_marker: None,
            description: "all impl milestones resolved".to_string(),
            forbid_deferred: false,
        };
        let out = gate_check_to_out(&mar);
        assert_eq!(out.kind, "milestones-all-resolved");
        assert_eq!(out.pattern.as_deref(), Some("impl-milestone-"));

        let mar_detailed = GateCheck::MilestonesAllResolved {
            dir: std::path::PathBuf::from("docs/plan/DM2c"),
            file_prefixes: vec!["impl-milestone-".to_string()],
            placeholder_marker: Some("<DETAIL_HERE>".to_string()),
            description: "all impl milestones detailed".to_string(),
            forbid_deferred: false,
        };
        assert_eq!(
            gate_check_to_out(&mar_detailed).kind,
            "milestones-all-detailed"
        );

        let mar_no_defer = GateCheck::MilestonesAllResolved {
            dir: std::path::PathBuf::from("docs/plan/DM3a"),
            file_prefixes: vec!["impl-milestone-".to_string()],
            placeholder_marker: None,
            description: "all impl milestones implemented".to_string(),
            forbid_deferred: true,
        };
        assert_eq!(
            gate_check_to_out(&mar_no_defer).kind,
            "milestones-all-implemented"
        );

        // AnyExists: kind=any-exists; path is the | -joined list.
        let ae = GateCheck::AnyExists {
            paths: vec![
                std::path::PathBuf::from("docs/critiques/DM0-critique.md"),
                std::path::PathBuf::from("docs/critiques/DM0-critique.json"),
            ],
            description: "either critique file exists".to_string(),
        };
        let out = gate_check_to_out(&ae);
        assert_eq!(out.kind, "any-exists");
        assert!(out.path.as_deref().unwrap().contains(" | "));

        // AnyMatches: kind=any-matches; path AND pattern set.
        let am = GateCheck::AnyMatches {
            paths: vec![std::path::PathBuf::from(
                "docs/plan/DM3a/impl-milestone-01.md",
            )],
            pattern: "RESOLVED".to_string(),
            description: "at least one milestone resolved".to_string(),
        };
        let out = gate_check_to_out(&am);
        assert_eq!(out.kind, "any-matches");
        assert_eq!(out.pattern.as_deref(), Some("RESOLVED"));
    }

    #[test]
    fn bugs_cmd_list_with_empty_log_prints_summary_and_returns_ok() {
        let tmp = tempfile::tempdir().unwrap();
        // No .sim-flow/bug-log.jsonl on disk -> load_all returns [].
        let r = bugs_cmd(
            tmp.path(),
            &crate::cli::BugsAction::List {
                open: false,
                resolved: false,
                step: None,
                category: None,
            },
        );
        assert!(r.is_ok());
    }

    #[test]
    fn bugs_cmd_list_filters_by_open_resolved_step_category() {
        use sim_flow::__internal::bug_log;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".sim-flow")).unwrap();

        // Open one bug at DM0, category=correctness.
        let id1 = bug_log::open(tmp.path(), "DM0", None, "correctness", "issue 1").unwrap();
        // Open another at DM1, category=compile_error.
        let _id2 = bug_log::open(tmp.path(), "DM1", None, "compile_error", "issue 2").unwrap();
        // Resolve the first.
        bug_log::resolve(tmp.path(), &id1, "fixed it", None).unwrap();

        // Filter: only open.
        let r = bugs_cmd(
            tmp.path(),
            &crate::cli::BugsAction::List {
                open: true,
                resolved: false,
                step: None,
                category: None,
            },
        );
        assert!(r.is_ok());

        // Filter: only resolved.
        let r = bugs_cmd(
            tmp.path(),
            &crate::cli::BugsAction::List {
                open: false,
                resolved: true,
                step: None,
                category: None,
            },
        );
        assert!(r.is_ok());

        // Filter: step.
        let r = bugs_cmd(
            tmp.path(),
            &crate::cli::BugsAction::List {
                open: false,
                resolved: false,
                step: Some("DM1".to_string()),
                category: None,
            },
        );
        assert!(r.is_ok());

        // Filter: category.
        let r = bugs_cmd(
            tmp.path(),
            &crate::cli::BugsAction::List {
                open: false,
                resolved: false,
                step: None,
                category: Some("compile_error".to_string()),
            },
        );
        assert!(r.is_ok());
    }

    #[test]
    fn bugs_cmd_show_renders_full_record_for_known_id() {
        use sim_flow::__internal::bug_log;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".sim-flow")).unwrap();
        let id = bug_log::open(
            tmp.path(),
            "DM2c",
            Some("test-milestone-03"),
            "test_failure",
            "test_x panics on overflow",
        )
        .unwrap();
        bug_log::append_event(
            tmp.path(),
            &id,
            bug_log::BugEvent {
                ts: "1700000000".to_string(),
                kind: "hypothesis".to_string(),
                rationale: Some("off-by-one in indexing".to_string()),
                outcome: None,
                message: None,
            },
        )
        .unwrap();
        bug_log::resolve(tmp.path(), &id, "fixed bounds check", None).unwrap();

        let r = bugs_cmd(tmp.path(), &crate::cli::BugsAction::Show { id });
        assert!(r.is_ok());
    }

    #[test]
    fn bugs_cmd_show_returns_state_error_for_unknown_id() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".sim-flow")).unwrap();
        let r = bugs_cmd(
            tmp.path(),
            &crate::cli::BugsAction::Show {
                id: "bug-999".to_string(),
            },
        );
        let err = r.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("bug-999"), "error should mention id: {msg}");
    }

    #[test]
    fn prompt_scope_for_maps_each_variant() {
        use sim_flow::__internal::prompts::PromptScope;
        assert!(matches!(
            prompt_scope_for(crate::cli::PromptScopeArg::Project),
            PromptScope::Project
        ));
        assert!(matches!(
            prompt_scope_for(crate::cli::PromptScopeArg::Global),
            PromptScope::Global
        ));
    }

    #[test]
    fn plan_progress_rejects_more_than_one_mode_flag() {
        let tmp = tempfile::tempdir().unwrap();
        // --kind impl AND --current-step (illegal combo)
        let r = plan_progress_cmd(
            tmp.path(),
            Some(crate::cli::PlanKindArg::Impl),
            Some("DM3a"),
            false,
        );
        let err = r.unwrap_err();
        assert!(format!("{err}").contains("mutually exclusive"));
        // --all AND --kind impl (illegal combo)
        let r = plan_progress_cmd(tmp.path(), Some(crate::cli::PlanKindArg::Impl), None, true);
        assert!(r.is_err());
    }

    #[test]
    fn plan_progress_requires_some_mode_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let r = plan_progress_cmd(tmp.path(), None, None, false);
        let err = r.unwrap_err();
        assert!(format!("{err}").contains("must pass one"));
    }

    #[test]
    fn plan_progress_all_mode_succeeds_on_an_empty_project() {
        // No docs/plan present -- read_all_plan_progress should
        // return an empty report and the command writes valid JSON.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".sim-flow")).unwrap();
        // Project state is required by some readers; create a minimal one.
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let r = plan_progress_cmd(tmp.path(), None, None, true);
        assert!(r.is_ok());
    }

    #[test]
    fn plan_progress_kind_mode_succeeds_for_each_plan_kind_arg() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        for k in [
            crate::cli::PlanKindArg::Impl,
            crate::cli::PlanKindArg::Test,
            crate::cli::PlanKindArg::Perf,
        ] {
            assert!(plan_progress_cmd(tmp.path(), Some(k), None, false).is_ok());
        }
    }

    #[test]
    fn plan_progress_current_step_infers_kind_via_plan_kind_for_step() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        // DM3a is an impl step in DMF -- plan_kind_for_step returns
        // Impl. The command should still succeed even with no plan
        // files on disk; the reader emits an empty report.
        assert!(plan_progress_cmd(tmp.path(), None, Some("DM3a"), false).is_ok());
    }

    #[test]
    fn documents_cmd_rejects_unknown_flow_id() {
        let tmp = tempfile::tempdir().unwrap();
        let r = documents_cmd(tmp.path(), "bogus-flow");
        assert!(r.is_err());
    }

    #[test]
    fn documents_cmd_succeeds_on_a_freshly_inited_dm_project() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        assert!(documents_cmd(tmp.path(), "direct-modeling").is_ok());
    }

    #[test]
    fn critiques_cmd_with_no_critiques_returns_ok() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        // No docs/critiques on disk -- prints "(no critiques)" and ok.
        assert!(critiques_cmd(tmp.path(), None).is_ok());
        // With step filter for a step that has no critique file.
        assert!(critiques_cmd(tmp.path(), Some("DM0")).is_ok());
    }

    #[test]
    fn read_jsonl_lines_skips_blank_and_strips_whitespace() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("log.jsonl");
        std::fs::write(&path, "  {\"a\":1}\n\n {\"b\":2}  \n   \n{\"c\":3}\n").unwrap();
        let lines = read_jsonl_lines(&path).unwrap();
        assert_eq!(lines, vec!["{\"a\":1}", "{\"b\":2}", "{\"c\":3}"]);
    }

    #[test]
    fn read_jsonl_lines_missing_path_returns_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nope.jsonl");
        let err = read_jsonl_lines(&path).unwrap_err();
        assert!(matches!(err, sim_flow::Error::Io { .. }));
    }

    #[test]
    fn parse_jsonl_record_returns_none_for_malformed_input() {
        #[derive(serde::Deserialize)]
        struct Tiny {
            #[allow(dead_code)]
            n: u32,
        }
        assert!(parse_jsonl_record::<Tiny>("{\"n\":7}").is_some());
        // Missing required field -- returns None and logs a warn.
        assert!(parse_jsonl_record::<Tiny>("{}").is_none());
        // Not JSON at all.
        assert!(parse_jsonl_record::<Tiny>("not json").is_none());
    }

    #[test]
    fn config_cmd_show_prints_default_config_toml() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let r = config_cmd(tmp.path(), &crate::cli::ConfigAction::Show);
        assert!(r.is_ok());
    }

    #[test]
    fn config_cmd_show_without_config_falls_back_to_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        // No .sim-flow/config.toml -- Config::load returns Ok(default).
        let r = config_cmd(tmp.path(), &crate::cli::ConfigAction::Show);
        assert!(r.is_ok());
    }

    #[test]
    fn runs_cmd_on_empty_index_returns_ok() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        // No runs recorded -- prints "(no runs match the filter)" and ok.
        assert!(runs_cmd(tmp.path(), None, None, None, None, 100, false).is_ok());
        // Also via JSON.
        assert!(runs_cmd(tmp.path(), None, None, None, None, 100, true).is_ok());
    }

    #[test]
    fn runs_cmd_with_filters_on_empty_index_is_still_ok() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        assert!(
            runs_cmd(
                tmp.path(),
                Some("throughput"),
                Some("mesh"),
                Some("noc"),
                Some("001-parent"),
                5,
                false,
            )
            .is_ok()
        );
    }

    #[test]
    fn baseline_cmd_list_on_empty_index_returns_ok() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        // No baselines pinned -- prints nothing and returns ok.
        assert!(
            baseline_cmd(
                tmp.path(),
                &crate::cli::BaselineAction::List { json: false }
            )
            .is_ok()
        );
        assert!(baseline_cmd(tmp.path(), &crate::cli::BaselineAction::List { json: true }).is_ok());
    }

    #[test]
    fn metrics_cmd_with_no_log_file_returns_ok_with_no_op_message() {
        let tmp = tempfile::tempdir().unwrap();
        // No .sim-flow/logs/llm-metrics.jsonl on disk.
        for group_by in [
            crate::cli::MetricsGroupBy::Raw,
            crate::cli::MetricsGroupBy::Step,
            crate::cli::MetricsGroupBy::Kind,
            crate::cli::MetricsGroupBy::Backend,
            crate::cli::MetricsGroupBy::Model,
        ] {
            assert!(metrics_cmd(tmp.path(), group_by, false).is_ok());
        }
    }

    #[test]
    fn metrics_cmd_skips_malformed_rows_and_reports_count() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".sim-flow").join("logs");
        std::fs::create_dir_all(&dir).unwrap();
        // Empty file -> "no rows" branch.
        std::fs::write(dir.join("llm-metrics.jsonl"), "").unwrap();
        assert!(metrics_cmd(tmp.path(), crate::cli::MetricsGroupBy::Raw, false).is_ok());
        // File with only malformed lines -> skipped path; no rows.
        std::fs::write(
            dir.join("llm-metrics.jsonl"),
            "{not json}\n{also not json}\n",
        )
        .unwrap();
        assert!(metrics_cmd(tmp.path(), crate::cli::MetricsGroupBy::Raw, false).is_ok());
    }

    #[test]
    fn run_step_gate_only_unknown_step_returns_invalid_step_error() {
        use clap::Parser;
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let cli = crate::cli::Cli::try_parse_from(["sim-flow", "status"]).unwrap();
        let err = run_step(&cli, tmp.path(), Some("DM-zzz"), None, true, false).unwrap_err();
        assert!(format!("{err}").contains("not a"), "{err}");
    }

    #[test]
    fn run_step_gate_only_failing_gate_surfaces_gate_error() {
        use clap::Parser;
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let cli = crate::cli::Cli::try_parse_from(["sim-flow", "status"]).unwrap();
        // DM0 has critique-clean which fails (no critique present).
        let err = run_step(&cli, tmp.path(), Some("DM0"), None, true, false).unwrap_err();
        assert!(format!("{err}").contains("failed"), "{err}");
        // Same in JSON mode.
        let err = run_step(&cli, tmp.path(), Some("DM0"), None, true, true).unwrap_err();
        assert!(format!("{err}").contains("failed"), "{err}");
    }

    #[test]
    fn describe_rejects_step_kind_without_dot() {
        use clap::Parser;
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let cli = crate::cli::Cli::try_parse_from(["sim-flow", "status"]).unwrap();
        let err = describe(&cli, tmp.path(), "DM0-work", false).unwrap_err();
        assert!(format!("{err}").contains("<step>.<kind>"));
    }

    #[test]
    fn describe_rejects_unknown_kind_string() {
        use clap::Parser;
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let cli = crate::cli::Cli::try_parse_from(["sim-flow", "status"]).unwrap();
        let err = describe(&cli, tmp.path(), "DM0.review", false).unwrap_err();
        assert!(format!("{err}").contains("unknown session kind"));
    }

    #[test]
    fn describe_rejects_unknown_step_id() {
        use clap::Parser;
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let cli = crate::cli::Cli::try_parse_from(["sim-flow", "status"]).unwrap();
        let err = describe(&cli, tmp.path(), "DM-zzz.work", false).unwrap_err();
        assert!(format!("{err}").contains("not a"));
    }

    #[test]
    fn record_run_cmd_writes_a_row_to_the_experiments_db() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let r = record_run_cmd(
            tmp.path(),
            "baseline",
            Some("throughput"),
            Some("mesh"),
            Some("noc"),
            None,
            Some("first run"),
        );
        assert!(r.is_ok());
        // Now `runs list` should report 1 row.
        let dot = dot_dir(tmp.path());
        let index = sim_flow::__internal::tracking::index::ExperimentIndex::open(&dot).unwrap();
        assert_eq!(index.count_runs().unwrap(), 1);
    }

    #[test]
    fn diff_cmd_with_two_unknown_run_ids_returns_config_error() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        // No runs in the index -> both lhs and rhs lookups fail.
        let r = diff_cmd(tmp.path(), "no-such-a", "no-such-b");
        assert!(r.is_err());
    }

    #[test]
    fn sweep_cmd_with_missing_toml_returns_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let r = sweep_cmd(tmp.path(), &tmp.path().join("no-such.toml"));
        assert!(r.is_err());
    }

    #[test]
    fn advance_unknown_step_returns_invalid_step_error() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let err = advance(tmp.path(), Some("DM-zzz"), None, false).unwrap_err();
        assert!(format!("{err}").contains("not a"), "{err}");
    }

    #[test]
    fn advance_refuses_per_candidate_steps_or_candidate_arg() {
        // DM doesn't have per_candidate steps in the registry; pass
        // an explicit candidate arg to trip the per-candidate guard.
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let err = advance(tmp.path(), Some("DM0"), Some("mesh"), false).unwrap_err();
        assert!(format!("{err}").contains("per-candidate"), "{err}");
    }

    #[test]
    fn advance_failing_gate_returns_gate_error_in_text_and_json_modes() {
        // DM0's gate-checks include critique-clean: at fresh init there
        // is no critique file, so the gate will fail.
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        // text mode
        let err = advance(tmp.path(), None, None, false).unwrap_err();
        assert!(format!("{err}").contains("failed"), "{err}");
        // json mode
        let err = advance(tmp.path(), None, None, true).unwrap_err();
        assert!(format!("{err}").contains("failed"), "{err}");
    }

    #[test]
    fn emit_gate_json_serializes_a_failing_report() {
        use sim_flow::__internal::gate::{GateFailure, GateReport};
        let report = GateReport {
            failures: vec![GateFailure {
                description: "spec exists".into(),
                reason: "missing".into(),
            }],
        };
        // Should not error -- just exercise serde path.
        assert!(emit_gate_json("DM0", &report).is_ok());
    }

    #[test]
    fn new_cmd_study_and_candidate_are_not_yet_implemented() {
        // Phase 5 lands these; for now they return State errors so
        // callers see "not yet implemented" rather than a panic.
        use clap::Parser;
        let tmp = tempfile::tempdir().unwrap();
        let cli = crate::cli::Cli::try_parse_from(["sim-flow", "status"]).unwrap();
        let r = new_cmd(
            &cli,
            tmp.path(),
            &crate::cli::NewKind::Study {
                name: "noc".to_string(),
            },
        );
        assert!(r.is_err());
        let r = new_cmd(
            &cli,
            tmp.path(),
            &crate::cli::NewKind::Candidate {
                name: "mesh".to_string(),
            },
        );
        assert!(r.is_err());
    }

    #[test]
    fn watchers_cmd_list_with_no_watchers_returns_ok_in_both_modes() {
        // Without an active sim-flow auto session, the watch registry
        // is empty -- list prints the (no live watchers) note in
        // text mode and an empty array in JSON mode.
        assert!(watchers_cmd(&crate::cli::WatchersAction::List { json: false }).is_ok());
        assert!(watchers_cmd(&crate::cli::WatchersAction::List { json: true }).is_ok());
    }

    #[test]
    fn sweep_results_cmd_with_no_runs_returns_ok() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        // No sweep child runs in the index -- prints summary and ok.
        assert!(sweep_results_cmd(tmp.path(), "001-no-such-parent").is_ok());
    }

    #[test]
    fn coverage_cmd_show_uses_defaults_when_config_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let r = coverage_cmd(
            tmp.path(),
            &crate::cli::CoverageAction::Show { json: false },
        );
        assert!(r.is_ok());
        let r = coverage_cmd(tmp.path(), &crate::cli::CoverageAction::Show { json: true });
        assert!(r.is_ok());
    }

    #[test]
    fn coverage_cmd_set_writes_threshold_and_level_to_config() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let r = coverage_cmd(
            tmp.path(),
            &crate::cli::CoverageAction::Set {
                threshold_pct: Some(80.0),
                level: Some(crate::cli::actions::CoverageLevelArg::Total),
            },
        );
        assert!(r.is_ok());
        // Round-trip: the saved value should now appear under coverage.
        let cfg =
            sim_flow::__internal::config::Config::load(&tmp.path().join(".sim-flow")).unwrap();
        assert!((cfg.coverage.threshold_pct - 80.0).abs() < 0.01);
    }

    #[test]
    fn coverage_cmd_set_clamps_threshold_above_one_hundred() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let r = coverage_cmd(
            tmp.path(),
            &crate::cli::CoverageAction::Set {
                threshold_pct: Some(9000.0),
                level: None,
            },
        );
        assert!(r.is_ok());
        let cfg =
            sim_flow::__internal::config::Config::load(&tmp.path().join(".sim-flow")).unwrap();
        assert!(cfg.coverage.threshold_pct <= 100.0 + 0.01);
    }

    #[test]
    fn baseline_cmd_create_without_any_runs_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        // No runs in the index to pin against.
        let r = baseline_cmd(
            tmp.path(),
            &crate::cli::BaselineAction::Create {
                name: "v1".to_string(),
                run: None,
                notes: None,
                json: false,
            },
        );
        assert!(r.is_err());
    }

    #[test]
    fn init_overwrites_existing_state_toml() {
        // Current contract: `sim-flow init` is unconditional --
        // running it on an existing project resets state.toml to
        // the head step. This is intentional today (no --force
        // gate like reset has) so newcomers can re-init a stale
        // checkout, but it's worth flagging in the audit log as a
        // candidate for the next destructive-action pass.
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let dot = tmp.path().join(".sim-flow/state.toml");
        // Mutate state externally to simulate progress past DM0.
        let body = std::fs::read_to_string(&dot).unwrap();
        std::fs::write(&dot, body.replace("DM0", "DM2c")).unwrap();
        // Re-running init clobbers back to DM0.
        init(tmp.path(), Flow::DirectModeling).unwrap();
        let after = std::fs::read_to_string(&dot).unwrap();
        assert!(
            after.contains("DM0"),
            "init did not reset to DM0; got:\n{after}"
        );
        assert!(
            !after.contains("DM2c"),
            "init did not clobber DM2c; got:\n{after}"
        );
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
        Flow::SystemVerilogConvert => "SV0",
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
        SpecMdStructured {
            spec_md_path,
            manifest_path,
            description,
        } => GateCheckOut {
            kind: "spec-md-structured",
            description,
            path: Some(match manifest_path {
                Some(m) => format!("{} | manifest={}", spec_md_path.display(), m.display()),
                None => spec_md_path.display().to_string(),
            }),
            pattern: None,
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

/// Flip the project from DirectModeling into SystemVerilogConvert.
/// Archives the DMF gate history under `state.archived_gates["dm"]`
/// (visible via `sim-flow status`) and parks `current_step` at SV0.
/// After this, `sim-flow auto` drives SV0 → SV0d → SV1 → SV2 → SV3.
///
/// Refuses to flip unless DM4b has passed (the SV-Convert prereq);
/// `--force` bypasses the check for tests / advanced workflows. The
/// flip is in-place destructive on gate flags (archived, not lost),
/// so the precondition is the safety net.
fn convert_sv(project: &Path, force: bool) -> sim_flow::Result<()> {
    let dot = dot_dir(project);
    let mut state = State::load(&dot)?;
    match state.flow {
        Flow::SystemVerilogConvert => {
            println!(
                "convert-sv: project is already in the systemverilog-convert flow (current_step = {}). Nothing to do.",
                state.current_step,
            );
            return Ok(());
        }
        Flow::DirectModeling => {}
        Flow::DesignStudy => {
            if !force {
                return Err(sim_flow::Error::State(
                    "convert-sv: project is in the design-study flow; finish DM (or pass `--force`) before flipping to systemverilog-convert"
                        .into(),
                ));
            }
        }
    }
    if !force && state.gates.get("DM4b").map(|g| !g.passed).unwrap_or(true) {
        return Err(sim_flow::Error::State(
            "convert-sv: DM4b has not passed; finish the DirectModeling flow before flipping to systemverilog-convert, or pass `--force` to override".into(),
        ));
    }
    state.flip_to_sv_convert("SV0");
    state.save(&dot)?;
    println!(
        "convert-sv: flipped to systemverilog-convert; current_step = SV0. DM gate history archived under state.archived_gates[\"dm\"]."
    );
    println!("Next: `sim-flow auto` to drive SV0 → SV0d → SV1 → SV2 → SV3.");
    Ok(())
}

/// `sim-flow bugs list / show` dispatcher.
fn bugs_cmd(project: &Path, action: &BugsAction) -> sim_flow::Result<()> {
    use sim_flow::__internal::bug_log;
    match action {
        BugsAction::List {
            open,
            resolved,
            step,
            category,
        } => {
            let records = bug_log::load_all(project);
            let filtered: Vec<&bug_log::BugRecord> = records
                .iter()
                .filter(|r| {
                    let status_match = if *open {
                        r.status == "open"
                    } else if *resolved {
                        r.status != "open"
                    } else {
                        true
                    };
                    let step_match = step.as_deref().map(|s| r.step == s).unwrap_or(true);
                    let cat_match = category.as_deref().map(|c| r.category == c).unwrap_or(true);
                    status_match && step_match && cat_match
                })
                .collect();
            if filtered.is_empty() {
                println!(
                    "(no bugs match the filter; bug-log.jsonl has {} total entries)",
                    records.len()
                );
                return Ok(());
            }
            // Plain text table. Columns sized to common content;
            // `issue` truncates at 72 chars.
            println!(
                "{:<8} {:<6} {:<9} {:<10} ISSUE",
                "ID", "STEP", "CATEGORY", "STATUS"
            );
            for rec in filtered {
                let issue = {
                    let mut iter = rec.issue.chars();
                    let head: String = iter.by_ref().take(69).collect();
                    if iter.next().is_some() {
                        format!("{head}...")
                    } else {
                        rec.issue.clone()
                    }
                };
                println!(
                    "{:<8} {:<6} {:<9} {:<10} {}",
                    rec.id, rec.step, rec.category, rec.status, issue
                );
            }
            Ok(())
        }
        BugsAction::Show { id } => {
            let records = bug_log::load_all(project);
            let Some(rec) = records.iter().find(|r| r.id == *id) else {
                return Err(sim_flow::Error::State(format!(
                    "bugs show: no bug with id `{id}` in `.sim-flow/bug-log.jsonl`"
                )));
            };
            println!("ID:         {}", rec.id);
            println!("Step:       {}", rec.step);
            if let Some(m) = &rec.milestone {
                println!("Milestone:  {m}");
            }
            println!("Category:   {}", rec.category);
            println!("Status:     {}", rec.status);
            println!("Opened:     {}", rec.opened_at);
            if let Some(c) = &rec.closed_at {
                println!("Closed:     {c}");
            }
            println!();
            println!("Issue:");
            println!("  {}", rec.issue);
            if !rec.events.is_empty() {
                println!();
                println!("Events ({}):", rec.events.len());
                for ev in &rec.events {
                    let body = match ev.kind.as_str() {
                        "hypothesis" | "fix_attempt" => ev
                            .rationale
                            .as_deref()
                            .unwrap_or("<no rationale>")
                            .to_string(),
                        "expectation_nudge" => {
                            ev.message.as_deref().unwrap_or("<no message>").to_string()
                        }
                        _ => ev
                            .rationale
                            .as_deref()
                            .or(ev.message.as_deref())
                            .unwrap_or("<no body>")
                            .to_string(),
                    };
                    let outcome_suffix = ev
                        .outcome
                        .as_deref()
                        .map(|o| format!(" [{o}]"))
                        .unwrap_or_default();
                    println!("  [{}] {}{}: {}", ev.ts, ev.kind, outcome_suffix, body);
                }
            }
            if let Some(r) = &rec.resolution {
                println!();
                println!("Resolution:");
                println!("  {r}");
            }
            Ok(())
        }
    }
}

fn reset(project: &Path, step_id: &str, force: bool) -> sim_flow::Result<()> {
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
    // Gate the destructive cleanup behind --force. Resetting `DM2a`
    // from `DM4b` deletes every downstream artifact (the entire
    // model + testbench + perf body of work) with no opportunity to
    // abort. A misclick from the dashboard's Reset button hit the
    // same path. See orchestrator audit #15 (2026-05-16).
    if !force {
        let cleared = order.len() - idx;
        return Err(sim_flow::Error::InvalidStep(format!(
            "reset: refusing to delete {cleared} step(s) of artifacts from `{step_id}` forward without `--force`. Re-run with `sim-flow reset {step_id} --force` to confirm."
        )));
    }
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

fn perf_run_cmd(project: &Path, file: Option<&Path>) -> sim_flow::Result<()> {
    use sim_flow::__internal::tracking::{perf_plan, perf_run, variants};

    let plan_path = match file {
        Some(p) => p.to_path_buf(),
        None => project.join(perf_plan::DEFAULT_PLAN_PATH),
    };
    let plan = perf_plan::load(&plan_path)?;
    let variants_manifest = variants::load_project(project)?;
    // Cross-validate the plan against the manifest if one exists.
    if let Some(ref manifest) = variants_manifest {
        plan.validate(Some(manifest))
            .map_err(|err| sim_flow::Error::Config(format!("{}: {err}", plan_path.display())))?;
    }
    let results = perf_run::run(project, &plan, variants_manifest.as_ref())?;
    println!(
        "perf-run complete: {} studies, {} total runs{}",
        results.studies.len(),
        results.total_runs,
        if results.budget_reached {
            format!(" (budget cap {} reached)", plan.plan.budget_runs)
        } else {
            String::new()
        }
    );
    for study in &results.studies {
        println!(
            "  [{}] parent={} cells={}",
            study.study_name,
            study.parent_run_id,
            study.cells.len()
        );
    }
    Ok(())
}

fn diff_cmd(project: &Path, lhs: &str, rhs: &str) -> sim_flow::Result<()> {
    use sim_flow::__internal::tracking::diff;
    let report = diff::run(project, lhs, rhs)?;
    print!("{}", diff::render_markdown(&report));
    Ok(())
}

fn plan_progress_cmd(
    project: &Path,
    kind: Option<PlanKindArg>,
    current_step: Option<&str>,
    all: bool,
) -> sim_flow::Result<()> {
    use sim_flow::__internal::plan_progress::{self, PlanKind};
    let supplied = [kind.is_some(), current_step.is_some(), all]
        .iter()
        .filter(|x| **x)
        .count();
    if supplied > 1 {
        return Err(sim_flow::Error::Config(
            "plan-progress: --kind, --current-step, and --all are mutually exclusive".into(),
        ));
    }
    if all {
        let report = plan_progress::read_all_plan_progress(project);
        let json = serde_json::to_string_pretty(&report)
            .map_err(|err| sim_flow::Error::Config(format!("serialize plan progress: {err}")))?;
        println!("{json}");
        return Ok(());
    }
    let pk = match (kind, current_step) {
        (Some(k), _) => match k {
            PlanKindArg::Impl => PlanKind::Impl,
            PlanKindArg::Test => PlanKind::Test,
            PlanKindArg::Perf => PlanKind::Perf,
        },
        (None, Some(step)) => plan_progress::plan_kind_for_step(step),
        (None, None) => {
            return Err(sim_flow::Error::Config(
                "plan-progress: must pass one of --kind, --current-step, or --all".into(),
            ));
        }
    };
    let report = plan_progress::read_plan_progress_for_kind(project, pk);
    let json = serde_json::to_string_pretty(&report)
        .map_err(|err| sim_flow::Error::Config(format!("serialize plan progress: {err}")))?;
    println!("{json}");
    Ok(())
}

fn documents_cmd(project: &Path, flow: &str) -> sim_flow::Result<()> {
    use sim_flow::__internal::documents;
    documents::validate_flow(flow)?;
    let docs = documents::enumerate_project_documents(project, flow);
    let json = serde_json::to_string_pretty(&docs)
        .map_err(|err| sim_flow::Error::Config(format!("serialize documents: {err}")))?;
    println!("{json}");
    Ok(())
}

fn critiques_cmd(project: &Path, step: Option<&str>) -> sim_flow::Result<()> {
    use sim_flow::__internal::critique;
    match step {
        Some(step_id) => {
            let entry = critique::read_critique_entry(project, step_id)?;
            let json = serde_json::to_string_pretty(&entry)
                .map_err(|err| sim_flow::Error::Config(format!("serialize critique: {err}")))?;
            println!("{json}");
        }
        None => {
            let entries = critique::list_critique_entries(project)?;
            let json = serde_json::to_string_pretty(&entries)
                .map_err(|err| sim_flow::Error::Config(format!("serialize critiques: {err}")))?;
            println!("{json}");
        }
    }
    Ok(())
}

/// `sim-flow db <action>` -- introspection over the per-user global DB.
fn db_cmd(cwd_project: &Path, action: &DbAction) -> sim_flow::Result<()> {
    use sim_flow::__internal::global_db::{GlobalDb, default_db_path};
    match action {
        DbAction::Path => {
            match default_db_path() {
                Some(p) => println!("{}", p.display()),
                None => {
                    return Err(sim_flow::Error::State(
                        "global DB path unavailable: directories::ProjectDirs returned None \
                         (HOME unset?)"
                            .to_string(),
                    ));
                }
            }
            Ok(())
        }
        DbAction::Chart {
            kind,
            project,
            step,
            limit,
            bar_width,
        } => {
            let Some(db_path) = default_db_path() else {
                return Err(sim_flow::Error::State(
                    "global DB unavailable: directories::ProjectDirs returned None".to_string(),
                ));
            };
            let mut db = GlobalDb::open(&db_path)?;
            let library_kind: sim_flow::__internal::db_charts::ChartKind = (*kind).into();
            let filters = sim_flow::__internal::db_reports::ReportFilters {
                project: project.clone(),
                step: step.clone(),
                limit: *limit,
            };
            let data =
                sim_flow::__internal::db_charts::build_chart(&mut db, library_kind, &filters)?;
            render_terminal_chart(&data, bar_width.unwrap_or(60));
            Ok(())
        }
        DbAction::Report {
            kind,
            project,
            step,
            limit,
            json,
        } => {
            let Some(db_path) = default_db_path() else {
                return Err(sim_flow::Error::State(
                    "global DB unavailable: directories::ProjectDirs returned None".to_string(),
                ));
            };
            let mut db = GlobalDb::open(&db_path)?;
            let library_kind: sim_flow::__internal::db_reports::ReportKind = (*kind).into();
            let filters = sim_flow::__internal::db_reports::ReportFilters {
                project: project.clone(),
                step: step.clone(),
                limit: *limit,
            };
            let (columns, rows) =
                sim_flow::__internal::db_reports::run_report(&mut db, library_kind, &filters)?;
            if *json {
                let value = serde_json::json!({
                    "report": library_kind.slug(),
                    "columns": columns,
                    "rows": rows,
                });
                println!("{}", serde_json::to_string_pretty(&value).unwrap());
            } else {
                println!("# {}", library_kind.slug());
                render_text_table(&columns, &rows);
            }
            Ok(())
        }
        DbAction::Query { sql, json } => {
            let Some(db_path) = default_db_path() else {
                return Err(sim_flow::Error::State(
                    "global DB unavailable: directories::ProjectDirs returned None".to_string(),
                ));
            };
            let mut db = GlobalDb::open(&db_path)?;
            let (columns, rows) = db.query_read_only(sql)?;
            if *json {
                let value = serde_json::json!({
                    "columns": columns,
                    "rows": rows,
                });
                println!("{}", serde_json::to_string_pretty(&value).unwrap());
            } else {
                render_text_table(&columns, &rows);
            }
            Ok(())
        }
        DbAction::Backfill {
            paths,
            force_tool_timings,
        } => {
            let Some(db_path) = default_db_path() else {
                return Err(sim_flow::Error::State(
                    "global DB unavailable: directories::ProjectDirs returned None".to_string(),
                ));
            };
            let db = GlobalDb::open(&db_path)?;
            let resolved_paths: Vec<PathBuf> = if paths.is_empty() {
                vec![cwd_project.to_path_buf()]
            } else {
                paths.clone()
            };
            for project in &resolved_paths {
                if !project.join(".sim-flow").is_dir() {
                    eprintln!("skip: {} has no `.sim-flow/` directory", project.display());
                    continue;
                }
                let summary = db_backfill_project(&db, project, *force_tool_timings)?;
                println!(
                    "{}: bugs={} llm_metrics={} tool_timings={} experiment_runs={} \
                     experiment_baselines={}",
                    project.display(),
                    summary.bugs,
                    summary.llm_metrics,
                    summary.tool_timings,
                    summary.experiment_runs,
                    summary.experiment_baselines,
                );
            }
            Ok(())
        }
        DbAction::Stats { json } => {
            let Some(path) = default_db_path() else {
                return Err(sim_flow::Error::State(
                    "global DB unavailable: directories::ProjectDirs returned None".to_string(),
                ));
            };
            // `db stats` opens the DB read-only on its own connection so it
            // doesn't fight the in-process singleton (which an active
            // `sim-flow auto` session may already own).
            let db = GlobalDb::open(&path)?;
            let stats = collect_db_stats(&db)?;
            if *json {
                let value = serde_json::json!({
                    "path": path.to_string_lossy(),
                    "schema_version": stats.schema_version,
                    "machine_id": stats.machine_id,
                    "user_identity": stats.user_identity,
                    "tables": stats.tables.iter().map(|t| {
                        serde_json::json!({
                            "table": t.name,
                            "row_count": t.row_count,
                            "last_write": t.last_write,
                        })
                    }).collect::<Vec<_>>(),
                });
                println!("{}", serde_json::to_string_pretty(&value).unwrap());
            } else {
                println!("path:           {}", path.display());
                println!("schema_version: {}", stats.schema_version);
                println!("machine_id:     {}", stats.machine_id);
                println!("user_identity:  {}", stats.user_identity);
                println!();
                println!("{:<24}  {:>10}  last_write", "table", "rows");
                println!("{:<24}  {:>10}  ----------", "----", "----");
                for t in &stats.tables {
                    let last = t.last_write.as_deref().unwrap_or("-");
                    println!("{:<24}  {:>10}  {}", t.name, t.row_count, last);
                }
            }
            Ok(())
        }
    }
}

/// Render a `ChartData` to stdout as a horizontal Unicode-bar
/// histogram. One bar per row, max bar length `bar_width` characters,
/// scaled to the row with the largest absolute value. Empty datasets
/// print a "(no data)" line.
fn render_terminal_chart(data: &sim_flow::__internal::db_charts::ChartData, bar_width: usize) {
    println!("{}", data.title);
    println!("{}", "─".repeat(data.title.chars().count()));
    if data.rows.is_empty() {
        println!("(no data)");
        return;
    }
    let label_width = data
        .rows
        .iter()
        .map(|r| r.label.chars().count())
        .max()
        .unwrap_or(0)
        .max(5);
    let max_value = data
        .rows
        .iter()
        .map(|r| r.value.abs())
        .fold(0.0_f64, f64::max);
    let scale = if max_value > 0.0 {
        bar_width as f64 / max_value
    } else {
        0.0
    };
    for row in &data.rows {
        let bar_len = ((row.value.abs() * scale).round() as usize).min(bar_width);
        let bar = "█".repeat(bar_len);
        let value_label = if row.value.fract() == 0.0 {
            format!("{:.0} {}", row.value, data.unit)
        } else {
            format!("{:.2} {}", row.value, data.unit)
        };
        println!(
            "{:<label_width$}  {bar:<bar_width$}  {value_label}",
            row.label,
            label_width = label_width,
            bar_width = bar_width,
        );
    }
}

/// Render a 2-D string-cell table to stdout with auto-sized columns.
/// Used by `db query` and (later) `db report` for tabular output.
fn render_text_table(columns: &[String], rows: &[Vec<serde_json::Value>]) {
    if columns.is_empty() {
        println!("(no columns)");
        return;
    }
    let stringify = |v: &serde_json::Value| -> String {
        match v {
            serde_json::Value::Null => "NULL".to_string(),
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    };
    let mut widths: Vec<usize> = columns.iter().map(|c| c.chars().count()).collect();
    let mut formatted: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    for row in rows {
        let mut cells: Vec<String> = Vec::with_capacity(columns.len());
        for (i, cell) in row.iter().enumerate() {
            let s = stringify(cell);
            if i < widths.len() {
                widths[i] = widths[i].max(s.chars().count());
            }
            cells.push(s);
        }
        formatted.push(cells);
    }
    // Cap any single column at 72 chars to keep wide TEXT cells from
    // wrecking the layout; truncation is marked with an ellipsis.
    let max_col_width = 72;
    for w in &mut widths {
        if *w > max_col_width {
            *w = max_col_width;
        }
    }
    let print_row = |cells: &[String]| {
        let mut line = String::new();
        for (i, cell) in cells.iter().enumerate() {
            let mut display = cell.clone();
            if display.chars().count() > widths[i] {
                let head: String = display.chars().take(widths[i].saturating_sub(1)).collect();
                display = format!("{head}…");
            }
            line.push_str(&format!("{display:<width$}", width = widths[i]));
            if i + 1 < cells.len() {
                line.push_str("  ");
            }
        }
        println!("{}", line.trim_end());
    };
    print_row(columns);
    let separators: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    print_row(&separators);
    for row in &formatted {
        print_row(row);
    }
    println!(
        "({} row{})",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" }
    );
}

#[derive(Default)]
struct BackfillSummary {
    bugs: usize,
    llm_metrics: usize,
    tool_timings: usize,
    experiment_runs: usize,
    experiment_baselines: usize,
}

fn db_backfill_project(
    db: &sim_flow::__internal::global_db::GlobalDb,
    project: &Path,
    force_tool_timings: bool,
) -> sim_flow::Result<BackfillSummary> {
    use sim_flow::__internal::bug_log::BugRecord;
    use sim_flow::__internal::session::llm_metrics::LlmMetricsRecord;
    use sim_flow::__internal::session::tool_timings::ToolTimingRecord;
    use sim_flow::__internal::tracking::index::{ExperimentIndex, RunFilter};

    let mut summary = BackfillSummary::default();

    // --- Bugs (JSONL) ----------------------------------------------------
    let bugs_path = project.join(".sim-flow").join("bug-log.jsonl");
    if bugs_path.is_file() {
        for line in read_jsonl_lines(&bugs_path)? {
            let Some(rec) = parse_jsonl_record::<BugRecord>(&line) else {
                continue;
            };
            db.record_bug(project, &rec)?;
            summary.bugs += 1;
        }
    }

    // --- LLM metrics (JSONL) --------------------------------------------
    let metrics_path = project
        .join(".sim-flow")
        .join("logs")
        .join("llm-metrics.jsonl");
    if metrics_path.is_file() {
        for line in read_jsonl_lines(&metrics_path)? {
            let Some(rec) = parse_jsonl_record::<LlmMetricsRecord>(&line) else {
                continue;
            };
            db.record_llm_metric(project, &rec)?;
            summary.llm_metrics += 1;
        }
    }

    // --- Tool timings (JSONL, offset-tracked) ----------------------------
    let timings_path = project
        .join(".sim-flow")
        .join("logs")
        .join("tool-timings.jsonl");
    if timings_path.is_file() {
        let start_offset = if force_tool_timings {
            0
        } else {
            db.backfill_offset(project, "tool_timings.jsonl")?
        };
        let file = std::fs::File::open(&timings_path).map_err(|source| sim_flow::Error::Io {
            path: timings_path.clone(),
            source,
        })?;
        let file_len = file
            .metadata()
            .map_err(|source| sim_flow::Error::Io {
                path: timings_path.clone(),
                source,
            })?
            .len();
        if start_offset < file_len {
            use std::io::{BufRead, BufReader, Seek, SeekFrom};
            let mut reader = BufReader::new(file);
            reader
                .seek(SeekFrom::Start(start_offset))
                .map_err(|source| sim_flow::Error::Io {
                    path: timings_path.clone(),
                    source,
                })?;
            let mut line = String::new();
            loop {
                line.clear();
                let bytes = reader
                    .read_line(&mut line)
                    .map_err(|source| sim_flow::Error::Io {
                        path: timings_path.clone(),
                        source,
                    })?;
                if bytes == 0 {
                    break;
                }
                if let Some(rec) = parse_jsonl_record::<ToolTimingRecord>(line.trim()) {
                    db.record_tool_timing(project, &rec)?;
                    summary.tool_timings += 1;
                }
            }
            db.set_backfill_offset(project, "tool_timings.jsonl", file_len)?;
        }
    }

    // --- experiments.db --------------------------------------------------
    let experiments_path = project.join(".sim-flow").join("experiments.db");
    if experiments_path.is_file() {
        let index = ExperimentIndex::open_path(&experiments_path)?;
        for run in index.list_runs(&RunFilter::default())? {
            db.record_experiment_run(project, &run)?;
            summary.experiment_runs += 1;
        }
        for (name, run_id, timestamp) in index.list_baselines()? {
            db.record_experiment_baseline(project, &name, &run_id, &timestamp, None)?;
            summary.experiment_baselines += 1;
        }
    }

    Ok(summary)
}

fn read_jsonl_lines(path: &Path) -> sim_flow::Result<Vec<String>> {
    let body = std::fs::read_to_string(path).map_err(|source| sim_flow::Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(body
        .lines()
        .filter_map(|s| {
            let trimmed = s.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .collect())
}

fn parse_jsonl_record<T: serde::de::DeserializeOwned>(line: &str) -> Option<T> {
    match serde_json::from_str::<T>(line) {
        Ok(rec) => Some(rec),
        Err(err) => {
            tracing::warn!(error = %err, "db backfill: skipping malformed JSONL line");
            None
        }
    }
}

struct DbStats {
    schema_version: u32,
    machine_id: String,
    user_identity: String,
    tables: Vec<DbTableStats>,
}

struct DbTableStats {
    name: &'static str,
    row_count: i64,
    /// Latest non-null timestamp-like column value. `None` when the
    /// table is empty or doesn't have a timestamp column we recognize.
    last_write: Option<String>,
}

fn collect_db_stats(db: &sim_flow::__internal::global_db::GlobalDb) -> sim_flow::Result<DbStats> {
    use sim_flow::__internal::global_db::user_identity;

    // `(table, timestamp-column-name)` for the human-friendly last-write
    // column. `None` -> we just report row count.
    const TABLES: &[(&str, Option<&str>)] = &[
        ("bugs", Some("opened_at")),
        ("llm_metrics", Some("timestamp")),
        ("tool_timings", Some("timestamp")),
        ("experiment_runs", Some("timestamp")),
        ("experiment_baselines", Some("timestamp")),
        ("experiment_ppa_estimates", Some("timestamp")),
    ];

    let mut tables = Vec::with_capacity(TABLES.len());
    for &(name, ts_col) in TABLES {
        let row_count = db.count(name)?;
        let last_write = if let Some(col) = ts_col
            && row_count > 0
        {
            db.latest_timestamp(name, col)?
        } else {
            None
        };
        tables.push(DbTableStats {
            name,
            row_count,
            last_write,
        });
    }
    Ok(DbStats {
        schema_version: db.schema_version()?,
        machine_id: db.machine_id().to_string(),
        user_identity: user_identity(),
        tables,
    })
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
