//! Step-prompt loader with scope-aware resolution.
//!
//! Each step's prompt file (`<slug>.md` for work, `<slug>-critique.md`
//! for critique) can be overridden in two scopes:
//!
//! 1. **Project**: `<project>/.sim-flow/prompts/<file>.md`. Scoped to one
//!    sim-flow project; ideal for tweaks specific to a particular model.
//! 2. **Global**: the OS-aware user config dir, resolved via the
//!    `directories` crate as `ProjectDirs::from("", "", "sim-flow")`:
//!    - macOS: `~/Library/Application Support/sim-flow/prompts/`
//!    - Linux: `~/.config/sim-flow/prompts/`
//!    - Windows: `%APPDATA%/sim-flow/prompts/`
//!      Applies to every project on this machine that doesn't have a
//!      project-scope override.
//! 3. **Default** (fallback): the version shipped in
//!    `<foundation>/tools/sim-flow/prompts/<file>.md`.
//!
//! `load_scoped` returns the active content + which scope provided it
//! so the dashboard can show the user where each prompt is coming from.
//! `save_override` / `delete_override` manage the two override scopes;
//! the default under `<foundation>/tools/sim-flow/prompts/` is
//! read-only here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use minijinja::{Environment, UndefinedBehavior};

use crate::client::SessionKind;
use crate::{Error, Result};

/// Default-scope prompts directory, relative to the foundation
/// (workspace) root. Sim-flow's tooling assets live colocated with
/// the crate at `tools/sim-flow/<asset>/`; the older top-level
/// `instructions/` location was moved here so all sim-flow-specific
/// trees (prompts, templates, extensions) sit under one umbrella.
pub const PROMPTS_DIR: &str = "tools/sim-flow/prompts";
/// Subdirectory under `<project>/.sim-flow/` (project scope) and
/// under the OS user-config dir (global scope) where prompt
/// overrides live. Same name in both contexts.
pub const PROMPTS_SUBDIR: &str = "prompts";
/// Subdirectory under `PROMPTS_DIR` for the orchestrator's "system
/// boilerplate" prompts (artifact-write convention, native-tools
/// convention, auto-mode notes). Underscore-prefixed so the
/// dashboard's per-step prompt list ignores them -- they're shared
/// session-wide rather than per-step, and not user-overridable.
pub const CONVENTIONS_SUBDIR: &str = "_conventions";
/// Subdirectory under `PROMPTS_DIR` for reusable template fragments
/// (`{{output_intro}}` etc.) substituted into per-step prompts at
/// load time. Underscore-prefixed for the same reason as
/// `_conventions/`: it's not a per-step prompt list entry.
pub const TEMPLATES_SUBDIR: &str = "_templates";

/// Where a resolved prompt's content came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptScope {
    Project,
    Global,
    Default,
}

impl PromptScope {
    pub fn as_str(self) -> &'static str {
        match self {
            PromptScope::Project => "project",
            PromptScope::Global => "global",
            PromptScope::Default => "default",
        }
    }
}

/// Resolved instruction prompt: content + which scope produced it.
#[derive(Debug, Clone)]
pub struct ResolvedPrompt {
    pub content: String,
    pub scope: PromptScope,
    pub path: PathBuf,
}

fn file_name_for(slug: &str, kind: SessionKind) -> String {
    let suffix = match kind {
        SessionKind::Work => "",
        SessionKind::Critique => "-critique",
    };
    format!("{slug}{suffix}.md")
}

/// Default-scope path: the version shipped in `<foundation>/instructions/`.
pub fn instruction_path(foundation_root: &Path, step_slug: &str, kind: SessionKind) -> PathBuf {
    foundation_root
        .join(PROMPTS_DIR)
        .join(file_name_for(step_slug, kind))
}

/// Project-scope override path. Always returns a path -- the file may
/// or may not exist on disk.
pub fn project_override_path(project_dir: &Path, step_slug: &str, kind: SessionKind) -> PathBuf {
    project_dir
        .join(".sim-flow")
        .join(PROMPTS_SUBDIR)
        .join(file_name_for(step_slug, kind))
}

/// Global-scope override directory. `Some` only when the platform
/// resolves a config directory; `None` on bizarre setups (e.g. CI
/// with `HOME` unset).
pub fn global_prompts_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "sim-flow").map(|d| d.config_dir().join(PROMPTS_SUBDIR))
}

/// Global-scope override path. Returns `None` if the global config
/// directory cannot be resolved on this platform.
pub fn global_override_path(step_slug: &str, kind: SessionKind) -> Option<PathBuf> {
    global_prompts_dir().map(|d| d.join(file_name_for(step_slug, kind)))
}

/// Absolute path to a "convention" prompt (`_conventions/<name>.md`)
/// under the foundation default tree. These hold session-wide
/// boilerplate that's identical across steps -- the artifact-write
/// convention, the native-tools convention, the automated-mode
/// notes -- and can either be loaded into a system message (JSONL
/// host) or referenced by absolute path in a "go read this" bootstrap
/// directive (PTY agents that have a Read tool of their own).
pub fn convention_path(foundation_root: &Path, name: &str) -> PathBuf {
    foundation_root
        .join(PROMPTS_DIR)
        .join(CONVENTIONS_SUBDIR)
        .join(format!("{name}.md"))
}

/// Read a convention prompt by name (e.g. `"native-tools"`,
/// `"fenced-blocks"`, `"auto-mode"`). Errors when the file is
/// missing -- conventions are required for every session, so a
/// missing file is a packaging bug worth surfacing loudly rather
/// than silently degrading.
pub fn load_convention(foundation_root: &Path, name: &str) -> Result<String> {
    let path = convention_path(foundation_root, name);
    std::fs::read_to_string(&path).map_err(|source| Error::Io { path, source })
}

/// Absolute path to a template fragment (`_templates/<name>.md`)
/// under the foundation default tree. These are short, reusable
/// chunks substituted into per-step prompts via `{{name}}`
/// placeholders. The orchestrator picks a per-mode fragment (e.g.
/// `output-intro-fenced.md` vs `output-intro-native.md`) at load
/// time based on the active artifact-write mode.
pub fn template_path(foundation_root: &Path, name: &str) -> PathBuf {
    foundation_root
        .join(PROMPTS_DIR)
        .join(TEMPLATES_SUBDIR)
        .join(format!("{name}.md"))
}

/// Read a template fragment by name. Errors loudly when missing --
/// fragments referenced by `{{key}}` placeholders MUST exist or
/// the prompt is structurally broken.
pub fn load_template(foundation_root: &Path, name: &str) -> Result<String> {
    let path = template_path(foundation_root, name);
    std::fs::read_to_string(&path).map_err(|source| Error::Io { path, source })
}

/// Substitution context: a map of `{{key}}` -> replacement value.
/// `BTreeMap` for stable ordering in diagnostics; values are owned
/// strings since the typical use case loads fragment bodies from
/// disk and hands ownership in.
pub type PromptContext = BTreeMap<String, String>;

/// Render a prompt body through the minijinja templating engine.
/// Strict undefined-variable handling: an unbound `{{key}}` (typo,
/// missing context entry, etc.) returns `Err`. The strict mode is
/// intentional -- silent omission of an `{{output_intro}}` block
/// would leave the prompt without any artifact-write directive,
/// and the model would have to guess.
///
/// `name` is used in error diagnostics only; pass the prompt's
/// filename or slug.
pub fn render_prompt(name: &str, body: &str, context: &PromptContext) -> Result<String> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    // Disable HTML auto-escaping; we're emitting plain markdown, not
    // HTML, and the artifact-write paths / JSON examples contain
    // characters (`<`, `&`, etc.) that would otherwise get mangled
    // into `&lt;` / `&amp;`.
    env.set_auto_escape_callback(|_| minijinja::AutoEscape::None);
    let tmpl = env
        .template_from_str(body)
        .map_err(|e| Error::State(format!("prompt template `{name}`: parse error: {e}")))?;
    tmpl.render(context).map_err(|e| {
        Error::State(format!(
            "prompt template `{name}`: render error: {e}. \
             Defined keys: [{}]",
            context.keys().cloned().collect::<Vec<_>>().join(", ")
        ))
    })
}

/// Resolve the instruction prompt for `(slug, kind)`. Checks project
/// scope first, then global, then the foundation default. Returns
/// [`Error::InstructionMissing`] only when the default itself is
/// missing.
pub fn load_scoped(
    foundation_root: &Path,
    project_dir: &Path,
    step_slug: &str,
    kind: SessionKind,
) -> Result<ResolvedPrompt> {
    let project_path = project_override_path(project_dir, step_slug, kind);
    if project_path.is_file() {
        let content = std::fs::read_to_string(&project_path).map_err(|source| Error::Io {
            path: project_path.clone(),
            source,
        })?;
        return Ok(ResolvedPrompt {
            content,
            scope: PromptScope::Project,
            path: project_path,
        });
    }
    if let Some(global_path) = global_override_path(step_slug, kind)
        && global_path.is_file()
    {
        let content = std::fs::read_to_string(&global_path).map_err(|source| Error::Io {
            path: global_path.clone(),
            source,
        })?;
        return Ok(ResolvedPrompt {
            content,
            scope: PromptScope::Global,
            path: global_path,
        });
    }
    let default_path = instruction_path(foundation_root, step_slug, kind);
    if !default_path.exists() {
        return Err(Error::InstructionMissing(default_path));
    }
    let content = std::fs::read_to_string(&default_path).map_err(|source| Error::Io {
        path: default_path.clone(),
        source,
    })?;
    Ok(ResolvedPrompt {
        content,
        scope: PromptScope::Default,
        path: default_path,
    })
}

/// Backwards-compatible wrapper: returns just the content. New code
/// should prefer `load_scoped` so the source-of-truth scope is visible.
pub fn load(foundation_root: &Path, step_slug: &str, kind: SessionKind) -> Result<String> {
    load_scoped(foundation_root, &PathBuf::new(), step_slug, kind).map(|r| r.content)
}

/// Variant used by the orchestrator and other callers that have a
/// project directory in hand.
pub fn load_for_project(
    foundation_root: &Path,
    project_dir: &Path,
    step_slug: &str,
    kind: SessionKind,
) -> Result<String> {
    load_scoped(foundation_root, project_dir, step_slug, kind).map(|r| r.content)
}

/// Like `load_for_project` but renders the loaded body through the
/// templating engine with the supplied context, so `{{key}}`
/// placeholders get substituted before the body is handed to the
/// LLM. A prompt that contains no placeholders is returned
/// unchanged.
pub fn load_for_project_with_context(
    foundation_root: &Path,
    project_dir: &Path,
    step_slug: &str,
    kind: SessionKind,
    context: &PromptContext,
) -> Result<String> {
    let resolved = load_scoped(foundation_root, project_dir, step_slug, kind)?;
    let name = format!(
        "{step_slug}{}",
        match kind {
            SessionKind::Work => "",
            SessionKind::Critique => "-critique",
        }
    );
    render_prompt(&name, &resolved.content, context)
}

/// Persist an override at the given scope. Creates the prompts
/// directory if necessary. Returns the path written.
pub fn save_override(
    scope: PromptScope,
    project_dir: &Path,
    step_slug: &str,
    kind: SessionKind,
    content: &str,
) -> Result<PathBuf> {
    let path = match scope {
        PromptScope::Project => project_override_path(project_dir, step_slug, kind),
        PromptScope::Global => global_override_path(step_slug, kind).ok_or_else(|| {
            Error::State(
                "instructions: cannot resolve a global config directory on this platform".into(),
            )
        })?,
        PromptScope::Default => {
            return Err(Error::State(
                "instructions: refusing to overwrite the foundation default; \
                 edit the source repo or save to project / global scope instead"
                    .into(),
            ));
        }
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(&path, content).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Remove an override at the given scope. Idempotent: returns
/// `Ok(false)` if the file was already absent. The default scope
/// rejects this operation.
pub fn delete_override(
    scope: PromptScope,
    project_dir: &Path,
    step_slug: &str,
    kind: SessionKind,
) -> Result<bool> {
    let path = match scope {
        PromptScope::Project => project_override_path(project_dir, step_slug, kind),
        PromptScope::Global => match global_override_path(step_slug, kind) {
            Some(p) => p,
            None => return Ok(false),
        },
        PromptScope::Default => {
            return Err(Error::State(
                "instructions: cannot delete the foundation default".into(),
            ));
        }
    };
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    Ok(true)
}

/// Listing entry for one (slug, kind) pair.
#[derive(Debug, Clone)]
pub struct PromptEntry {
    pub slug: String,
    pub kind: SessionKind,
    pub active_scope: PromptScope,
    pub project_path: PathBuf,
    pub project_present: bool,
    pub global_path: Option<PathBuf>,
    pub global_present: bool,
    pub default_path: PathBuf,
}

/// Enumerate every prompt the foundation ships, plus the active scope
/// and the per-scope existence flags so the dashboard can show the
/// user where each prompt's content is coming from.
pub fn list_prompts(foundation_root: &Path, project_dir: &Path) -> Result<Vec<PromptEntry>> {
    let dir = foundation_root.join(PROMPTS_DIR);
    let entries = std::fs::read_dir(&dir).map_err(|source| Error::Io {
        path: dir.clone(),
        source,
    })?;
    let mut out: Vec<PromptEntry> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(stem) = file_name.strip_suffix(".md") else {
            continue;
        };
        let (slug, kind) = if let Some(s) = stem.strip_suffix("-critique") {
            (s.to_string(), SessionKind::Critique)
        } else {
            (stem.to_string(), SessionKind::Work)
        };
        out.push(prompt_entry(foundation_root, project_dir, &slug, kind)?);
    }
    out.sort_by(|a, b| {
        a.slug
            .cmp(&b.slug)
            .then_with(|| (a.kind as u8).cmp(&(b.kind as u8)))
    });
    Ok(out)
}

fn prompt_entry(
    foundation_root: &Path,
    project_dir: &Path,
    slug: &str,
    kind: SessionKind,
) -> Result<PromptEntry> {
    let project_path = project_override_path(project_dir, slug, kind);
    let project_present = project_path.is_file();
    let global_path = global_override_path(slug, kind);
    let global_present = global_path.as_ref().map(|p| p.is_file()).unwrap_or(false);
    let default_path = instruction_path(foundation_root, slug, kind);
    let active_scope = if project_present {
        PromptScope::Project
    } else if global_present {
        PromptScope::Global
    } else {
        PromptScope::Default
    };
    Ok(PromptEntry {
        slug: slug.to_string(),
        kind,
        active_scope,
        project_path,
        project_present,
        global_path,
        global_present,
        default_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolves_work_and_critique_paths() {
        let work = instruction_path(Path::new("/foo"), "dm0-specification", SessionKind::Work);
        assert!(work.ends_with("tools/sim-flow/prompts/dm0-specification.md"));
        let crit = instruction_path(
            Path::new("/foo"),
            "dm0-specification",
            SessionKind::Critique,
        );
        assert!(crit.ends_with("tools/sim-flow/prompts/dm0-specification-critique.md"));
    }

    #[test]
    fn loads_default_when_no_overrides_present() {
        let root = tempdir().unwrap();
        let dir = root.path().join(PROMPTS_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("smoke.md"), "default body").unwrap();
        let project = tempdir().unwrap();
        let resolved =
            load_scoped(root.path(), project.path(), "smoke", SessionKind::Work).unwrap();
        assert_eq!(resolved.content, "default body");
        assert_eq!(resolved.scope, PromptScope::Default);
    }

    #[test]
    fn project_override_wins_over_default() {
        let foundation = tempdir().unwrap();
        std::fs::create_dir_all(foundation.path().join(PROMPTS_DIR)).unwrap();
        std::fs::write(
            foundation.path().join(PROMPTS_DIR).join("smoke.md"),
            "default body",
        )
        .unwrap();
        let project = tempdir().unwrap();
        save_override(
            PromptScope::Project,
            project.path(),
            "smoke",
            SessionKind::Work,
            "project body",
        )
        .unwrap();
        let resolved = load_scoped(
            foundation.path(),
            project.path(),
            "smoke",
            SessionKind::Work,
        )
        .unwrap();
        assert_eq!(resolved.content, "project body");
        assert_eq!(resolved.scope, PromptScope::Project);
    }

    #[test]
    fn delete_override_returns_false_when_absent() {
        let project = tempdir().unwrap();
        let removed = delete_override(
            PromptScope::Project,
            project.path(),
            "smoke",
            SessionKind::Work,
        )
        .unwrap();
        assert!(!removed);
    }

    #[test]
    fn missing_default_errors() {
        let foundation = tempdir().unwrap();
        let project = tempdir().unwrap();
        let err =
            load_scoped(foundation.path(), project.path(), "nope", SessionKind::Work).unwrap_err();
        assert!(matches!(err, Error::InstructionMissing(_)));
    }

    #[test]
    fn save_override_rejects_default_scope() {
        let project = tempdir().unwrap();
        let err = save_override(
            PromptScope::Default,
            project.path(),
            "smoke",
            SessionKind::Work,
            "body",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("foundation default"));
    }

    #[test]
    fn list_prompts_walks_instruction_dir() {
        let foundation = tempdir().unwrap();
        let dir = foundation.path().join(PROMPTS_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dm0-specification.md"), "work").unwrap();
        std::fs::write(dir.join("dm0-specification-critique.md"), "crit").unwrap();
        std::fs::write(dir.join("dm1-modeling-setup.md"), "work").unwrap();
        let project = tempdir().unwrap();
        let entries = list_prompts(foundation.path(), project.path()).unwrap();
        let labels: Vec<(String, SessionKind)> =
            entries.iter().map(|e| (e.slug.clone(), e.kind)).collect();
        assert!(labels.contains(&("dm0-specification".into(), SessionKind::Work)));
        assert!(labels.contains(&("dm0-specification".into(), SessionKind::Critique)));
        assert!(labels.contains(&("dm1-modeling-setup".into(), SessionKind::Work)));
        assert!(
            entries
                .iter()
                .all(|e| e.active_scope == PromptScope::Default)
        );
    }

    #[test]
    fn render_prompt_substitutes_known_keys() {
        let mut ctx = PromptContext::new();
        ctx.insert("output_intro".into(), "WRITE A FILE".into());
        let body = "## Output\n\n{{ output_intro }}\n\nThe path is `docs/spec.md`.";
        let out = render_prompt("dm0-specification", body, &ctx).unwrap();
        assert!(out.contains("WRITE A FILE"), "out: {out}");
        assert!(out.contains("The path is `docs/spec.md`."), "out: {out}");
        assert!(!out.contains("{{"), "placeholder leaked: {out}");
    }

    #[test]
    fn render_prompt_passes_through_when_no_placeholders() {
        // Existing prompts that don't reference `{{...}}` must
        // render to the same body verbatim. This is the migration
        // safety net -- adding the renderer to the load path must
        // not perturb prompts that haven't been retemplated yet.
        let ctx = PromptContext::new();
        let body = "## Goal\n\nNothing to substitute here.";
        let out = render_prompt("any", body, &ctx).unwrap();
        assert_eq!(out, body);
    }

    #[test]
    fn render_prompt_errors_loudly_on_unknown_key() {
        // Strict-undefined: an unbound `{{key}}` must NOT silently
        // render as an empty string. Without this, a typo
        // (`{{output_intr}}`) would delete the directive entirely
        // and the model would get a prompt missing the artifact-
        // write instructions.
        let ctx = PromptContext::new();
        let body = "## Output\n\n{{output_intro}}";
        let err = render_prompt("dm0", body, &ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("dm0"), "msg: {msg}");
        assert!(
            msg.contains("output_intro") || msg.contains("undefined"),
            "msg: {msg}"
        );
    }

    #[test]
    fn render_prompt_preserves_code_chars_no_html_escape() {
        // JSON / code examples in prompts contain `<`, `&`, etc.
        // Auto-escape would mangle these into &lt; / &amp;.
        let mut ctx = PromptContext::new();
        ctx.insert("output_intro".into(), "x".into());
        let body = "{{output_intro}}: `{\"foo\": \"<bar>\"}` & `a < b`";
        let out = render_prompt("test", body, &ctx).unwrap();
        assert!(out.contains("<bar>"), "escaped: {out}");
        assert!(out.contains("a < b"), "escaped: {out}");
        assert!(!out.contains("&lt;"), "escaped: {out}");
        assert!(!out.contains("&amp;"), "escaped: {out}");
    }

    /// Walk every DM*.md prompt under the foundation's default
    /// prompts dir, render it with both the fenced and native
    /// output-intro fragments, and verify both renders succeed.
    /// Catches templating drift (e.g. a placeholder added but not
    /// in the context, or a fragment file missing) at test time
    /// instead of mid-session.
    #[test]
    fn all_dm_prompts_render_in_both_modes() {
        // Walk up from the crate dir to find the foundation root.
        // The crate is at <foundation>/tools/sim-flow.
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let foundation_root = crate_dir
            .parent()
            .and_then(|p| p.parent())
            .expect("crate has two ancestors");
        let prompts_dir = foundation_root.join(PROMPTS_DIR);
        if !prompts_dir.is_dir() {
            // Tarball-without-prompts edge case; skip rather than
            // false-positive in CI.
            return;
        }
        let fenced = load_template(foundation_root, "output-intro-fenced")
            .expect("output-intro-fenced fragment must exist");
        let native = load_template(foundation_root, "output-intro-native")
            .expect("output-intro-native fragment must exist");
        let mut fenced_ctx = PromptContext::new();
        fenced_ctx.insert("output_intro".into(), fenced);
        let mut native_ctx = PromptContext::new();
        native_ctx.insert("output_intro".into(), native);

        let project_dir = PathBuf::new();
        let mut tested = 0;
        for entry in std::fs::read_dir(&prompts_dir).unwrap().flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            // Only walk DM-step prompts. The smoke/generate-verilog
            // prompts are exercised separately and don't carry an
            // output-intro placeholder yet.
            if !name.starts_with("dm") || !name.ends_with(".md") {
                continue;
            }
            let stem = name.trim_end_matches(".md");
            let (slug, kind) = if let Some(s) = stem.strip_suffix("-critique") {
                (s.to_string(), SessionKind::Critique)
            } else {
                (stem.to_string(), SessionKind::Work)
            };
            for ctx in [&fenced_ctx, &native_ctx] {
                load_for_project_with_context(foundation_root, &project_dir, &slug, kind, ctx)
                    .unwrap_or_else(|e| {
                        panic!("rendering `{name}` failed: {e}");
                    });
            }
            tested += 1;
        }
        // Sanity-check: we expect 14 work + 14 critique = 28 DM
        // prompts. A future split will need to update this floor.
        assert!(
            tested >= 28,
            "tested only {tested} DM prompts; expected 28+"
        );
    }
}
