use std::path::PathBuf;

use clap::{Parser, Subcommand};
use sim_flow::__internal::state::Flow;

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
    /// Reset a step and cascade to downstream gates.
    Reset {
        /// Step id to reset.
        step: String,
    },
    /// Show or set configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
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
        /// Optional source-spec path (.md / .markdown / .txt). The
        /// spec is copied into the project, chunked into per-page
        /// markdown under `.sim-flow/spec-pages/`, and a TOC is
        /// inlined into every session's system prompt. PDF support
        /// is Phase 5 and currently errors with a clear message.
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
    /// enforces these against the live `cargo tarpaulin` report.
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
}

#[derive(Debug, Subcommand)]
pub(crate) enum KeysAction {
    /// Store an API key in `<config>/sim-flow/credentials.toml`.
    /// The file is created (and made owner-only `0600` on POSIX) if
    /// it doesn't already exist; existing entries for other
    /// providers are preserved.
    Set {
        /// Provider id (`anthropic`, `openai`, `ollama`, `lmstudio`).
        provider: String,
        /// Read the key from this env var instead of prompting on
        /// stdin. Useful for scripted setup
        /// (`ANTHROPIC_API_KEY=… sim-flow keys set anthropic
        /// --from-env ANTHROPIC_API_KEY`).
        #[arg(long)]
        from_env: Option<String>,
    },
    /// Remove a provider's entry from `credentials.toml`. The env
    /// var is untouched (the CLI can't edit your shell rc); a key
    /// resolution can still succeed via the env var after `clear`.
    Clear {
        /// Provider id (`anthropic`, `openai`, `ollama`, `lmstudio`).
        provider: String,
    },
    /// Show, per provider, whether a key is reachable and via which
    /// source. Never prints the key value itself.
    List {
        /// Emit machine-readable JSON for tooling.
        #[arg(long)]
        json: bool,
    },
    /// Print the absolute path the credentials file would live at.
    /// Useful for `cat`-ing or editing it directly when scripting.
    Path,
}

#[derive(Debug, Subcommand)]
pub(crate) enum WatchersAction {
    /// List every live watcher registration.
    List {
        /// Emit machine-readable JSON for the dashboard.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum PromptsAction {
    /// List every available prompt and which scope is currently
    /// providing its content.
    List {
        /// Emit machine-readable JSON for the dashboard.
        #[arg(long)]
        json: bool,
    },
    /// Print the resolved content of a prompt.
    Show {
        /// Slug + kind, e.g. `dm0-specification.work` or
        /// `dm2c-model-impl-plan.critique`.
        slug_kind: String,
    },
    /// Persist an override at the chosen scope. Reads new content
    /// from stdin (so the dashboard can pipe edits through without a
    /// temp file).
    Save {
        /// Slug + kind (see `show`).
        slug_kind: String,
        /// Where to save: `project` or `global`.
        #[arg(long)]
        scope: PromptScopeArg,
    },
    /// Remove an override at the chosen scope (project / global / all).
    Reset {
        slug_kind: String,
        #[arg(long)]
        scope: PromptResetScope,
    },
    /// Print the absolute path of the resolved (or scope-specific) file.
    /// Useful for editor integrations that want to open the file.
    Path {
        slug_kind: String,
        /// Limit to one scope; default returns the active scope.
        #[arg(long)]
        scope: Option<PromptScopeArg>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum CoverageAction {
    /// Print the current threshold and level. Use `--json` for
    /// machine-readable output (the dashboard reads this).
    Show {
        #[arg(long)]
        json: bool,
    },
    /// Update one or both fields. Either flag may be omitted to
    /// keep its current value; passing neither is a no-op (but
    /// still legal -- it prints the unchanged settings so callers
    /// can use `set` as a verbose `show`).
    Set {
        /// Required line-coverage percentage. Clamped to
        /// `[0.0, 100.0]` before being written.
        #[arg(long)]
        threshold_pct: Option<f32>,
        /// Coverage level. `module` requires every module to hit
        /// the threshold; `total` only requires the project-wide
        /// total to do so.
        #[arg(long, value_enum)]
        level: Option<CoverageLevelArg>,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
pub(crate) enum CoverageLevelArg {
    Module,
    Total,
}

impl From<CoverageLevelArg> for sim_flow::__internal::config::CoverageLevel {
    fn from(value: CoverageLevelArg) -> Self {
        match value {
            CoverageLevelArg::Module => Self::Module,
            CoverageLevelArg::Total => Self::Total,
        }
    }
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum PromptScopeArg {
    Project,
    Global,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum PromptResetScope {
    Project,
    Global,
    All,
}

#[derive(Debug, Subcommand)]
pub(crate) enum BaselineAction {
    /// Create a named baseline from the most recent run (or `--run`).
    Create {
        name: String,
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Compare the given baseline against the most recent run (or
    /// `--current`).
    Compare {
        name: String,
        #[arg(long)]
        current: Option<String>,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// List all baselines.
    List {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum NewKind {
    /// Create a new Direct Modeling Flow project (DM0-DM4).
    Model {
        /// Project name (used for directory and crate name).
        name: String,
        /// Destination directory. Default: current directory.
        #[arg(long)]
        destination: Option<PathBuf>,
        /// Relative path from the generated project to sim-models/library.
        #[arg(long, default_value = "../../library")]
        library_path: String,
        /// Skip the post-generation `cargo check` validation.
        #[arg(long)]
        skip_cargo_check: bool,
        /// Emit machine-readable JSON describing the generated project.
        #[arg(long)]
        json: bool,
    },
    /// (Phase 5) Create a new Design Study Flow project.
    Study { name: String },
    /// (Phase 5) Create a new candidate inside an existing study.
    Candidate { name: String },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigAction {
    /// Print the effective configuration.
    Show,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum FlowArg {
    DirectModeling,
    DesignStudy,
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(argv: &[&str]) -> Cli {
        Cli::try_parse_from(argv).expect("clap should accept the test argv")
    }

    // ---------- StepMode / FlowArg conversions ----------

    #[test]
    fn step_mode_into_protocol_round_trips() {
        assert_eq!(
            sim_flow::__internal::session::protocol::StepMode::from(StepMode::Auto),
            sim_flow::__internal::session::protocol::StepMode::Auto,
        );
        assert_eq!(
            sim_flow::__internal::session::protocol::StepMode::from(StepMode::Manual),
            sim_flow::__internal::session::protocol::StepMode::Manual,
        );
    }

    #[test]
    fn flow_arg_into_state_flow_round_trips() {
        assert!(matches!(
            Flow::from(FlowArg::DirectModeling),
            Flow::DirectModeling
        ));
        assert!(matches!(
            Flow::from(FlowArg::DesignStudy),
            Flow::DesignStudy
        ));
    }

    // ---------- Auto subcommand: --llm-base-url plumbing ----------

    #[test]
    fn auto_default_omits_llm_base_url() {
        let cli = parse(&["sim-flow", "auto"]);
        match cli.command {
            Command::Auto {
                llm_base_url,
                llm_backend,
                ..
            } => {
                assert_eq!(llm_base_url, None);
                assert_eq!(llm_backend, "vscode");
            }
            other => panic!("expected Command::Auto, got {other:?}"),
        }
    }

    #[test]
    fn auto_accepts_llm_base_url_flag() {
        let cli = parse(&[
            "sim-flow",
            "auto",
            "--llm-base-url",
            "http://my-vllm:8000/v1",
        ]);
        match cli.command {
            Command::Auto { llm_base_url, .. } => {
                assert_eq!(llm_base_url.as_deref(), Some("http://my-vllm:8000/v1"));
            }
            other => panic!("expected Command::Auto, got {other:?}"),
        }
    }

    #[test]
    fn auto_accepts_llm_base_url_with_other_llm_flags() {
        let cli = parse(&[
            "sim-flow",
            "auto",
            "--llm-backend",
            "vllm",
            "--llm-model",
            "qwen3.6:32b",
            "--llm-base-url",
            "http://prod-vllm:8000/v1",
        ]);
        match cli.command {
            Command::Auto {
                llm_backend,
                llm_model,
                llm_base_url,
                ..
            } => {
                assert_eq!(llm_backend, "vllm");
                assert_eq!(llm_model.as_deref(), Some("qwen3.6:32b"));
                assert_eq!(llm_base_url.as_deref(), Some("http://prod-vllm:8000/v1"));
            }
            other => panic!("expected Command::Auto, got {other:?}"),
        }
    }

    // ---------- Session subcommand: --llm-base-url plumbing ----------

    #[test]
    fn session_default_omits_all_base_url_flags() {
        let cli = parse(&["sim-flow", "session", "DM0.work"]);
        match cli.command {
            Command::Session {
                step_kind,
                ollama_base_url,
                openai_base_url,
                llm_base_url,
                ..
            } => {
                assert_eq!(step_kind, "DM0.work");
                assert_eq!(ollama_base_url, None);
                assert_eq!(openai_base_url, None);
                assert_eq!(llm_base_url, None);
            }
            other => panic!("expected Command::Session, got {other:?}"),
        }
    }

    #[test]
    fn session_accepts_all_three_url_flags_independently() {
        // Setting all three is unusual but legal -- the precedence
        // resolution happens later in `commands.rs::session_cmd` and
        // is exercised by the agent-side `resolved_base_url` tests.
        let cli = parse(&[
            "sim-flow",
            "session",
            "DM2c.critique",
            "--llm-backend",
            "vllm",
            "--llm-base-url",
            "http://generic",
            "--ollama-base-url",
            "http://o:11434/v1",
            "--openai-base-url",
            "http://lm:1234/v1",
        ]);
        match cli.command {
            Command::Session {
                ollama_base_url,
                openai_base_url,
                llm_base_url,
                llm_backend,
                ..
            } => {
                assert_eq!(llm_backend, "vllm");
                assert_eq!(llm_base_url.as_deref(), Some("http://generic"));
                assert_eq!(ollama_base_url.as_deref(), Some("http://o:11434/v1"));
                assert_eq!(openai_base_url.as_deref(), Some("http://lm:1234/v1"));
            }
            other => panic!("expected Command::Session, got {other:?}"),
        }
    }

    #[test]
    fn auto_default_session_mode_is_per_step() {
        let cli = parse(&["sim-flow", "auto"]);
        match cli.command {
            Command::Auto {
                session_mode,
                step_mode,
                no_preamble,
                ..
            } => {
                assert_eq!(session_mode, SessionMode::PerStep);
                assert_eq!(step_mode, StepMode::Auto);
                assert!(no_preamble);
            }
            other => panic!("expected Command::Auto, got {other:?}"),
        }
    }
}
