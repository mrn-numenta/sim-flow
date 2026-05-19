use std::path::PathBuf;

use clap::{Parser, Subcommand};
use sim_flow::__internal::state::Flow;

pub(crate) mod actions;
pub(crate) mod embedder;
pub(crate) mod lance_index;

#[cfg(test)]
mod tests;

pub(crate) use actions::{
    BaselineAction, BugsAction, ConfigAction, CoverageAction, DbAction, KeysAction, NewKind,
    PromptResetScope, PromptScopeArg, PromptsAction, WatchersAction,
};

#[derive(Debug, Parser)]
#[command(
    name = "sim-flow",
    version,
    about = "Orchestrator for the AI-assisted modeling flows"
)]
pub(crate) struct Cli {
    /// Explicit path to the sim-foundation repository root. Overrides the
    /// `SIM_FOUNDATION_ROOT` env var and the walk-up search.
    #[arg(long, global = true)]
    pub(crate) foundation_root: Option<PathBuf>,

    /// Explicit project directory. Defaults to the current working
    /// directory.
    #[arg(long, global = true)]
    pub(crate) project: Option<PathBuf>,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
// `Auto` is genuinely large (clap collapses every flag into the
// variant struct); the boxing rewrite suggested by clippy would
// hurt readability without changing runtime behavior in any
// measurable way for this CLI's call frequency. Suppress here so
// the new --qa-llm-* / --critique-llm-* flags don't trip the
// inherited `-D warnings`.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Command {
    /// Initialize `.sim-flow/` state and config in the project directory.
    Init {
        /// Which flow to initialize.
        #[arg(long)]
        flow: FlowArg,
    },
    /// Show the current flow, step, and gate status.
    Status {
        /// Emit machine-readable JSON instead of the human-format
        /// summary.
        #[arg(long)]
        json: bool,
    },
    /// Run a step (default: the current step).
    Run {
        /// Step id. Defaults to the current step.
        step: Option<String>,
        /// Target a specific candidate (per-candidate steps only).
        #[arg(long)]
        candidate: Option<String>,
    },
    /// Run gate validation only (no AI session).
    Gate {
        /// Step id. Defaults to the current step.
        step: Option<String>,
        /// Candidate scope for per-candidate steps.
        #[arg(long)]
        candidate: Option<String>,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reset a step and cascade to downstream gates. Deletes every
    /// artifact and critique from the target step forward, which is
    /// not reversible -- a misclick on `DM2a` from `DM4b` wipes the
    /// entire model + testbench + perf work in between. Requires
    /// an explicit `--force` to confirm. See orchestrator audit #15
    /// (2026-05-16).
    Reset {
        /// Step id to reset.
        step: String,
        /// Confirm the destructive reset. Required: without this
        /// flag the command exits with a usage error instead of
        /// deleting forward state.
        #[arg(long)]
        force: bool,
    },
    /// Flip a DirectModeling-completed project into the
    /// SystemVerilog Convert flow. Archives the DM gate history
    /// (visible via `sim-flow status` after the flip) and parks
    /// `current_step` at `SV0`. After this, `sim-flow auto`
    /// drives SV0 -> SV0d -> SV1 -> SV2 -> SV3, emitting RTL +
    /// UVM under `generated/`. Requires DM4b to have passed; the
    /// command refuses to flip otherwise so half-finished
    /// projects don't lose DM-side context.
    ConvertSv {
        /// Skip the DM4b-passed precondition. Useful in tests
        /// and for projects that intentionally skip a DM step;
        /// the flip is destructive (archives DM gates) so we
        /// keep the safety check on by default.
        #[arg(long)]
        force: bool,
    },
    /// Inspect the project's bug log
    /// (`<project>/.sim-flow/bug-log.jsonl`).
    Bugs {
        #[command(subcommand)]
        action: BugsAction,
    },
    /// Show or set configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Inspect the per-user global telemetry DB
    /// (`~/Library/Application Support/sim-flow/sim-flow.db` on macOS).
    /// Aggregates bugs, LLM metrics, tool timings, and experiments
    /// across every project the developer has run on this machine.
    Db {
        #[command(subcommand)]
        action: DbAction,
    },
    /// Create a new project from a template.
    New {
        #[command(subcommand)]
        kind: NewKind,
    },
    /// List experiment runs from `.sim-flow/experiments.db`.
    Runs {
        /// Filter by workload.
        #[arg(long)]
        workload: Option<String>,
        /// Filter by candidate.
        #[arg(long)]
        candidate: Option<String>,
        /// Filter by study.
        #[arg(long)]
        study: Option<String>,
        /// Show sweep variants with this parent run id.
        #[arg(long)]
        sweep: Option<String>,
        /// Maximum rows to display.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Emit machine-readable JSON instead of the human-format
        /// table.
        #[arg(long)]
        json: bool,
    },
    /// Record a simulation run into the experiments index.
    RecordRun {
        /// Short description used to build the run id.
        #[arg(long)]
        description: String,
        #[arg(long)]
        workload: Option<String>,
        #[arg(long)]
        candidate: Option<String>,
        #[arg(long)]
        study: Option<String>,
        /// Path to the model's run manifest (relative to project dir).
        #[arg(long)]
        manifest: Option<PathBuf>,
        /// Free-form notes.
        #[arg(long)]
        notes: Option<String>,
    },
    /// Baseline management.
    Baseline {
        #[command(subcommand)]
        action: BaselineAction,
    },
    /// Execute a sweep described by a TOML definition.
    Sweep {
        /// Path to the sweep.toml definition.
        #[arg(long)]
        file: PathBuf,
    },
    /// List child runs for a sweep parent.
    SweepResults {
        /// Parent run id from `sim-flow runs --sweep`.
        parent: String,
    },
    /// Execute a declarative perf-plan (TOML). The plan declares
    /// studies (type1 / type2 / type3) and the executor walks each,
    /// invoking the project binary per cell and recording every run
    /// into `.sim-flow/experiments.db`.
    PerfRun {
        /// Path to the plan.toml. Defaults to
        /// `<project>/docs/perf-plan/plan.toml` if omitted.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Diff metrics between two recorded run-ids. Useful for CI
    /// regression checks ("did baseline get worse?") and
    /// interactive "what changed between A and B?" inspection.
    /// Output is markdown (pipe into a PR comment or design-review
    /// doc without further formatting).
    Diff {
        /// Left-hand-side run-id (the baseline).
        lhs: String,
        /// Right-hand-side run-id (the comparison).
        rhs: String,
    },
    /// Plan-execution progress. JSON-only output for UI surfaces
    /// (VS Code dashboard, future web/terminal viewers) so the
    /// extension doesn't reach into milestone files directly.
    /// Pass `--kind <impl|test|perf>` for one kind, or `--all`
    /// for every kind in one call; `--current-step <step>` returns
    /// whichever plan that step drives.
    PlanProgress {
        /// One plan kind to read (impl / test / perf).
        #[arg(long, value_enum)]
        kind: Option<PlanKindArg>,
        /// Step id from which to derive the plan kind (e.g. DM4b).
        /// Mutually exclusive with `--kind` and `--all`; specifying
        /// more than one is a CLI error.
        #[arg(long)]
        current_step: Option<String>,
        /// Return all three plan kinds (impl + test + perf) in one
        /// call.
        #[arg(long)]
        all: bool,
    },
    /// List recorded critiques. JSON-only output for UI surfaces.
    /// Without `--step`, returns every step's critique in stable
    /// order. The orchestrator owns the JSON+markdown parsing; the
    /// extension consumes the structured shape.
    Critiques {
        /// One step id (e.g. `DM3a`). Returns a single entry or
        /// `null` if neither the JSON nor markdown form is on disk.
        #[arg(long)]
        step: Option<String>,
    },
    /// Enumerate project documents (per-step work artifacts +
    /// critique files + source spec) with stats, line counts for
    /// code files, and inline previews for the markdown documents
    /// the dashboard renders directly. JSON-only output for UI
    /// surfaces. `--flow` selects which step sequence to walk;
    /// defaults to `direct-modeling`.
    Documents {
        /// Flow id: `direct-modeling` (default) or `design-study`.
        #[arg(long, default_value = "direct-modeling")]
        flow: String,
    },
    /// Validate the gate for a step and, if clean, mark it passed and
    /// advance `current_step` to the next step in the flow.
    ///
    /// Read-only inspection (no state mutation) is available via
    /// `sim-flow gate <step>`; this command is the explicit
    /// "advance state" primitive.
    Advance {
        /// Step id to advance past. Defaults to the current step.
        step: Option<String>,
        /// Candidate scope for per-candidate steps (currently
        /// rejected; per-candidate advancement lands with DSF).
        #[arg(long)]
        candidate: Option<String>,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Emit a step descriptor for hosts (chat extensions, IDE
    /// plugins) so they don't duplicate step knowledge. Reports the
    /// resolved instruction file path and body, expected work
    /// artifacts, predecessor inputs, and gate checks.
    Describe {
        /// Step id and kind, e.g. `DM0.work` or `DM2c.critique`.
        step_kind: String,
        /// Emit machine-readable JSON. (Default for this command;
        /// kept as a flag for symmetry with other subcommands.)
        #[arg(long, default_value_t = true)]
        json: bool,
    },
    /// Run an interactive work or critique session under orchestrator
    /// control. With `--jsonl`, speaks the session protocol on stdio
    /// (used by IDE hosts). Without it, the in-process TerminalHost
    /// drives the session against a built-in CliAgent.
    /// Drive the entire flow unattended: work -> critique -> advance
    /// per step from the current step through the end of the flow.
    /// Uses the JSONL session protocol on stdio so an IDE host can
    /// render a single continuous chat.
    Auto {
        /// LLM backend label echoed to the host (e.g. `vscode`,
        /// `anthropic`, `ollama`, `lmstudio`, `vllm`,
        /// `openai-compat`).
        #[arg(long, default_value = "vscode")]
        llm_backend: String,
        /// Optional model identifier.
        #[arg(long)]
        llm_model: Option<String>,
        /// Optional explicit model-family override. Empty means infer
        /// from `--llm-model`.
        #[arg(long)]
        llm_model_family: Option<String>,
        /// Optional explicit runtime capability profile override.
        #[arg(long)]
        llm_runtime_profile: Option<String>,
        /// Emit extra backend/runtime/model-family diagnostics around
        /// each LLM dispatch. Useful when debugging adaptation issues.
        #[arg(long, default_value_t = false)]
        llm_debug_adaptation: bool,
        /// Base URL override for the local-server backends
        /// (`ollama`, `lmstudio`, `vllm`, `openai-compat`). Each
        /// of these speaks an OpenAI-compatible chat-completions
        /// endpoint (Ollama exposes its compat shim at `/v1`).
        /// Format is whatever the server documents -- typically
        /// `http://<host>:<port>/v1`. When omitted, each backend
        /// falls back to its conventional default
        /// (`http://localhost:11434/v1` for Ollama,
        /// `http://localhost:1234/v1` for LM Studio,
        /// `http://localhost:8000/v1` for vLLM). Ignored for
        /// `vscode` / `anthropic` / `openai` (those use
        /// hosted-API endpoints) and the `*-cli` backends.
        #[arg(long)]
        llm_base_url: Option<String>,
        /// Optional per-kind LLM override for *critique* sessions.
        /// When set, critique sub-sessions use this backend (and
        /// its companion `--critique-llm-*` flags) instead of the
        /// work-side `--llm-backend`. Typical pattern: run work on
        /// a fast / cheap local model (e.g. `--llm-backend vllm
        /// --llm-base-url http://localhost:8012/v1`) and route
        /// critique to a stronger hosted model
        /// (`--critique-llm-backend anthropic
        /// --critique-llm-model claude-3-5-sonnet-latest`) so
        /// reviews catch issues the work-side model misses without
        /// paying the hosted-model cost on every turn. Each
        /// `--critique-llm-*` flag is independent: unset fields
        /// fall back to the matching work-side value, so you can
        /// override just the backend and keep the model, or just
        /// the base URL, etc.
        #[arg(long)]
        critique_llm_backend: Option<String>,
        /// Model id for critique sessions. Falls back to
        /// `--llm-model` when unset. See `--critique-llm-backend`.
        #[arg(long)]
        critique_llm_model: Option<String>,
        /// Model-family override for critique sessions. Falls back
        /// to `--llm-model-family` when unset.
        #[arg(long)]
        critique_llm_model_family: Option<String>,
        /// Runtime-profile override for critique sessions. Falls
        /// back to `--llm-runtime-profile` when unset.
        #[arg(long)]
        critique_llm_runtime_profile: Option<String>,
        /// Base URL override for critique sessions. Falls back to
        /// `--llm-base-url` when unset. Honored only when the
        /// critique backend is a local-server family
        /// (`ollama` / `lmstudio` / `vllm` / `openai-compat`).
        #[arg(long)]
        critique_llm_base_url: Option<String>,
        /// Optional per-kind LLM override for *idle-state Q&A*
        /// turns -- the side-conversation that fires when the user
        /// types a `UserMessage` while manual mode is parked
        /// between sub-sessions. Mirrors `--critique-llm-*`:
        /// unset fields fall back per-field to `--llm-*`. Use case:
        /// route conversational Q&A to a cheaper / chattier model
        /// (e.g. `--qa-llm-backend openai --qa-llm-model gpt-4o-mini`)
        /// while work + critique stay on the heavyweight default.
        #[arg(long)]
        qa_llm_backend: Option<String>,
        #[arg(long)]
        qa_llm_model: Option<String>,
        #[arg(long)]
        qa_llm_model_family: Option<String>,
        #[arg(long)]
        qa_llm_runtime_profile: Option<String>,
        #[arg(long)]
        qa_llm_base_url: Option<String>,
        /// Per-session structural-gate iteration cap. The
        /// orchestrator's auto mode fires this when a Work session
        /// has produced no artifact for this many consecutive turns
        /// (the gate stays dirty, no fenced write block appears).
        ///
        /// Default history: 3 -> 6. The bump was triggered by the
        /// post-Phase-0 hardening pass: with
        /// `SIM_FLOW_DISABLE_THINKING=1`, qwen3.6 retry-work
        /// sessions sometimes spend several turns reading +
        /// considering the critique before committing a write,
        /// and the original cap of 3 fired before the model
        /// settled. 6 gives the forcing-prompt loop more room
        /// (the orchestrator pushes "Produce the artifact file(s)
        /// now ..." after each empty turn) without letting a
        /// truly-stuck run burn forever. See the Phase 0b /
        /// Phase 0c entries in
        /// docs/brainstorming/model-robustness-study.md.
        #[arg(long, default_value_t = 6)]
        max_auto_iters: u32,
        /// Cross-session retry cap (absolute ceiling) when the
        /// critique reports gate-failing findings. Even
        /// genuinely-progressing runs stop here -- if the model is
        /// still chipping at the problem after 10+ retries,
        /// something else is wrong (prompt, gate, model).
        #[arg(long, default_value_t = 10)]
        max_critique_iters: u32,
        /// No-progress cap: flip to manual after this many
        /// consecutive critique retries whose gate-failing-finding
        /// count did NOT strictly decrease. Catches "model
        /// plateaued / oscillating" patterns early without
        /// burning the absolute retry budget. Set to 0 to disable
        /// (the absolute cap then becomes the only signal).
        #[arg(long, default_value_t = 3)]
        max_critique_no_progress_iters: u32,
        /// Run DM0.work in interactive mode (the user describes what
        /// to build). Subsequent sessions still run in auto mode.
        #[arg(long)]
        dm0_interactive: bool,
        /// Deprecated. The legacy `--spec` flag ran
        /// `ingest_spec_file` to populate
        /// `.sim-flow/source-spec.<ext>` and per-page chunks under
        /// `.sim-flow/spec-pages/`. That pipeline is retired in
        /// favor of the format-discovery corpus written by
        /// `sim-flow ingest` to `.sim-flow/spec-ingest/`. The flag
        /// is still accepted for backward compatibility but no
        /// longer triggers an ingest; users should run
        /// `sim-flow ingest --project <project> --source <path>`
        /// first.
        #[arg(long)]
        spec: Option<PathBuf>,
        /// Speak the session protocol over a reconnectable Unix
        /// socket at the provided path instead of stdio. Used by the
        /// VS Code chat panel so a live auto session can survive
        /// extension reloads and later reattach.
        #[arg(long)]
        transport_socket: Option<PathBuf>,
        /// Bind a read-only event-broadcast Unix socket at the given
        /// path. Every event the orchestrator emits is mirrored to
        /// every connected observer (history is replayed on attach).
        /// Observers cannot send commands; the primary host (stdio
        /// or `--transport-socket`) still owns the command channel.
        /// Use this to watch a run that's being driven by something
        /// else -- the dashboard, `e2e_manual`, an external script,
        /// or `nc -U`. Multiple observers can attach concurrently.
        #[arg(long)]
        watch_socket: Option<PathBuf>,
        /// Lifecycle for interactive CLI-agent backends (currently
        /// only `claude`). `per-step` (default) spawns a fresh agent
        /// per step. `single` (Pass 2) keeps one agent across the
        /// whole flow with lazy re-spawn. Ignored for non-interactive
        /// backends (vscode / anthropic / openai / ollama / openai-compat).
        #[arg(long, value_enum, default_value_t = SessionMode::PerStep)]
        session_mode: SessionMode,
        /// Step-axis mode (orthogonal to `--session-mode`). `auto`
        /// (default) walks `current_step` to end of flow without user
        /// input. `manual` binds the transport, hello-handshakes, then
        /// parks waiting for `RunStep` / `RunCritique` / `RunGate` /
        /// `Advance` / `Reset` / `SetStepMode` / `Shutdown` host
        /// commands. The dashboard's step-mode toggle picks this at
        /// connect time and can flip the flag live via `SetStepMode`.
        #[arg(long, value_enum, default_value_t = StepMode::Auto)]
        step_mode: StepMode,
        /// Hard cap on total LLM requests per work / critique
        /// sub-session. Backstop against runaway-loop bugs that the
        /// more specific `max_auto_iters` / `max_critique_iters`
        /// caps don't catch. Default 500 (was 50): a full
        /// 14-step DM flow with retries can legitimately need 200+
        /// dispatches, and the prior default tripped runs that
        /// were otherwise progressing fine. 0 disables the check
        /// (NOT recommended -- runaway loops will burn tokens
        /// indefinitely).
        #[arg(long, default_value_t = 500)]
        max_llm_requests: u32,
        /// Number of structurally-identical assistant responses in a
        /// row that triggers an auto-abort. Digits / whitespace are
        /// normalized before comparing so timestamp-shaped churn
        /// doesn't defeat the check. 0 or 1 disables.
        #[arg(long, default_value_t = 3)]
        max_identical_responses: u32,
        /// Cap on concurrent in-flight LLM Work sessions during
        /// plan-detail walks (DM2cd / DM3ad / DM4ad). `0` means
        /// unbounded; `1` forces the legacy serial path. Higher
        /// values fan out N pending milestone stubs in parallel up
        /// to the cap. No effect on execution walks (DM2d / DM3b /
        /// DM3c / DM4b) which stay serial. CLI value overrides
        /// `.sim-flow/config.toml::[llm].max_parallel_requests`;
        /// `None` (flag unset) falls through to the config value.
        #[arg(long)]
        max_parallel_requests: Option<u32>,
        /// Default ON. Loads the response-shape convention into
        /// every session's system prompt: tool calls first, prose
        /// last, no recap, no hedging. Cuts qwen3.6 / nemotron
        /// preamble that routinely consumes the full max_tokens
        /// budget. Pass `--preamble` to disable when debugging a
        /// model's reasoning (extra prose IS what you want then).
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        no_preamble: bool,
    },
    Session {
        /// Step id and kind, e.g. `DM0.work` or `DM2c.critique`.
        step_kind: String,
        /// Speak the JSONL session protocol on stdio. Required for
        /// IDE hosts.
        #[arg(long)]
        jsonl: bool,
        /// Speak the session protocol over a reconnectable Unix
        /// socket at the provided path instead of stdio.
        #[arg(long)]
        transport_socket: Option<PathBuf>,
        /// LLM backend. With `--jsonl`: opaque label echoed back to
        /// the host. Without `--jsonl`: selects the built-in
        /// CliAgent (`claude`, `codex`, `gh-copilot`, `ollama`,
        /// `openai-compat`).
        #[arg(long, default_value = "vscode")]
        llm_backend: String,
        /// Optional model identifier. Required for OpenAI-compat
        /// servers and useful for Ollama (defaults to `llama3.1`);
        /// ignored by `claude` / `codex` unless their CLI accepts a
        /// `--model` flag.
        #[arg(long)]
        llm_model: Option<String>,
        /// Optional explicit model-family override. Empty means infer
        /// from `--llm-model`.
        #[arg(long)]
        llm_model_family: Option<String>,
        /// Optional explicit runtime capability profile override.
        #[arg(long)]
        llm_runtime_profile: Option<String>,
        /// Emit extra backend/runtime/model-family diagnostics around
        /// each LLM dispatch. Useful when debugging adaptation issues.
        #[arg(long, default_value_t = false)]
        llm_debug_adaptation: bool,
        /// Override the Ollama base URL (default
        /// `http://localhost:11434/v1` — Ollama's OpenAI-compat
        /// shim; the native API at `/api` isn't used). Only
        /// meaningful when `--llm-backend ollama`. Superseded by
        /// `--llm-base-url` when both are set.
        #[arg(long)]
        ollama_base_url: Option<String>,
        /// Override the OpenAI-compat base URL (default
        /// `http://localhost:1234/v1` — LM Studio's port). Only
        /// meaningful when `--llm-backend openai-compat`. Set to
        /// vLLM / llama.cpp / TGI's port as needed. Superseded by
        /// `--llm-base-url` when both are set.
        #[arg(long)]
        openai_base_url: Option<String>,
        /// Generic base-URL override that applies to whichever
        /// backend is selected -- mirrors `sim-flow auto`'s flag.
        /// Wins over `--ollama-base-url` / `--openai-base-url` when
        /// both are set. Format is whatever the server documents,
        /// typically `http://<host>:<port>/v1`.
        #[arg(long)]
        llm_base_url: Option<String>,
        /// Candidate scope for per-candidate steps (DSF).
        #[arg(long)]
        candidate: Option<String>,
    },
    /// Inspect or override the per-step instruction prompts. Each
    /// prompt resolves in scope order: project (`<project>/.sim-flow/prompts/`),
    /// global (OS-aware user config dir), then the foundation default
    /// shipped in `<foundation>/instructions/`.
    Prompts {
        #[command(subcommand)]
        action: PromptsAction,
    },
    /// Inspect or update DM3c coverage acceptance criteria stored
    /// in `.sim-flow/config.toml::coverage`. The DM3c critique
    /// enforces these against the live `cargo llvm-cov` report.
    Coverage {
        #[command(subcommand)]
        action: CoverageAction,
    },
    /// Generate a Sugiyama block diagram of the project's foundation
    /// model. Runs `cargo run -- --dump-netlist-json <tmp>` against
    /// the project to obtain the live ConnectivityPlan, then renders
    /// SVG via the workspace `block-diagram` crate. Output lands at
    /// `<project>/.sim-flow/block-diagram.svg` and the dashboard's
    /// "Block Diagram" tab picks it up.
    BlockDiagram {
        /// Output SVG path. Defaults to `<project>/.sim-flow/block-diagram.svg`.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Layout direction: `tb` (top-to-bottom; default) or `lr`.
        #[arg(long, default_value = "tb")]
        direction: String,
        /// Show port type names alongside ports.
        #[arg(long)]
        show_types: bool,
        /// Optional pre-existing netlist JSON. Skips the cargo step
        /// when supplied -- useful when the project's binary needs
        /// arguments the block-diagram subcommand doesn't know about.
        #[arg(long)]
        netlist: Option<PathBuf>,
    },
    /// Manage stored API keys for LLM backends. Resolution order at
    /// run time: provider env var (e.g. `ANTHROPIC_API_KEY`) →
    /// `<config>/sim-flow/credentials.toml` → (in the VS Code
    /// extension only) OS keychain via SecretStorage. Both the CLI
    /// and the extension share this on-disk file so a key set
    /// once works in both contexts.
    Keys {
        #[command(subcommand)]
        action: KeysAction,
    },
    /// Run the spec-ingest pipeline (Phase 2 architecture chapter 1)
    /// against a primary source spec, optionally with peer specs.
    /// Output lands at `<project>/.sim-flow/spec-ingest/`. See
    /// `tools/sim-flow/docs/architecture/01-spec-ingest-pipeline.md`
    /// for the full contract.
    Ingest {
        /// Absolute or project-relative path to the primary source
        /// spec (PDF / markdown / text). Required unless `--rebuild`
        /// or `--status` is passed.
        #[arg(long)]
        source: Option<PathBuf>,
        /// Peer spec registration in the form `id=path`. May appear
        /// multiple times. The peer spec is recorded in the
        /// manifest; full ingestion of peers is currently deferred
        /// to a follow-up rebuild.
        #[arg(long = "peer", value_parser = parse_peer_arg, action = clap::ArgAction::Append)]
        peers: Vec<(String, PathBuf)>,
        /// Path to an ingest config TOML. Defaults to
        /// `<project>/.sim-flow/spec-ingest.config.toml` when present.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Project root the ingest writes under. Defaults to the
        /// global `--project` flag (or the current working dir).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Re-ingest from the source path recorded in the existing
        /// manifest.
        #[arg(long)]
        rebuild: bool,
        /// Print a summary of the existing manifest and exit. No
        /// ingestion is performed.
        #[arg(long)]
        status: bool,
        /// Force a fresh `format.json` discovery call, overwriting
        /// any existing cache. Composes with `--no-format-discovery`
        /// (in which case the cache is rebuilt from the first-cut
        /// classifier alone). Mutually exclusive with `--format`.
        #[arg(long)]
        rediscover_format: bool,
        /// Load the `format.json` descriptor from this path and skip
        /// the discovery pipeline entirely. The supplied descriptor
        /// is NOT copied into `.sim-flow/spec-ingest/format.json`;
        /// the on-disk cache is left untouched. Overrides
        /// `--rediscover-format` and `--no-format-discovery`.
        #[arg(long)]
        format: Option<PathBuf>,
        /// Skip the LLM critique pass. The first-cut deterministic
        /// classifier still runs; its output is cached as
        /// `format.json` with `model = "first-cut-builtin"`. Useful
        /// for CI / offline environments and for the markdown / text
        /// source kinds where format discovery has no meaningful
        /// signal.
        #[arg(long)]
        no_format_discovery: bool,
        /// LLM backend for the format-discovery critique pass.
        /// Recognised values are the same as `sim-flow session` /
        /// `sim-flow auto`: `vllm`, `openai-compat`, `ollama`,
        /// `lmstudio`, `anthropic`, `claude`, `codex`,
        /// `gh-copilot`. When unset, the discovery pipeline falls
        /// back to the deterministic first-cut classifier (same as
        /// `--no-format-discovery`) and prints a stderr hint.
        #[arg(long, env = "SIM_FLOW_INGEST_LLM_BACKEND")]
        llm_backend: Option<String>,
        /// Model identifier for the format-discovery LLM. Required
        /// for backends that take a `model` argument
        /// (`vllm` / `openai-compat` / `ollama` / `lmstudio`).
        #[arg(long, env = "SIM_FLOW_INGEST_LLM_MODEL")]
        llm_model: Option<String>,
        /// Optional explicit model-family override (e.g.
        /// `claude-sonnet-4-6`, `qwen-3-coder`). When unset the
        /// agent infers a family from the configured model id.
        #[arg(long, env = "SIM_FLOW_INGEST_LLM_MODEL_FAMILY")]
        llm_model_family: Option<String>,
        /// Optional explicit runtime capability profile override.
        #[arg(long, env = "SIM_FLOW_INGEST_LLM_RUNTIME_PROFILE")]
        llm_runtime_profile: Option<String>,
        /// Base URL for HTTP-based LLM backends. For local vLLM at
        /// `localhost:8012` pass
        /// `--llm-base-url http://localhost:8012/v1`. Same precedence
        /// rules as `sim-flow session`'s `--llm-base-url` (generic
        /// override wins over the per-backend
        /// `--ollama-base-url` / `--openai-base-url` legacy flags).
        #[arg(long, env = "SIM_FLOW_INGEST_LLM_BASE_URL")]
        llm_base_url: Option<String>,
    },
    /// Discover running orchestrators that have a `--watch-socket`
    /// observer surface bound. Reads the registry directory each
    /// `sim-flow auto --watch-socket ...` registers on bind and
    /// removes on shutdown; stale entries (process gone, socket
    /// missing) are silently dropped from the list. Used by the
    /// dashboard's "Attach to running session" picker and any
    /// external script that wants to tail an in-flight run.
    Watchers {
        #[command(subcommand)]
        action: WatchersAction,
    },
    /// Inspect structured LLM-dispatch metrics for this project.
    /// Reads `.sim-flow/logs/llm-metrics.jsonl` -- one row per
    /// `RequestLlmResponse` round-trip emitted by `run_session` --
    /// and renders aggregates by step / kind / backend / model.
    /// Each row carries wall-time, prompt/completion bytes, and a
    /// byte-based token estimate; use this when you want to know
    /// where time and tokens went without parsing the raw JSONL.
    Metrics {
        /// Aggregation axis. `step` (default) groups by step
        /// (DM0 / DM1 / ...). `kind` groups by work / critique /
        /// qa. `backend` groups by the backend label. `model`
        /// groups by model id. `raw` prints every row verbatim
        /// in JSON (handy for piping into jq).
        #[arg(long, value_enum, default_value_t = MetricsGroupBy::Step)]
        group_by: MetricsGroupBy,
        /// Emit machine-readable JSON instead of a human table.
        /// `raw` always emits JSON regardless of this flag.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Diagnostics for the embedding client (Chapter 5 §5.9).
    /// `sim-flow embedder check` validates that the configured
    /// `embedder.toml` connects to its provider and the returned
    /// vector dimension matches the configured `dimension`. Use
    /// when configuring a new project's embedder for the first
    /// time, when an index build fails with a dimension error, or
    /// when a retrieval tool surfaces "embedder unreachable" mid-
    /// session.
    Embedder {
        #[command(subcommand)]
        action: EmbedderAction,
    },
    /// Build (or rebuild) the shared framework lance index
    /// (Chapter 3 §3.9.1). The output lands at
    /// `<out>/framework_chunks.lance/` plus a `manifest.toml` and
    /// an `embedder.toml` recording the embedder identity used at
    /// build time.
    BuildFrameworkIndex {
        /// Root of the framework workspace. Default: walk up from
        /// the project dir to discover sim-foundation, then point at
        /// `<root>/crates/framework/`.
        #[arg(long)]
        framework_root: Option<PathBuf>,
        /// Output root for the shared framework index. Default:
        /// `~/.sim-flow/lance-index/api/` (the `SIM_FLOW_API_INDEX_ROOT`
        /// env var overrides).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Explicit embedder config path (bypasses the project /
        /// env / home priority resolution).
        #[arg(long)]
        embedder: Option<PathBuf>,
        /// Force a full re-embed even if not stale.
        #[arg(long)]
        force: bool,
    },
    /// Build (or rebuild) the per-project spec lance index (Chapter
    /// 3 §3.9.2). Requires `sim-flow ingest` to have run first.
    /// Writes `<project>/.sim-flow/lance-index/` containing
    /// `spec_chunks.lance/`, `signal_table_rows.lance/`,
    /// `cross_spec_refs.lance/`, a `manifest.toml`, and an
    /// `embedder.toml`.
    BuildSpecIndex {
        /// Project root. Default: the global `--project` flag (or
        /// the current working dir).
        #[arg(long)]
        project: Option<PathBuf>,
        /// Explicit embedder config path.
        #[arg(long)]
        embedder: Option<PathBuf>,
        /// Force a full re-embed even if not stale.
        #[arg(long)]
        force: bool,
        /// Print the staleness state (Fresh / SourceChanged /
        /// SpecMdChanged / EmbedderChanged) and exit without
        /// rebuilding.
        #[arg(long)]
        check: bool,
    },
    /// Convenience: re-ingest the source spec and rebuild the
    /// per-project lance index in one command (Chapter 3 §3.9.3).
    /// Equivalent to `sim-flow ingest --rebuild` followed by
    /// `sim-flow build-spec-index`.
    RefreshSpec {
        /// Project root. Default: the global `--project` flag (or
        /// the current working dir).
        #[arg(long)]
        project: Option<PathBuf>,
    },
}

/// Subcommands under `sim-flow embedder`. Currently `check`; future
/// additions might include `ad-hoc embed "..."` or per-source
/// diagnostics.
#[derive(Debug, Subcommand)]
pub(crate) enum EmbedderAction {
    /// Resolve the embedder config and run a probe embed.
    Check {
        /// Explicit path to an `embedder.toml`. Bypasses the
        /// project / env / home priority resolution.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Print extra fields (header counts, retry policy, elapsed
        /// per attempt) on success.
        #[arg(long, default_value_t = false)]
        verbose: bool,
    },
}

/// Aggregation axis for `sim-flow metrics`. Mirrors a small set of
/// canonical roll-up dimensions; pivoting on additional axes (e.g.
/// `step+kind`) is a future enhancement.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum MetricsGroupBy {
    Step,
    Kind,
    Backend,
    Model,
    /// No aggregation: emit each row verbatim. Always JSON.
    Raw,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum FlowArg {
    DirectModeling,
    DesignStudy,
    SystemverilogConvert,
}

/// Which plan-kind to read in `sim-flow plan-progress`. Mirrors
/// `crate::plan_progress::PlanKind` minus the `None` variant.
#[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
pub(crate) enum PlanKindArg {
    Impl,
    Test,
    Perf,
}

/// Step-axis mode. Orthogonal to `SessionMode` (transport / agent
/// lifecycle). `Auto` runs the full work → critique → advance loop
/// unattended; `Manual` parks the orchestrator after the hello
/// handshake and dispatches sub-sessions only in response to host
/// commands. The flag is live: `SetStepMode { mode }` flips it
/// mid-run, and the existing cap-exceeded "drop to interactive"
/// path also flips it from auto → manual.
#[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
pub(crate) enum StepMode {
    Auto,
    Manual,
}

impl From<StepMode> for sim_flow::__internal::session::protocol::StepMode {
    fn from(value: StepMode) -> Self {
        match value {
            StepMode::Auto => Self::Auto,
            StepMode::Manual => Self::Manual,
        }
    }
}

#[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
pub(crate) enum SessionMode {
    /// Spawn a fresh interactive CLI-agent session per step. The user
    /// types `/exit` to finish each step; the orchestrator runs the
    /// gate and advances state automatically.
    PerStep,
    /// Keep one agent session alive across the whole flow with lazy
    /// re-spawn. Pass 2; the user manually triggers gate / advance
    /// via the dashboard control socket. Currently falls back to
    /// per-step until that landing.
    Single,
}

impl From<FlowArg> for Flow {
    fn from(value: FlowArg) -> Self {
        match value {
            FlowArg::DirectModeling => Flow::DirectModeling,
            FlowArg::DesignStudy => Flow::DesignStudy,
            FlowArg::SystemverilogConvert => Flow::SystemVerilogConvert,
        }
    }
}

/// Parse `--peer id=path` argument values. Splits on the first `=`
/// and returns `(id, path)`.
fn parse_peer_arg(s: &str) -> Result<(String, PathBuf), String> {
    let (id, rest) = s
        .split_once('=')
        .ok_or_else(|| format!("expected --peer <id>=<path>, got `{s}`"))?;
    if id.is_empty() {
        return Err(format!("peer id is empty in `{s}`"));
    }
    if rest.is_empty() {
        return Err(format!("peer path is empty in `{s}`"));
    }
    Ok((id.to_string(), PathBuf::from(rest)))
}
