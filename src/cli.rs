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
    /// Reset a step and cascade to downstream gates.
    Reset {
        /// Step id to reset.
        step: String,
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

/// Catalog of named reports the `sim-flow db report <kind>` CLI knows
/// how to run. Each entry maps to one canned SQL query in
/// `db_reports`; adding a report is a one-line addition there plus a
/// new variant here.
#[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
pub(crate) enum DbReportKind {
    /// Bugs grouped by step, with category breakdown and count.
    /// Defaults to all projects; use `--project` to scope.
    BugsByStep,
    /// Bugs grouped by category, with per-category count.
    BugsByCategory,
    /// Most-recently-opened bugs across all projects. `--limit` to
    /// cap (default 20).
    BugsRecent,
    /// All currently-open bugs (status `open` or `manual`).
    BugsOpen,
    /// LLM-turn wall time and turn count grouped by step.
    LlmTimeByStep,
    /// LLM-turn wall time and token totals grouped by backend +
    /// model. Reveals "where the money goes" across backends.
    LlmTimeByBackend,
    /// LLM-turn wall time grouped by (step, kind) so the
    /// work-vs-critique cost split is visible per step.
    LlmTimeByKind,
    /// Per-tool wall time + invocation count. `caller_kind` mixed.
    /// Use the dedicated `gate-time-by-step` for gate-only.
    ToolTimeByTool,
    /// Wall time per (step, caller_kind) -- splits LLM-driven tool
    /// time from gate-driven shell time per step.
    ToolTimeByStep,
    /// Wall time per gate-driven shell command per step. Surfaces
    /// which step's gate is the slowest to evaluate.
    GateTimeByStep,
    /// Most-recently-recorded experiment runs across all projects.
    /// `--limit` to cap (default 20).
    ExperimentsRecent,
}

/// Catalog of named charts. Each renders as a horizontal Unicode-bar
/// histogram in the terminal -- one bar per label, length scaled to
/// the max value in the dataset. Future work will add an SVG renderer
/// on top of the existing `ChartFamily` machinery; v1 keeps the
/// surface terminal-only.
#[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
pub(crate) enum DbChartKind {
    /// Bug count per step, one bar per step.
    BugsByStep,
    /// Bug count per category, one bar per category.
    BugsByCategory,
    /// Total LLM-turn wall time per step.
    LlmTimeByStep,
    /// Total LLM-turn wall time per backend + model.
    LlmTimeByBackend,
    /// Tool wall time per tool name.
    ToolTimeByTool,
}

impl From<DbChartKind> for sim_flow::__internal::db_charts::ChartKind {
    fn from(value: DbChartKind) -> Self {
        use sim_flow::__internal::db_charts::ChartKind as L;
        match value {
            DbChartKind::BugsByStep => L::BugsByStep,
            DbChartKind::BugsByCategory => L::BugsByCategory,
            DbChartKind::LlmTimeByStep => L::LlmTimeByStep,
            DbChartKind::LlmTimeByBackend => L::LlmTimeByBackend,
            DbChartKind::ToolTimeByTool => L::ToolTimeByTool,
        }
    }
}

impl From<DbReportKind> for sim_flow::__internal::db_reports::ReportKind {
    fn from(value: DbReportKind) -> Self {
        use sim_flow::__internal::db_reports::ReportKind as L;
        match value {
            DbReportKind::BugsByStep => L::BugsByStep,
            DbReportKind::BugsByCategory => L::BugsByCategory,
            DbReportKind::BugsRecent => L::BugsRecent,
            DbReportKind::BugsOpen => L::BugsOpen,
            DbReportKind::LlmTimeByStep => L::LlmTimeByStep,
            DbReportKind::LlmTimeByBackend => L::LlmTimeByBackend,
            DbReportKind::LlmTimeByKind => L::LlmTimeByKind,
            DbReportKind::ToolTimeByTool => L::ToolTimeByTool,
            DbReportKind::ToolTimeByStep => L::ToolTimeByStep,
            DbReportKind::GateTimeByStep => L::GateTimeByStep,
            DbReportKind::ExperimentsRecent => L::ExperimentsRecent,
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum DbAction {
    /// Print the resolved path to the per-user global DB.
    /// `sqlite3 $(sim-flow db path)` is always an option for ad-hoc
    /// queries. Useful for confirming the data directory location
    /// across platforms.
    Path,
    /// Per-table row counts, last-write timestamps, schema version,
    /// and resolved machine identity / user identity for the live
    /// global DB. Read-only; safe to run during an auto session.
    Stats {
        /// Emit machine-readable JSON instead of the human table.
        #[arg(long)]
        json: bool,
    },
    /// Run a named cross-project report from the catalog against the
    /// per-user global DB. The catalog covers common "where did time
    /// go" / "what's flaky" / "what did I hit recently" questions;
    /// for anything else, fall back to `sim-flow db query`. If a
    /// query you've been writing repeatedly with `db query` becomes
    /// routine, promote it to a named report here.
    Report {
        /// Which report to run. See the catalog comment on
        /// `DbReportKind` for the full list of supported names.
        kind: DbReportKind,
        /// Restrict to rows whose `project_dir` contains this
        /// substring. Useful for "just rgb_toy please" or
        /// "everything under users/mneilly".
        #[arg(long)]
        project: Option<String>,
        /// Restrict to a specific step (e.g. `DM3c`, `SV2`).
        #[arg(long)]
        step: Option<String>,
        /// Max rows to emit for reports that support it
        /// (`bugs-recent`, `experiments-recent`, ...). Defaults to
        /// the report's own sensible cap.
        #[arg(long)]
        limit: Option<usize>,
        /// Emit machine-readable JSON instead of the text table.
        #[arg(long)]
        json: bool,
    },
    /// Render a named chart from the catalog. Terminal output uses
    /// Unicode block characters for the bars; one bar per label. Pass
    /// the same filters as `db report` to scope the underlying query.
    Chart {
        /// Which chart to render. See `DbChartKind` for the catalog.
        kind: DbChartKind,
        /// Restrict to rows whose `project_dir` contains this
        /// substring.
        #[arg(long)]
        project: Option<String>,
        /// Restrict to a specific step.
        #[arg(long)]
        step: Option<String>,
        /// Cap the number of bars (after sorting by value desc).
        /// Defaults to 20.
        #[arg(long)]
        limit: Option<usize>,
        /// Max width in characters of the longest bar. Defaults to
        /// 60. Useful to set to a smaller value on a narrow terminal.
        #[arg(long)]
        bar_width: Option<usize>,
    },
    /// Read-only SQL escape hatch over the per-user global DB. Useful
    /// for ad-hoc trend questions the named-report catalog doesn't
    /// cover yet -- if a query becomes routine, promote it.
    ///
    /// `PRAGMA query_only=ON` is set on the connection before the SQL
    /// runs, so INSERT / UPDATE / DELETE / DDL are rejected with a
    /// readonly-database error. Results print as a left-aligned text
    /// table by default; `--json` for machine-readable.
    Query {
        /// SQL to run. Single statement; multi-statement scripts are
        /// not supported. Wrap in single quotes for the shell.
        sql: String,
        /// Emit machine-readable JSON instead of the text table.
        #[arg(long)]
        json: bool,
    },
    /// One-shot importer for projects that pre-date the live mirror.
    /// Walks each given project's `.sim-flow/` directory and bulk-
    /// inserts every JSONL row and every `experiments.db` row into
    /// the global DB. Idempotent: re-running over a project that
    /// was already imported is a no-op (live mirrors use the same
    /// UNIQUE indexes; `tool_timings` deduplicates via a per-source
    /// byte-offset tracker in `meta`).
    ///
    /// Not part of the steady-state flow -- the per-entry mirror
    /// keeps the DB current with zero user action. Use this once
    /// when first installing the global-DB mirror on a machine with
    /// existing project history, or after restoring a project from
    /// backup.
    Backfill {
        /// Project directory (or any path whose ancestor is a
        /// project). Defaults to the current working directory.
        /// Multiple paths can be given; each is imported in
        /// sequence.
        #[arg(value_name = "PROJECT_DIR")]
        paths: Vec<PathBuf>,
        /// Re-import tool_timings even if a prior `db backfill` for
        /// the same project already covered them. Without this flag
        /// the importer uses a per-source byte offset stored in
        /// `meta` to skip already-imported lines.
        #[arg(long)]
        force_tool_timings: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum BugsAction {
    /// List bugs in `.sim-flow/bug-log.jsonl`. Default shows all
    /// statuses; filter with `--open` or `--resolved`.
    List {
        /// Only show open bugs.
        #[arg(long, conflicts_with_all = ["resolved"])]
        open: bool,
        /// Only show resolved bugs (status = "resolved" or
        /// "manual"). Equivalent to "not open."
        #[arg(long)]
        resolved: bool,
        /// Filter by step id (e.g. `--step DM3c`).
        #[arg(long)]
        step: Option<String>,
        /// Filter by category (`framework | test | impl | perf |
        /// tooling | other`).
        #[arg(long)]
        category: Option<String>,
    },
    /// Show one bug's full event trail.
    Show {
        /// Bug id (e.g. `bug-001`).
        id: String,
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
        assert!(matches!(
            Flow::from(FlowArg::SystemverilogConvert),
            Flow::SystemVerilogConvert
        ));
    }

    #[test]
    fn convert_sv_parses_without_force() {
        let cli = parse(&["sim-flow", "convert-sv"]);
        match cli.command {
            Command::ConvertSv { force } => assert!(!force, "default --force should be false"),
            other => panic!("expected Command::ConvertSv, got {other:?}"),
        }
    }

    #[test]
    fn convert_sv_parses_with_force() {
        let cli = parse(&["sim-flow", "convert-sv", "--force"]);
        match cli.command {
            Command::ConvertSv { force } => assert!(force, "--force must propagate"),
            other => panic!("expected Command::ConvertSv, got {other:?}"),
        }
    }

    #[test]
    fn bugs_list_parses_with_filters() {
        let cli = parse(&[
            "sim-flow",
            "bugs",
            "list",
            "--open",
            "--step",
            "DM3c",
            "--category",
            "framework",
        ]);
        match cli.command {
            Command::Bugs {
                action:
                    BugsAction::List {
                        open,
                        resolved,
                        step,
                        category,
                    },
            } => {
                assert!(open);
                assert!(!resolved);
                assert_eq!(step.as_deref(), Some("DM3c"));
                assert_eq!(category.as_deref(), Some("framework"));
            }
            other => panic!("expected Command::Bugs(List), got {other:?}"),
        }
    }

    #[test]
    fn bugs_show_requires_id() {
        let cli = parse(&["sim-flow", "bugs", "show", "bug-001"]);
        match cli.command {
            Command::Bugs {
                action: BugsAction::Show { id },
            } => assert_eq!(id, "bug-001"),
            other => panic!("expected Command::Bugs(Show), got {other:?}"),
        }
    }

    #[test]
    fn init_accepts_systemverilog_convert_flow() {
        let cli = parse(&["sim-flow", "init", "--flow", "systemverilog-convert"]);
        match cli.command {
            Command::Init { flow } => {
                let f: Flow = flow.into();
                assert!(matches!(f, Flow::SystemVerilogConvert));
            }
            other => panic!("expected Command::Init, got {other:?}"),
        }
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

    // ---------- Auto subcommand: --critique-llm-* plumbing ----------

    #[test]
    fn auto_default_omits_all_critique_llm_flags() {
        // Default `sim-flow auto` keeps the critique stack
        // implicit (everything falls back to the work-side
        // `--llm-*` knobs in the orchestrator's
        // `resolve_llm_for_kind`).
        let cli = parse(&["sim-flow", "auto"]);
        match cli.command {
            Command::Auto {
                critique_llm_backend,
                critique_llm_model,
                critique_llm_model_family,
                critique_llm_runtime_profile,
                critique_llm_base_url,
                ..
            } => {
                assert_eq!(critique_llm_backend, None);
                assert_eq!(critique_llm_model, None);
                assert_eq!(critique_llm_model_family, None);
                assert_eq!(critique_llm_runtime_profile, None);
                assert_eq!(critique_llm_base_url, None);
            }
            other => panic!("expected Command::Auto, got {other:?}"),
        }
    }

    #[test]
    fn auto_accepts_critique_llm_backend_flag() {
        // The canonical use case: vLLM for work, Anthropic for
        // critique. Every flag should land in its own field; we
        // assert each one individually so a renamed destination
        // surfaces the regression.
        let cli = parse(&[
            "sim-flow",
            "auto",
            "--llm-backend",
            "vllm",
            "--llm-model",
            "qwen3.6",
            "--critique-llm-backend",
            "anthropic",
            "--critique-llm-model",
            "claude-3-5-sonnet-latest",
            "--critique-llm-model-family",
            "claude_messages",
            "--critique-llm-runtime-profile",
            "anthropic_messages",
            "--critique-llm-base-url",
            "https://api.anthropic.com",
        ]);
        match cli.command {
            Command::Auto {
                llm_backend,
                llm_model,
                critique_llm_backend,
                critique_llm_model,
                critique_llm_model_family,
                critique_llm_runtime_profile,
                critique_llm_base_url,
                ..
            } => {
                assert_eq!(llm_backend, "vllm");
                assert_eq!(llm_model.as_deref(), Some("qwen3.6"));
                assert_eq!(critique_llm_backend.as_deref(), Some("anthropic"));
                assert_eq!(
                    critique_llm_model.as_deref(),
                    Some("claude-3-5-sonnet-latest")
                );
                assert_eq!(
                    critique_llm_model_family.as_deref(),
                    Some("claude_messages")
                );
                assert_eq!(
                    critique_llm_runtime_profile.as_deref(),
                    Some("anthropic_messages")
                );
                assert_eq!(
                    critique_llm_base_url.as_deref(),
                    Some("https://api.anthropic.com")
                );
            }
            other => panic!("expected Command::Auto, got {other:?}"),
        }
    }

    #[test]
    fn auto_partial_critique_override_parses_without_complaint() {
        // The CLI doesn't validate that critique flags are
        // self-consistent (e.g. model set but backend unset).
        // The orchestrator emits a Diagnostic at session start
        // when it spots a partial override (see auto.rs); this
        // test pins down that the CLI itself stays permissive
        // -- the warning is informational, not a parse error.
        let cli = parse(&[
            "sim-flow",
            "auto",
            "--critique-llm-model",
            "claude-3-5-sonnet-latest",
        ]);
        match cli.command {
            Command::Auto {
                critique_llm_backend,
                critique_llm_model,
                ..
            } => {
                assert_eq!(critique_llm_backend, None);
                assert_eq!(
                    critique_llm_model.as_deref(),
                    Some("claude-3-5-sonnet-latest")
                );
            }
            other => panic!("expected Command::Auto, got {other:?}"),
        }
    }
}
