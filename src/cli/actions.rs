//! Per-subcommand action enums and the small value-enum types they
//! depend on. Kept in one file because each module is short
//! (5-100 lines apiece) and they share a common pattern (clap
//! Subcommand or ValueEnum derive).

use std::path::PathBuf;

use clap::Subcommand;

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
