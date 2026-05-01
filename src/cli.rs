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
        /// `anthropic`, `ollama`).
        #[arg(long, default_value = "vscode")]
        llm_backend: String,
        /// Optional model identifier.
        #[arg(long)]
        llm_model: Option<String>,
        /// Per-session structural-gate iteration cap.
        #[arg(long, default_value_t = 3)]
        max_auto_iters: u32,
        /// Cross-session retry cap when the critique reports blockers.
        #[arg(long, default_value_t = 3)]
        max_critique_iters: u32,
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
        /// Lifecycle for interactive CLI-agent backends (currently
        /// only `claude`). `per-step` (default) spawns a fresh agent
        /// per step. `single` (Pass 2) keeps one agent across the
        /// whole flow with lazy re-spawn. Ignored for non-interactive
        /// backends (vscode / anthropic / openai / ollama / lmstudio).
        #[arg(long, value_enum, default_value_t = SessionMode::PerStep)]
        session_mode: SessionMode,
        /// Hard cap on total LLM requests per work / critique
        /// sub-session. Backstop against runaway-loop bugs that the
        /// more specific `max_auto_iters` / `max_critique_iters`
        /// caps don't catch. 0 disables the check (NOT recommended).
        #[arg(long, default_value_t = 50)]
        max_llm_requests: u32,
        /// Number of structurally-identical assistant responses in a
        /// row that triggers an auto-abort. Digits / whitespace are
        /// normalized before comparing so timestamp-shaped churn
        /// doesn't defeat the check. 0 or 1 disables.
        #[arg(long, default_value_t = 3)]
        max_identical_responses: u32,
    },
    Session {
        /// Step id and kind, e.g. `DM0.work` or `DM2c.critique`.
        step_kind: String,
        /// Speak the JSONL session protocol on stdio. Required for
        /// IDE hosts.
        #[arg(long)]
        jsonl: bool,
        /// LLM backend. With `--jsonl`: opaque label echoed back to
        /// the host. Without `--jsonl`: selects the built-in
        /// CliAgent (`claude`, `codex`, `gh-copilot`, `ollama`,
        /// `lmstudio`).
        #[arg(long, default_value = "vscode")]
        llm_backend: String,
        /// Optional model identifier. Required for LM Studio and
        /// useful for Ollama (defaults to `llama3.1`); ignored by
        /// `claude` / `codex` unless their CLI accepts a
        /// `--model` flag.
        #[arg(long)]
        llm_model: Option<String>,
        /// Override the Ollama base URL (default
        /// `http://localhost:11434/v1`). Only meaningful when
        /// `--llm-backend ollama`.
        #[arg(long)]
        ollama_base_url: Option<String>,
        /// Override the LM Studio base URL (default
        /// `http://localhost:1234/v1`). Only meaningful when
        /// `--llm-backend lmstudio`.
        #[arg(long)]
        lmstudio_base_url: Option<String>,
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
