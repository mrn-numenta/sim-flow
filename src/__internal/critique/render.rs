//! Render canonical critique JSON into the markdown view the
//! orchestrator emits alongside it. The agent writes only the JSON;
//! `render_critique_markdown_to_disk` produces the human-readable
//! `.md` sibling each pass.

use std::path::Path;

use crate::{Error, Result};

use super::{CritiqueFinding, CritiqueJson};

/// Map a `<step>-critique.md` path to its expected JSON sibling. For
/// `docs/critiques/DM3a-critique.md`, returns
/// `docs/critiques/DM3a-critique.json`. Returns `None` for paths
/// that don't end in `.md` (already a JSON path, or some other
/// extension we don't expect).
pub fn json_sibling(md_path: &Path) -> Option<std::path::PathBuf> {
    let ext = md_path.extension().and_then(|e| e.to_str())?;
    if !ext.eq_ignore_ascii_case("md") {
        return None;
    }
    Some(md_path.with_extension("json"))
}

/// True iff `rel_path` looks like a critique JSON artifact path
/// (`docs/critiques/<step>-critique.json`). Used by the
/// orchestrator to decide whether to render a markdown view after
/// the agent writes the file.
pub fn is_critique_json_path(rel_path: &str) -> bool {
    let normalized = rel_path.replace('\\', "/");
    normalized.starts_with("docs/critiques/")
        && normalized.ends_with("-critique.json")
        && !normalized.contains("..")
}

/// Render a freshly-written critique JSON into its markdown
/// sibling. Idempotent: re-runs overwrite the previous render.
/// Errors are wrapped so the orchestrator can surface "agent
/// emitted malformed critique JSON" as a clear failure rather
/// than silently leaving a stale `.md` on disk.
///
/// Returns the markdown bytes that were written so the caller can
/// emit an `ArtifactWritten` event for the rendered file.
pub fn render_critique_markdown_to_disk(
    project_dir: &Path,
    json_rel_path: &str,
) -> Result<Vec<u8>> {
    let json_abs = project_dir.join(json_rel_path);
    let body = std::fs::read_to_string(&json_abs).map_err(|source| Error::Io {
        path: json_abs.clone(),
        source,
    })?;
    let parsed: CritiqueJson = serde_json::from_str(&body).map_err(|err| {
        Error::State(format!(
            "render_critique_markdown_to_disk: malformed critique JSON at {}: {err}",
            json_abs.display()
        ))
    })?;
    let md = render_markdown(&parsed);
    let md_rel = json_rel_path
        .strip_suffix(".json")
        .map(|s| format!("{s}.md"));
    let md_abs = match md_rel {
        Some(rel) => project_dir.join(rel),
        None => {
            return Err(Error::State(format!(
                "render_critique_markdown_to_disk: expected .json suffix on {json_rel_path}"
            )));
        }
    };
    if let Some(parent) = md_abs.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(&md_abs, md.as_bytes()).map_err(|source| Error::Io {
        path: md_abs.clone(),
        source,
    })?;
    Ok(md.into_bytes())
}

/// Render a JSON critique into deterministic markdown. The
/// orchestrator runs this each pass after the agent writes the
/// JSON so humans get a stable view without the agent having to
/// emit two artifacts. Output shape:
///
/// ```text
/// # <step> Critique
///
/// <summary>
///
/// ## <section-1>
///
/// - **BLOCKER:** title-1
///   body-1
/// - **UNRESOLVED:** title-2
///   body-2
///
/// ## <section-2>
/// ...
///
/// ## Notes
///
/// <notes>
/// ```
///
/// Sections are emitted in the order they FIRST appear in
/// `findings` (so the agent controls grouping); findings without a
/// section are bucketed under `## Findings`.
pub fn render_markdown(json: &CritiqueJson) -> String {
    let mut out = format!("# {} Critique\n\n", json.step);
    if !json.summary.trim().is_empty() {
        out.push_str(json.summary.trim());
        out.push_str("\n\n");
    }

    // Group findings by section, preserving first-appearance order.
    let mut sections: Vec<(String, Vec<&CritiqueFinding>)> = Vec::new();
    for f in &json.findings {
        let section_key = if f.section.trim().is_empty() {
            "Findings".to_string()
        } else {
            f.section.trim().to_string()
        };
        match sections.iter_mut().find(|(k, _)| k == &section_key) {
            Some(entry) => entry.1.push(f),
            None => sections.push((section_key, vec![f])),
        }
    }

    for (section, findings) in &sections {
        out.push_str(&format!("## {section}\n\n"));
        for f in findings {
            out.push_str(&format!(
                "- **{}:** {}\n",
                f.kind.as_label(),
                f.title.trim()
            ));
            let body = f.body.trim();
            if !body.is_empty() {
                for line in body.lines() {
                    out.push_str(&format!("  {line}\n"));
                }
            }
        }
        out.push('\n');
    }

    if !json.notes.trim().is_empty() {
        out.push_str("## Notes\n\n");
        out.push_str(json.notes.trim());
        out.push('\n');
    }

    out
}
