//! Agent-callable tools the orchestrator advertises to the LLM.
//!
//! Two classes of capability live in `sim-flow` (see
//! docs/architecture/ai-flow/08-orchestrator-tools.md):
//!
//! - **Agent-callable tools** in this module: bounded I/O the LLM
//!   uses during a turn (read_file, list_dir, write_file, search).
//!   Path-sandboxed to the project directory.
//! - **Orchestrator-only validators** in `session::runners`: cargo
//!   check / test / coverage. Never invoked by the agent directly.
//!
//! Tools execute inside the orchestrator (filesystem + grep). Hosts
//! get `ToolInvoked` notifications for display. If we ever need
//! user approval before a destructive op, we'd add a
//! `RequestToolApproval` event - explicit protocol extension, not a
//! quiet behavior change.

use std::path::{Path, PathBuf};

use crate::__internal::session::ask_user::SuspendOutcome;
use crate::{Error, Result};

mod api_common;
mod api_expand_macro;
mod api_hover;
mod api_impls;
mod api_references;
mod api_search;
mod api_semantic_search;
mod ask_user;
mod declare_fix;
mod declare_hypothesis;
mod delete_file;
mod edit_file;
mod list_dir;
mod log_bug;
mod read_file;
mod read_markdown;
mod record_run;
mod resolve_bug;
mod run_cargo;
mod search;
mod signal_table_query;
mod spec_semantic_search;
mod write_file;

pub use api_expand_macro::ApiExpandMacroTool;
pub use api_hover::ApiHoverTool;
pub use api_impls::ApiImplsTool;
pub use api_references::ApiReferencesTool;
pub use api_search::ApiSearchTool;
pub use api_semantic_search::ApiSemanticSearchTool;
pub use ask_user::{ASK_USER_TURN_CAP, AskUserTool};
pub use declare_fix::DeclareFixTool;
pub use declare_hypothesis::DeclareHypothesisTool;
pub use delete_file::DeleteFileTool;
pub use edit_file::EditFileTool;
pub use list_dir::ListDirTool;
pub use log_bug::LogBugTool;
pub use read_file::ReadFileTool;
pub use read_markdown::ReadMarkdownTool;
pub use record_run::RecordRunTool;
pub use resolve_bug::ResolveBugTool;
pub use run_cargo::RunCargoTool;
pub use search::SearchTool;
pub use signal_table_query::SignalTableQueryTool;
pub use spec_semantic_search::SpecSemanticSearchTool;
pub use write_file::WriteFileTool;

/// Roots a tool may resolve paths against. `project_dir` is read+write;
/// `library_root` (when present) is read-only and is the auto-detected
/// sim-models repo containing `docs/`, `examples/`, and `library/`.
/// `framework_root` (also read-only) points at the
/// `<foundation>/crates/framework/` crate so the agent can read the
/// framework's source-level API surface when needed.
/// `framework_docs_root` (also read-only) points at the normalized API
/// docs root (the directory that contains `toc.md` and `pages/`).
/// Tools accept paths in three forms:
///
/// - bare relative path -- resolves under `project_dir` (existing
///   behavior; what `write_file` and the artifact-write convention use).
/// - `lib:<rel>` prefix -- resolves under `library_root`. Tools that
///   only read (`read_file`, `list_dir`, `search`) honor this prefix;
///   `write_file` rejects it.
/// - `fw:<rel>` prefix -- resolves under the framework assets. Read-only
///   like `lib:`. `fw:api/...` reads normalized API docs (start with
///   `fw:api/toc.md`); other `fw:` paths read the framework source tree
///   (for example `fw:src/prelude.rs`).
pub struct ToolContext<'a> {
    pub project_dir: &'a Path,
    pub library_root: Option<&'a Path>,
    pub framework_root: Option<&'a Path>,
    pub framework_docs_root: Option<&'a Path>,
    /// Project-relative path prefixes the current step+kind allows
    /// writes to. Empty list means no writes are allowed in this
    /// session. `WriteFileTool` and `EditFileTool` enforce this; the
    /// orchestrator's artifact-write extractor enforces the same set
    /// before persisting fenced ` ```path ` blocks.
    pub write_paths: &'a [String],
    /// When the current sub-session is scoped to a milestone, the
    /// orchestrator passes the milestone file's body here so
    /// `WriteFileTool` can autocorrect paths: if the agent writes
    /// `src/foo.rs` but the milestone references `src/model/foo.rs`,
    /// the tool redirects the write and tells the agent. `None`
    /// outside milestone-walk steps. Empty string is fine (no
    /// hints; no redirect).
    pub current_milestone_body: Option<&'a str>,
    /// Project-relative path of the current milestone file when the
    /// orchestrator has scoped the sub-session to one (e.g.
    /// `docs/test-plan/test-milestone-03-stress.md`). Used by
    /// `log_bug` to record which specific milestone surfaced the
    /// bug -- valuable when mining the log later ("which milestone
    /// types fail most often?"). `None` outside milestone-walk
    /// steps. Parallel to `current_milestone_body`.
    pub current_milestone_path: Option<&'a str>,
    /// Project-relative paths the user explicitly approved for
    /// `delete_file` even though they fall outside `write_paths`.
    /// Populated by the orchestrator after a `RequestUserInput`
    /// scope-override prompt in interactive mode; consumed by
    /// `DeleteFileTool::invoke`. Empty in auto mode (per the
    /// design decision that auto runs do NOT pause for tool
    /// approvals).
    pub approved_deletes: &'a [String],
    /// Step id of the flow step that's currently running. Mutating
    /// tools (`write_file`, `edit_file`, `delete_file`) append the
    /// path they touched to this step's manifest so a later
    /// `sim-flow reset` can clean exactly the files the step
    /// produced. `None` in synthetic contexts (unit tests, the
    /// dashboard's spec-ingest probe) -- those don't need
    /// manifests because they don't gate-advance.
    pub step_id: Option<&'a str>,
}

impl<'a> ToolContext<'a> {
    pub fn new(
        project_dir: &'a Path,
        library_root: Option<&'a Path>,
        framework_root: Option<&'a Path>,
        framework_docs_root: Option<&'a Path>,
    ) -> Self {
        Self {
            project_dir,
            library_root,
            framework_root,
            framework_docs_root,
            write_paths: &[],
            current_milestone_body: None,
            current_milestone_path: None,
            approved_deletes: &[],
            step_id: None,
        }
    }

    pub fn with_write_paths(mut self, write_paths: &'a [String]) -> Self {
        self.write_paths = write_paths;
        self
    }

    pub fn with_milestone_body(mut self, body: Option<&'a str>) -> Self {
        self.current_milestone_body = body;
        self
    }

    pub fn with_milestone_path(mut self, path: Option<&'a str>) -> Self {
        self.current_milestone_path = path;
        self
    }

    pub fn with_approved_deletes(mut self, approved: &'a [String]) -> Self {
        self.approved_deletes = approved;
        self
    }

    pub fn with_step_id(mut self, step_id: &'a str) -> Self {
        self.step_id = Some(step_id);
        self
    }
}

/// Distinctive marker the orchestrator scans for after a tool
/// dispatch to detect "delete_file refused because the path is
/// outside this step's write allowlist." Surfaced as a stable
/// prefix on the tool's err display so a future refactor (e.g.
/// returning structured violation data instead of a string) doesn't
/// silently break the orchestrator's RequestUserInput trigger.
/// Public so the orchestrator and tests can match on it; the
/// suffix after the marker is the offending path.
pub const DELETE_SCOPE_VIOLATION_MARKER: &str = "delete_file: scope-violation:";

/// Render a single-line preview of `s` for human-readable tool-arg
/// display. Escapes newlines/tabs/quotes/backslashes the way debug
/// formatting would, then caps the visible char count at `max_chars`
/// and wraps the result in double quotes so empty strings show as `""`
/// and the boundary is unambiguous. Counts characters (not bytes) so a
/// UTF-8 boundary is never split.
pub fn preview_one_line(s: &str, max_chars: usize) -> String {
    let mut escaped = String::with_capacity(s.len().min(max_chars * 2 + 8));
    let mut truncated = false;
    for (chars, c) in s.chars().enumerate() {
        if chars >= max_chars {
            truncated = true;
            break;
        }
        match c {
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            c if (c as u32) < 0x20 => escaped.push_str(&format!("\\x{:02x}", c as u32)),
            c => escaped.push(c),
        }
    }
    if truncated {
        format!("\"{escaped}...\"")
    } else {
        format!("\"{escaped}\"")
    }
}

/// Common shape every tool implements. The orchestrator dispatches
/// by name; a tool's `args` is a JSON object whose schema each impl
/// documents inline.
pub trait Tool {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// JSON Schema for the tool's args. Used both for native tool-use
    /// advertisement (Anthropic / OpenAI) and as an inline reminder
    /// for the fenced-block fallback.
    fn args_schema(&self) -> serde_json::Value;
    /// Execute the tool. Path-sandbox checks live in each impl (or a
    /// shared helper in this module).
    fn invoke(&self, ctx: &ToolContext, args: &serde_json::Value) -> Result<ToolResult>;
}

/// Result of a single tool invocation. The orchestrator threads
/// `display` into the next user-message LLM turn so the model sees
/// the tool's output directly. `ok` controls whether the chat UI
/// renders a success or error annotation. `attachments` carries
/// binary content (e.g. an image returned by `read_file` against a
/// `.jpg` / `.png` file) that the orchestrator forwards to the host
/// as multimodal parts on the next user message; tools that only
/// produce text leave it empty. The struct itself is internal --
/// `display` is what serializes through `ToolInvoked.args_summary`,
/// not the whole result.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub ok: bool,
    pub display: String,
    pub attachments: Vec<ToolAttachment>,
    /// Number of failing tests reported by `run_cargo` when the
    /// command was `test`. `None` for every other tool, and for
    /// successful test runs. The orchestrator's auto-iter loop reads
    /// this to detect progress: a strictly-decreasing count between
    /// turns resets the no-artifact iteration counter, so an agent
    /// that fixes one of N test failures per turn isn't bailed out
    /// by the per-session cap when it's still making progress.
    pub test_failure_count: Option<usize>,
    /// Names of the failing tests reported by `run_cargo` when the
    /// command was `test`. Parallel to `test_failure_count` (same
    /// `None` semantics). Used by the auto-iter loop's
    /// fix-vs-investigation classifier: regression detection
    /// (current ⊄ target) and progress detection (target.intersection
    /// shrank) both key on the names, not the raw count, so the
    /// "fixed A, broke B" 1-for-1 case doesn't look identical to
    /// "made no progress."
    pub test_failures: Option<Vec<String>>,
    /// Project-relative paths this tool mutated (created, edited, or
    /// deleted). Populated by `write_file`, `edit_file`,
    /// `delete_file`, and the orchestrator's artifact-extract path.
    /// Empty for read-only tools. The auto-iter loop intersects this
    /// against the step's manifest snapshot to classify a turn as
    /// "modifies existing artifacts" (fix attempt) vs. "only adds new
    /// files / reads" (data collection / diagnostic).
    pub touched_paths: Vec<String>,
    /// `true` when this tool call is the agent's explicit commit to
    /// "the next `cargo test` is a fix attempt." Only `declare_fix`
    /// sets it. The orchestrator treats the next test run as a fix
    /// attempt regardless of file-op state, increments
    /// `declared_fixes_count`, and resets the investigation counter.
    /// Composes with file-op heuristic: a turn that also touched a
    /// pre-session path is still a fix attempt (one classification,
    /// two counters in parallel).
    pub declared_fix: bool,
    /// When `Some`, this tool call has suspended the LLM turn pending
    /// a user reply. The orchestrator's dispatch loop must derive a
    /// `RequestUserInput` event from `pending`, park the work
    /// session, and exit the current LLM turn cleanly (discarding
    /// any subsequent tool calls in the same model response with a
    /// `tool_calls_after_ask_user` warning per Architecture §6.5.1).
    /// The resume hook returns the user's reply as the suspended
    /// call's tool-result on the next LLM turn. Only the `ask_user`
    /// tool sets this today.
    pub suspend: Option<SuspendOutcome>,
}

/// Internal-only; never serialized over the JSONL protocol. The
/// orchestrator converts these to base64-encoded `LlmAttachment`s
/// before they cross the boundary.
#[derive(Debug, Clone)]
pub struct ToolAttachment {
    pub mime: String,
    pub bytes: Vec<u8>,
    pub source_path: String,
}

impl ToolResult {
    pub fn ok(display: impl Into<String>) -> Self {
        Self {
            ok: true,
            display: display.into(),
            attachments: Vec::new(),
            test_failure_count: None,
            test_failures: None,
            touched_paths: Vec::new(),
            declared_fix: false,
            suspend: None,
        }
    }
    pub fn err(display: impl Into<String>) -> Self {
        Self {
            ok: false,
            display: display.into(),
            attachments: Vec::new(),
            test_failure_count: None,
            test_failures: None,
            touched_paths: Vec::new(),
            declared_fix: false,
            suspend: None,
        }
    }
    pub fn ok_with_attachment(
        display: impl Into<String>,
        mime: impl Into<String>,
        bytes: Vec<u8>,
        source_path: impl Into<String>,
    ) -> Self {
        Self {
            ok: true,
            display: display.into(),
            attachments: vec![ToolAttachment {
                mime: mime.into(),
                bytes,
                source_path: source_path.into(),
            }],
            test_failure_count: None,
            test_failures: None,
            touched_paths: Vec::new(),
            declared_fix: false,
            suspend: None,
        }
    }
    pub fn with_test_failure_count(mut self, count: usize) -> Self {
        self.test_failure_count = Some(count);
        self
    }
    pub fn with_test_failures(mut self, names: Vec<String>) -> Self {
        self.test_failures = Some(names);
        self
    }
    pub fn with_touched_path(mut self, path: impl Into<String>) -> Self {
        self.touched_paths.push(path.into());
        self
    }
    pub fn with_declared_fix(mut self) -> Self {
        self.declared_fix = true;
        self
    }
    /// Construct a tool result that signals "the LLM turn is
    /// suspended pending a user reply." Used by `ask_user`; the
    /// dispatch loop reads `suspend.as_ref()` to detect the
    /// suspension. `display` is left empty because the suspended
    /// result is never threaded back to the model — the resume path
    /// returns the user's actual answer on the next turn.
    pub fn suspended(outcome: SuspendOutcome) -> Self {
        Self {
            ok: true,
            display: String::new(),
            attachments: Vec::new(),
            test_failure_count: None,
            test_failures: None,
            touched_paths: Vec::new(),
            declared_fix: false,
            suspend: Some(outcome),
        }
    }
}

/// Maps a path's extension to a standard image MIME type. Returns
/// `None` for non-image extensions; callers fall back to text-mode
/// reads.
pub fn image_mime_from_path(path: &Path) -> Option<&'static str> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("png") => Some("image/png"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        _ => None,
    }
}

/// Build the orchestrator's tool dispatcher from a list of tool
/// names. Unknown names are skipped silently so descriptors can
/// declare aspirational tools that aren't implemented yet.
///
/// The Phase 5 tools (`api_semantic_search`, `spec_semantic_search`,
/// `signal_table_query`, `ask_user`) require runtime state
/// (`Arc<RetrievalService>` / `Arc<AskUserRuntime>`). This entry
/// point omits them. Callers that have the runtime should use
/// [`build_dispatcher_with_runtime`] instead.
pub fn build_dispatcher(names: &[&'static str]) -> Vec<Box<dyn Tool>> {
    build_dispatcher_with_runtime(names, None, None)
}

/// Build the dispatcher with the Phase 5 stateful tools wired in.
/// Each `Arc<_>` is optional so callers that don't have a retrieval
/// service or ask_user runtime can still build a partial catalog.
pub fn build_dispatcher_with_runtime(
    names: &[&'static str],
    retrieval: Option<std::sync::Arc<crate::__internal::session::retrieval::RetrievalService>>,
    ask_user: Option<std::sync::Arc<crate::__internal::session::ask_user::AskUserRuntime>>,
) -> Vec<Box<dyn Tool>> {
    let mut out: Vec<Box<dyn Tool>> = Vec::new();
    for name in names {
        match *name {
            "read_file" => out.push(Box::new(ReadFileTool)),
            "read_markdown" => out.push(Box::new(ReadMarkdownTool)),
            "list_dir" => out.push(Box::new(ListDirTool)),
            "write_file" => out.push(Box::new(WriteFileTool)),
            "edit_file" => out.push(Box::new(EditFileTool)),
            "delete_file" => out.push(Box::new(DeleteFileTool)),
            "search" => out.push(Box::new(SearchTool)),
            "run_cargo" => out.push(Box::new(RunCargoTool)),
            "declare_fix" => out.push(Box::new(DeclareFixTool)),
            "declare_hypothesis" => out.push(Box::new(DeclareHypothesisTool)),
            "log_bug" => out.push(Box::new(LogBugTool)),
            "resolve_bug" => out.push(Box::new(ResolveBugTool)),
            "record_run" => out.push(Box::new(RecordRunTool)),
            "api_search" => out.push(Box::new(ApiSearchTool)),
            "api_hover" => out.push(Box::new(ApiHoverTool)),
            "api_impls" => out.push(Box::new(ApiImplsTool)),
            "api_references" => out.push(Box::new(ApiReferencesTool)),
            "api_expand_macro" => out.push(Box::new(ApiExpandMacroTool)),
            "api_semantic_search" => {
                if let Some(svc) = retrieval.clone() {
                    out.push(Box::new(ApiSemanticSearchTool::new(svc)));
                }
            }
            "spec_semantic_search" => {
                if let Some(svc) = retrieval.clone() {
                    out.push(Box::new(SpecSemanticSearchTool::new(svc)));
                }
            }
            "signal_table_query" => {
                if let Some(svc) = retrieval.clone() {
                    out.push(Box::new(SignalTableQueryTool::new(svc)));
                }
            }
            "ask_user" => {
                if let Some(rt) = ask_user.clone() {
                    out.push(Box::new(AskUserTool::new(rt)));
                }
            }
            _ => {} // unknown tool name; skip
        }
    }
    out
}

/// Path-safety check shared by all tools. Rejects absolute paths,
/// `..` traversal, control chars, and Windows meta chars. The
/// behavior matches the artifact-write extractor in
/// `session::orchestrator::is_safe_relative_path` so write_file via
/// tool and write_file via fenced artifact agree on what is safe.
pub fn is_safe_relative_path(p: &str) -> bool {
    if p.is_empty() {
        return false;
    }
    if p.starts_with('/') || p.starts_with('\\') {
        return false;
    }
    if p.contains("..") {
        return false;
    }
    if p.contains(['<', '>', ':', '"', '|', '?', '*']) {
        return false;
    }
    if p.chars().any(|c| (c as u32) < 0x20) {
        return false;
    }
    true
}

/// Resolve a relative path under the project dir, rejecting any
/// path that fails `is_safe_relative_path`. Tools call this before
/// any filesystem op.
pub fn resolve_safe_path(project_dir: &Path, rel: &str) -> Result<PathBuf> {
    if !is_safe_relative_path(rel) {
        return Err(Error::Protocol(format!(
            "rejecting unsafe tool path: {rel}"
        )));
    }
    Ok(project_dir.join(rel))
}

/// Read-side resolver. Accepts either a bare project-relative path,
/// a `lib:<rel>` prefix (library root), or an `fw:<rel>` prefix
/// (framework assets). `fw:api/...` resolves under the normalized API-doc root;
/// other `fw:` paths resolve under the framework source root. Returns
/// `Ok(Some(abs))` when the input names a real path inside one of the
/// allowed roots, `Ok(None)` when a prefix is used but the corresponding
/// root is not configured, and `Err` when the path fails the safety
/// check.
pub fn resolve_read_path(ctx: &ToolContext, raw: &str) -> Result<Option<PathBuf>> {
    if let Some(rel) = raw.strip_prefix("lib:") {
        return resolve_under(ctx.library_root, "lib", rel);
    }
    if let Some(rel) = raw.strip_prefix("fw:") {
        if rel == "api" {
            return resolve_under(ctx.framework_docs_root, "fw", "");
        }
        if let Some(api_rel) = rel.strip_prefix("api/") {
            return resolve_under(ctx.framework_docs_root, "fw", api_rel);
        }
        if rel.is_empty() {
            if let Some(root) = ctx.framework_root {
                return Ok(Some(root.to_path_buf()));
            }
            return Ok(ctx.framework_docs_root.map(Path::to_path_buf));
        }
        return resolve_under(ctx.framework_root, "fw", rel);
    }
    Ok(Some(resolve_safe_path(ctx.project_dir, raw)?))
}

fn resolve_under(root: Option<&Path>, prefix: &str, rel: &str) -> Result<Option<PathBuf>> {
    let Some(root) = root else {
        return Ok(None);
    };
    if rel.is_empty() {
        return Ok(Some(root.to_path_buf()));
    }
    if !is_safe_relative_path(rel) {
        return Err(Error::Protocol(format!(
            "rejecting unsafe tool path: {prefix}:{rel}"
        )));
    }
    Ok(Some(root.join(rel)))
}

/// Parse a fenced tool-call block (`tool:<name>` info-string) into
/// (name, raw_body). The body is whatever lives between the open
/// and close fences; per-tool argument parsers live alongside their
/// `Tool` impl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedToolCall {
    pub name: String,
    pub body: String,
}

/// Scan `response_text` for fenced `tool:<name>` blocks. The
/// orchestrator runs this alongside artifact extraction; the two
/// kinds of fences are mutually exclusive (an info-string is either
/// a relative file path with a dot, or a `tool:<name>` directive).
///
/// Lenient `tool:write_file` recovery: some agents emit the
/// function-call shape with a path-only body and put the content in
/// a SEPARATE adjacent language fence (```rust, ```text, etc.):
///
/// ````text
/// ```tool:write_file
/// src/model/mod.rs
/// ```
///
/// ```rust
/// pub mod foo;
/// ```
/// ````
///
/// The strict reading rejects the first fence (no content) and
/// silently drops the second (info-string isn't a path), so the
/// agent's write goes nowhere. We detect the pattern and merge:
/// when a `tool:write_file` body is a single non-empty line that
/// resembles a project-relative path AND the next fenced block has
/// an unrecognized (non-`tool:`, non-path) info-string, treat the
/// next block's body as the file content and emit a single
/// JSON-shaped tool call.
pub fn extract_tool_calls(response_text: &str) -> Vec<ParsedToolCall> {
    // Tokenize the response into fenced blocks first, then walk
    // them so we can look at adjacent pairs.
    let blocks = collect_fenced_blocks(response_text);
    let mut out: Vec<ParsedToolCall> = Vec::new();
    let mut i = 0;
    while i < blocks.len() {
        let b = &blocks[i];
        if let Some(call) = parse_json_tool_block(b) {
            out.push(call);
            i += 1;
            continue;
        }
        let Some(name) = b.info.strip_prefix("tool:") else {
            i += 1;
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            i += 1;
            continue;
        }
        // Lenient write_file path-only-body merge.
        if name == "write_file" && body_is_just_path(&b.body) && i + 1 < blocks.len() {
            let next = &blocks[i + 1];
            let next_info = next.info.trim();
            let next_is_tool = next_info.starts_with("tool:");
            let next_is_path = !next_info.is_empty() && next_info.contains('.');
            if !next_is_tool && !next_is_path {
                let path = b.body.trim().to_string();
                let content = next.body.clone();
                let json = serde_json::json!({ "path": path, "content": content });
                out.push(ParsedToolCall {
                    name: name.to_string(),
                    body: json.to_string(),
                });
                i += 2;
                continue;
            }
        }
        out.push(ParsedToolCall {
            name: name.to_string(),
            body: b.body.clone(),
        });
        i += 1;
    }
    out
}

fn parse_json_tool_block(block: &FencedBlock) -> Option<ParsedToolCall> {
    if block.info.trim() != "json" {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(block.body.trim()).ok()?;
    let obj = value.as_object()?;
    let name = obj.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let args = obj
        .get("arguments")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let body = match args {
        serde_json::Value::String(s) => s,
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    };
    Some(ParsedToolCall {
        name: name.to_string(),
        body,
    })
}

struct FencedBlock {
    info: String,
    body: String,
}

/// Walk every ```...``` fence in `text`, returning each in order
/// with its info-string and body.
fn collect_fenced_blocks(text: &str) -> Vec<FencedBlock> {
    let mut out: Vec<FencedBlock> = Vec::new();
    let mut in_block: Option<(String, Vec<String>)> = None;
    for line in text.split('\n') {
        if let Some((_, body)) = in_block.as_mut() {
            if line.trim_start().starts_with("```") && line.trim().len() == 3 {
                let (info, lines) = in_block.take().expect("guarded by Some");
                out.push(FencedBlock {
                    info,
                    body: lines.join("\n"),
                });
                continue;
            }
            body.push(line.to_string());
        } else if let Some(rest) = line.strip_prefix("```") {
            let info = rest.trim().to_string();
            if !info.is_empty() {
                in_block = Some((info, Vec::new()));
            }
        }
    }
    out
}

/// True when `body` is a single non-empty line that looks like a
/// project-relative file path (contains a `.`, no whitespace,
/// doesn't start with `{`). Used by the lenient write_file merge.
fn body_is_just_path(body: &str) -> bool {
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() != 1 {
        return false;
    }
    let line = lines[0].trim();
    !line.is_empty()
        && !line.starts_with('{')
        && line.contains('.')
        && !line.chars().any(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_a_single_tool_call() {
        let body = "We need to read a file.\n\n```tool:read_file\nsrc/lib.rs\n```\n";
        let calls = extract_tool_calls(body);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].body, "src/lib.rs");
    }

    #[test]
    fn ignores_non_tool_fences() {
        let body = "```rust\nfn main() {}\n```\n```spec.md\n# Spec\n```\n";
        assert!(extract_tool_calls(body).is_empty());
    }

    #[test]
    fn parses_multiple_tool_calls_in_one_response() {
        let body = "```tool:list_dir\nsrc/\n```\n\n```tool:read_file\nsrc/lib.rs\n```\n";
        let calls = extract_tool_calls(body);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "list_dir");
        assert_eq!(calls[1].name, "read_file");
    }

    #[test]
    fn parses_json_fenced_tool_calls() {
        let body =
            "```json\n{\"name\":\"read_file\",\"arguments\":{\"path\":\"docs/spec.md\"}}\n```\n";
        let calls = extract_tool_calls(body);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].body, "{\"path\":\"docs/spec.md\"}");
    }

    #[test]
    fn safe_path_helper_rejects_traversal() {
        assert!(is_safe_relative_path("src/lib.rs"));
        assert!(!is_safe_relative_path("/etc/passwd"));
        assert!(!is_safe_relative_path("../escape.md"));
        assert!(!is_safe_relative_path(""));
    }

    #[test]
    fn dispatcher_filters_unknown_tools() {
        let tools = build_dispatcher(&["read_file", "bogus", "search"]);
        let names: Vec<_> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, vec!["read_file", "search"]);
    }

    #[test]
    fn dispatcher_includes_every_known_tool() {
        // Mirror the full match arm in build_dispatcher so every
        // production tool name resolves to its struct.
        let names = [
            "read_file",
            "list_dir",
            "write_file",
            "edit_file",
            "delete_file",
            "search",
            "run_cargo",
            "declare_fix",
            "declare_hypothesis",
            "log_bug",
            "resolve_bug",
            "record_run",
            "api_search",
            "api_hover",
            "api_impls",
            "api_references",
            "api_expand_macro",
        ];
        let tools = build_dispatcher(&names);
        let got: Vec<_> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(got, names);
    }

    #[test]
    fn dispatcher_with_runtime_includes_phase5_tools() {
        use crate::__internal::session::ask_user::AskUserRuntime;
        use crate::__internal::session::embedder::{EmbedError, EmbeddingClient};
        use crate::__internal::session::retrieval::RetrievalService;
        use async_trait::async_trait;
        use std::sync::Arc;

        struct MockEmbedder;

        #[async_trait]
        impl EmbeddingClient for MockEmbedder {
            fn provider(&self) -> &str {
                "mock"
            }
            fn model_id(&self) -> &str {
                "mock-embed"
            }
            fn dimension(&self) -> usize {
                8
            }
            async fn embed(
                &self,
                texts: &[&str],
            ) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
                Ok(texts.iter().map(|_| vec![0.0; 8]).collect())
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder);
        let service = Arc::new(RetrievalService::new(tmp.path(), embedder).unwrap());
        let runtime = Arc::new(AskUserRuntime::new(
            tmp.path().to_path_buf(),
            "DM0".to_string(),
        ));
        let names = [
            "api_semantic_search",
            "spec_semantic_search",
            "signal_table_query",
            "ask_user",
        ];
        let tools = build_dispatcher_with_runtime(&names, Some(service), Some(runtime));
        let got: Vec<_> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(got, names);
    }

    #[test]
    fn phase5_tools_advertise_schemas_conforming_to_spec() {
        // Architecture Chapter 4 §§4.2-4.5 specifies the JSON-schema
        // shape for each tool's args. The advertise-shape (built from
        // `args_schema()` by the agent layer at session start) feeds
        // straight into the LLM provider's tool-call API. This test
        // asserts each schema's required fields, top-level shape,
        // and enum values match the spec.
        use crate::__internal::session::ask_user::AskUserRuntime;
        use crate::__internal::session::embedder::{EmbedError, EmbeddingClient};
        use crate::__internal::session::retrieval::RetrievalService;
        use async_trait::async_trait;
        use std::sync::Arc;

        struct MockEmbedder;

        #[async_trait]
        impl EmbeddingClient for MockEmbedder {
            fn provider(&self) -> &str {
                "mock"
            }
            fn model_id(&self) -> &str {
                "mock-embed"
            }
            fn dimension(&self) -> usize {
                8
            }
            async fn embed(
                &self,
                texts: &[&str],
            ) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
                Ok(texts.iter().map(|_| vec![0.0; 8]).collect())
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder);
        let service = Arc::new(RetrievalService::new(tmp.path(), embedder).unwrap());
        let runtime = Arc::new(AskUserRuntime::new(
            tmp.path().to_path_buf(),
            "DM0".to_string(),
        ));

        let api_tool = ApiSemanticSearchTool::new(service.clone());
        let api_schema = api_tool.args_schema();
        assert_eq!(api_schema["type"], "object");
        assert!(
            api_schema["required"]
                .as_array()
                .map(|a| a.iter().any(|v| v == "query"))
                .unwrap_or(false),
            "api_semantic_search required = [\"query\"]"
        );
        // `k` has min/max from §4.2.
        assert_eq!(api_schema["properties"]["k"]["minimum"], 1);
        assert_eq!(api_schema["properties"]["k"]["maximum"], 20);
        // `kind` enum from §4.2.
        let api_kinds = api_schema["properties"]["kind"]["enum"]
            .as_array()
            .expect("api_semantic_search.kind.enum");
        for required in [
            "api-page",
            "src-fn",
            "src-impl",
            "src-trait",
            "src-mod-doc",
            "src-other",
        ] {
            assert!(
                api_kinds.iter().any(|v| v == required),
                "{required} missing from api_semantic_search.kind"
            );
        }

        let spec_tool = SpecSemanticSearchTool::new(service.clone());
        let spec_schema = spec_tool.args_schema();
        assert_eq!(spec_schema["type"], "object");
        let spec_kinds = spec_schema["properties"]["kind"]["enum"]
            .as_array()
            .expect("spec_semantic_search.kind.enum");
        for required in ["prose", "table", "stub", "mixed"] {
            assert!(
                spec_kinds.iter().any(|v| v == required),
                "{required} missing from spec_semantic_search.kind"
            );
        }
        assert!(spec_schema["properties"]["source"].is_object());

        let sig_tool = SignalTableQueryTool::new(service);
        let sig_schema = sig_tool.args_schema();
        assert_eq!(sig_schema["type"], "object");
        assert!(
            sig_schema["required"]
                .as_array()
                .map(|a| a.iter().any(|v| v == "filter"))
                .unwrap_or(false),
            "signal_table_query required = [\"filter\"]"
        );
        // additionalProperties: false guards filter shape per §4.4.
        assert_eq!(
            sig_schema["properties"]["filter"]["additionalProperties"],
            false
        );

        let ask_tool = AskUserTool::new(runtime);
        let ask_schema = ask_tool.args_schema();
        assert_eq!(ask_schema["type"], "object");
        assert!(
            ask_schema["required"]
                .as_array()
                .map(|a| a.iter().any(|v| v == "question"))
                .unwrap_or(false),
            "ask_user required = [\"question\"]"
        );
        let ask_kinds = ask_schema["properties"]["kind"]["enum"]
            .as_array()
            .expect("ask_user.kind.enum");
        for required in ["free-form", "yes-no", "choice", "value"] {
            assert!(
                ask_kinds.iter().any(|v| v == required),
                "{required} missing from ask_user.kind"
            );
        }
        let record_as_kinds = ask_schema["properties"]["record_as"]["enum"]
            .as_array()
            .expect("ask_user.record_as.enum");
        for required in ["open-question", "auto-decision", "none"] {
            assert!(
                record_as_kinds.iter().any(|v| v == required),
                "{required} missing from ask_user.record_as"
            );
        }
        // thread_id must be optional (i.e. NOT in required).
        let ask_required = ask_schema["required"].as_array().unwrap();
        assert!(
            !ask_required.iter().any(|v| v == "thread_id"),
            "thread_id must be optional"
        );
    }

    #[test]
    fn universal_tools_lists_phase5_tools() {
        let names: Vec<&str> = crate::__internal::steps::UNIVERSAL_TOOLS.to_vec();
        for required in [
            "api_semantic_search",
            "spec_semantic_search",
            "signal_table_query",
            "ask_user",
        ] {
            assert!(
                names.contains(&required),
                "{required} missing from UNIVERSAL_TOOLS: {names:?}"
            );
        }
    }

    #[test]
    fn image_mime_from_path_covers_known_extensions_and_falls_back() {
        use std::path::Path;
        assert_eq!(image_mime_from_path(Path::new("a.png")), Some("image/png"));
        assert_eq!(image_mime_from_path(Path::new("a.jpg")), Some("image/jpeg"));
        assert_eq!(
            image_mime_from_path(Path::new("a.jpeg")),
            Some("image/jpeg")
        );
        assert_eq!(image_mime_from_path(Path::new("a.gif")), Some("image/gif"));
        assert_eq!(
            image_mime_from_path(Path::new("a.webp")),
            Some("image/webp")
        );
        // Case-insensitive on extension.
        assert_eq!(image_mime_from_path(Path::new("a.PNG")), Some("image/png"));
        // Non-image -> None.
        assert_eq!(image_mime_from_path(Path::new("a.txt")), None);
        assert_eq!(image_mime_from_path(Path::new("noext")), None);
    }

    #[test]
    fn is_safe_relative_path_rejects_meta_chars_and_control_bytes() {
        // Windows meta chars.
        for bad in ["a<b", "a>b", "a:b", "a\"b", "a|b", "a?b", "a*b"] {
            assert!(!is_safe_relative_path(bad), "{bad}");
        }
        // Control byte.
        assert!(!is_safe_relative_path("a\x01b"));
        // Backslash absolute path (Windows-style).
        assert!(!is_safe_relative_path("\\bad"));
    }

    #[test]
    fn resolve_safe_path_rejects_unsafe_paths_but_returns_path_for_safe_ones() {
        let p = std::path::Path::new("/tmp/proj");
        assert_eq!(
            resolve_safe_path(p, "src/lib.rs").unwrap(),
            p.join("src/lib.rs")
        );
        assert!(resolve_safe_path(p, "../escape.rs").is_err());
        assert!(resolve_safe_path(p, "/etc/passwd").is_err());
    }

    #[test]
    fn resolve_read_path_routes_lib_and_fw_prefixes_under_their_roots() {
        let project = std::path::Path::new("/tmp/proj");
        let lib_root = std::path::Path::new("/tmp/lib");
        let fw_root = std::path::Path::new("/tmp/fw");
        let docs_root = std::path::Path::new("/tmp/docs");
        let ctx = ToolContext::new(project, Some(lib_root), Some(fw_root), Some(docs_root));
        assert_eq!(
            resolve_read_path(&ctx, "lib:foo.md").unwrap(),
            Some(lib_root.join("foo.md")),
        );
        assert_eq!(
            resolve_read_path(&ctx, "fw:src/lib.rs").unwrap(),
            Some(fw_root.join("src/lib.rs")),
        );
        assert_eq!(
            resolve_read_path(&ctx, "fw:api/toc.md").unwrap(),
            Some(docs_root.join("toc.md")),
        );
        // Bare path -> under project_dir.
        assert_eq!(
            resolve_read_path(&ctx, "docs/spec.md").unwrap(),
            Some(project.join("docs/spec.md")),
        );
    }

    #[test]
    fn resolve_read_path_returns_none_when_prefix_root_is_unconfigured() {
        let project = std::path::Path::new("/tmp/proj");
        let ctx = ToolContext::new(project, None, None, None);
        // lib: with no library_root => Ok(None).
        assert_eq!(resolve_read_path(&ctx, "lib:foo").unwrap(), None);
        // fw:api with no docs root => Ok(None).
        assert_eq!(resolve_read_path(&ctx, "fw:api/toc.md").unwrap(), None);
    }

    #[test]
    fn preview_one_line_quotes_and_escapes_special_chars() {
        // Always wrapped in JSON-style quotes.
        assert_eq!(preview_one_line("hello", 10), "\"hello\"");
        // Newline / tab / backslash / quote are escaped.
        assert_eq!(preview_one_line("a\nb", 10), "\"a\\nb\"");
        assert_eq!(preview_one_line("a\tb", 10), "\"a\\tb\"");
        assert_eq!(preview_one_line("a\\b", 10), "\"a\\\\b\"");
        assert_eq!(preview_one_line("a\"b", 10), "\"a\\\"b\"");
        // Other control bytes hex-escape.
        assert_eq!(preview_one_line("a\x01b", 10), "\"a\\x01b\"");
        // Past max_chars: trailing `...` marker INSIDE the closing quote.
        let out = preview_one_line("abcdefghij", 5);
        assert!(out.ends_with("...\""), "{out}");
        assert!(out.starts_with("\""), "{out}");
    }
}
