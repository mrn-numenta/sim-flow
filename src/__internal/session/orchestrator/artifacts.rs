//! Fenced ` ```<path> ` artifact-write extraction + path detection.
//!
//! Owns the `ExtractedArtifact` type the turn loop consumes, the
//! fence parser that pulls artifact-write blocks out of the agent's
//! response, and the gatekeeping that decides whether a candidate
//! path is allowed to land on disk. The framework / library /
//! framework-docs root detectors live here too because they are the
//! filesystem-shape helpers the orchestrator uses to populate the
//! tool-context the agent sees.

use std::path::{Path, PathBuf};

use super::options::FRAMEWORK_DOCS_ROOT_ENV;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedArtifact {
    pub relative_path: String,
    pub content: String,
}

pub(super) fn extract_artifacts(response_text: &str) -> Vec<ExtractedArtifact> {
    use std::collections::HashMap;
    // Multi-line search for `^``` <path>\n...\n``` $`. We do this by
    // hand to avoid a regex with multiline + dotall combos that fight
    // the `regex` crate's defaults.
    let mut out: HashMap<String, String> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut lines = response_text.split('\n').enumerate().peekable();
    let mut in_block: Option<(String, Vec<String>)> = None;
    for (_idx, line) in &mut lines {
        let trimmed_line = line;
        if let Some((path, body)) = in_block.as_mut() {
            if trimmed_line.trim_start().starts_with("```") && trimmed_line.trim().len() == 3 {
                // Closing fence.
                let content = body.join("\n");
                if !out.contains_key(path) {
                    order.push(path.clone());
                }
                out.insert(path.clone(), content);
                in_block = None;
            } else {
                body.push(line.to_string());
            }
        } else if let Some(rest) = trimmed_line.strip_prefix("```") {
            // Opening fence: info-string follows. If it looks like a
            // path (has a `.` and no whitespace), treat as artifact.
            let info = rest.trim();
            if !info.is_empty() && info.contains('.') && is_safe_relative_path(info) {
                in_block = Some((info.to_string(), Vec::new()));
            }
            // else: a normal language fence (e.g. ```rust); ignore.
        }
    }

    order
        .into_iter()
        .map(|path| {
            let content = out.remove(&path).unwrap_or_default();
            ExtractedArtifact {
                relative_path: path,
                content: strip_trailing_newline(&content).to_string(),
            }
        })
        .collect()
}

fn strip_trailing_newline(s: &str) -> &str {
    s.strip_suffix('\n').unwrap_or(s)
}

pub(super) fn is_safe_relative_path(p: &str) -> bool {
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
    p.contains('.')
}

/// True when `p` lands inside `.sim-flow/` (the orchestrator's own
/// state tree). Agents must never write here -- not `state.toml` (a
/// past run had the agent "fix" its own gate status by editing it),
/// not `config.toml`, not the prompt overrides, not the control
/// socket. We enforce this on the JSONL artifact-writer side; in PTY
/// mode the system prompt carries the same prohibition since the
/// agent's native Write tool is out of our reach.
fn writes_to_sim_flow_state(p: &str) -> bool {
    let normalized = p.replace('\\', "/");
    normalized == ".sim-flow" || normalized.starts_with(".sim-flow/")
}

pub(super) fn write_artifact(
    project_dir: &Path,
    write_paths: &[String],
    art: &ExtractedArtifact,
) -> Result<u64> {
    if !is_safe_relative_path(&art.relative_path) {
        return Err(Error::Protocol(format!(
            "rejecting unsafe artifact path: {}",
            art.relative_path
        )));
    }
    if writes_to_sim_flow_state(&art.relative_path) {
        return Err(Error::Protocol(format!(
            "rejecting agent write to orchestrator state tree: {} (the `.sim-flow/` directory is read-only for the agent; write generated documents under `docs/`, project source under `src/`, etc.)",
            art.relative_path
        )));
    }
    if !crate::steps::is_path_allowed_for_writes(write_paths, &art.relative_path) {
        return Err(Error::Protocol(format!(
            "rejecting agent write to `{}`: outside the per-step write allowlist ({}). Update the artifact path to land under one of the allowed prefixes, or extend the step's `work_write_paths` if the new location is a deliberate widening.",
            art.relative_path,
            if write_paths.is_empty() {
                "(none)".to_string()
            } else {
                write_paths.join(", ")
            },
        )));
    }
    // is_safe_relative_path rejects absolute paths and any segment
    // containing "..", so `project_dir.join(<safe-relative>)` is
    // guaranteed to stay inside `project_dir` without needing a
    // canonicalize round-trip on a not-yet-existing file.
    let abs = project_dir.join(&art.relative_path);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(&abs, art.content.as_bytes()).map_err(|source| Error::Io {
        path: abs.clone(),
        source,
    })?;
    // When the agent writes a critique JSON, render the markdown
    // sibling immediately. The agent only emits the canonical JSON;
    // humans (and the gate's grep-the-md fallback for legacy
    // projects) read the rendered markdown. Render errors surface
    // as protocol errors so a malformed critique fails loud rather
    // than silently leaving a stale `.md` on disk.
    if crate::critique::is_critique_json_path(&art.relative_path) {
        crate::critique::render_critique_markdown_to_disk(project_dir, &art.relative_path)?;
    }
    Ok(art.content.len() as u64)
}

/// Resolve the foundation framework crate root from
/// `<foundation_root>/crates/framework/`. Returns `None` if the
/// expected layout isn't present (e.g. the foundation_root override
/// points somewhere other than the canonical sim-foundation tree).
pub(super) fn detect_framework_root(foundation_root: &Path) -> Option<PathBuf> {
    let candidate = foundation_root.join("crates").join("framework");
    if candidate.join("src").is_dir() {
        Some(candidate)
    } else {
        None
    }
}

pub(super) fn detect_framework_docs_root(foundation_root: &Path) -> Option<PathBuf> {
    if let Some(candidate) = std::env::var_os(FRAMEWORK_DOCS_ROOT_ENV).map(PathBuf::from)
        && is_framework_docs_root(&candidate)
    {
        return Some(candidate);
    }
    let candidate = foundation_root
        .join("target")
        .join("sim-flow-vscode-api-docs");
    if is_framework_docs_root(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

fn is_framework_docs_root(candidate: &Path) -> bool {
    candidate.join("toc.md").is_file() && candidate.join("pages").is_dir()
}

/// Walk up from `project_dir` looking for a directory that contains
/// both `docs/modeling-guide/` and `examples/`. That layout matches
/// the sim-models repo we want the agent to reference. Returns the
/// first such ancestor (highest in the tree); `None` if nothing in the
/// chain matches.
pub(super) fn detect_library_root(project_dir: &Path) -> Option<PathBuf> {
    // `SIM_FLOW_LIBRARY_ROOT` is the explicit override. The e2e binaries
    // and any caller running the orchestrator against a project that
    // doesn't live under sim-models (tempdir smoke projects, CI, etc.)
    // set this so the agent can still resolve `lib:examples/...` and
    // `lib:docs/modeling-guide/...` references the prompts depend on.
    if let Ok(s) = std::env::var("SIM_FLOW_LIBRARY_ROOT") {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            let p = PathBuf::from(trimmed);
            let docs = p.join("docs").join("modeling-guide");
            let examples = p.join("examples");
            if docs.is_dir() && examples.is_dir() {
                return Some(p);
            }
        }
    }
    let mut cursor = project_dir.to_path_buf();
    // Cap at 16 levels to avoid pathological infinite loops if the
    // canonical path resolution does anything weird.
    for _ in 0..16 {
        let docs = cursor.join("docs").join("modeling-guide");
        let examples = cursor.join("examples");
        if docs.is_dir() && examples.is_dir() {
            return Some(cursor);
        }
        if !cursor.pop() {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_artifacts_picks_fenced_blocks_with_paths() {
        let body = "Here is the spec.\n\n```spec.md\n# Spec\nClock: 2 GHz\n```\n\nDone.";
        let arts = extract_artifacts(body);
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].relative_path, "spec.md");
        assert_eq!(arts[0].content, "# Spec\nClock: 2 GHz");
    }

    #[test]
    fn extract_artifacts_ignores_language_only_fences() {
        let body = "```rust\nfn main() {}\n```\n";
        assert!(extract_artifacts(body).is_empty());
    }

    #[test]
    fn extract_artifacts_rejects_traversal_paths() {
        let body = "```../etc/passwd\nx\n```\n```/abs.md\nx\n```\n";
        assert!(extract_artifacts(body).is_empty());
    }

    #[test]
    fn extract_artifacts_keeps_latest_when_path_repeats() {
        let body = "```spec.md\nv1\n```\n```spec.md\nv2\n```\n";
        let arts = extract_artifacts(body);
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].content, "v2");
    }

    #[test]
    fn write_artifact_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = vec!["docs/".to_string()];
        let bytes = write_artifact(
            tmp.path(),
            &allowed,
            &ExtractedArtifact {
                relative_path: "docs/notes.md".into(),
                content: "hi".into(),
            },
        )
        .unwrap();
        assert_eq!(bytes, 2);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("docs/notes.md")).unwrap(),
            "hi"
        );
    }

    #[test]
    fn write_artifact_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = vec!["docs/".to_string()];
        let err = write_artifact(
            tmp.path(),
            &allowed,
            &ExtractedArtifact {
                relative_path: "../escape.md".into(),
                content: "x".into(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, Error::Protocol(_)));
    }

    #[test]
    fn write_artifact_rejects_orchestrator_state_writes() {
        // Agent must not touch anything under `.sim-flow/` -- past
        // runs have had the agent try to "fix" its own gate status
        // by editing state.toml. Cover the obvious targets and a
        // backslash-disguised variant.
        let tmp = tempfile::tempdir().unwrap();
        let allowed = vec!["docs/".to_string()];
        for bad in [
            ".sim-flow/state.toml",
            ".sim-flow/config.toml",
            ".sim-flow/critiques/DM0-critique.md",
            ".sim-flow/prompts/dm0-specification.work.md",
            ".sim-flow\\state.toml",
        ] {
            let err = write_artifact(
                tmp.path(),
                &allowed,
                &ExtractedArtifact {
                    relative_path: bad.into(),
                    content: "tampered".into(),
                },
            )
            .unwrap_err();
            let msg = format!("{err}");
            assert!(
                msg.contains("orchestrator state tree"),
                "expected state-tree rejection for {bad:?}, got: {msg}",
            );
        }
    }

    #[test]
    fn write_artifact_rejects_paths_outside_write_allowlist() {
        // The per-step write allowlist gates fenced artifact-write
        // blocks, not just `write_file` tool calls. A step whose
        // allowlist is `["docs/"]` must reject a fenced ` ```src/lib.rs `
        // block — otherwise the allowlist would only constrain the
        // tool-use API, leaving the artifact-write convention as a
        // bypass.
        let tmp = tempfile::tempdir().unwrap();
        let allowed = vec!["docs/".to_string()];
        let err = write_artifact(
            tmp.path(),
            &allowed,
            &ExtractedArtifact {
                relative_path: "src/lib.rs".into(),
                content: "fn main() {}".into(),
            },
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("write allowlist"),
            "expected allowlist rejection, got: {msg}",
        );
    }
}
