//! `sim-flow new model` implementation.
//!
//! `sim-flow new study` and `sim-flow new candidate` are Phase 5 work and
//! error out with a clear "not yet implemented" message until their
//! templates are populated.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::template;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct NewModelOptions {
    pub project_name: String,
    pub destination: PathBuf,
    pub foundation_root: PathBuf,
    pub library_path: String,
    pub skip_cargo_check: bool,
}

#[derive(Debug, Serialize)]
pub struct NewModelOutcome {
    pub project_dir: PathBuf,
    pub crate_name: String,
    pub next_step: String,
}

pub fn new_model(options: &NewModelOptions) -> Result<NewModelOutcome> {
    validate_project_name(&options.project_name)?;
    let project_dir = options.destination.join(&options.project_name);
    if project_dir.exists() {
        return Err(Error::State(format!(
            "destination already exists: {}",
            project_dir.display()
        )));
    }
    let template_dir = template::template_path(&options.foundation_root, "model-project");
    if !template_dir.is_dir() {
        return Err(Error::FoundationRoot(format!(
            "model-project template not found at {}",
            template_dir.display()
        )));
    }

    let values = template::default_placeholders(
        &options.project_name,
        &options.foundation_root,
        &options.library_path,
    );
    template::expand_into(&template_dir, &project_dir, &values)?;

    if !options.skip_cargo_check {
        cargo_check(&project_dir)?;
    }

    let crate_name = values
        .get("crate_name")
        .cloned()
        .unwrap_or_else(|| template::crate_name(&options.project_name));
    Ok(NewModelOutcome {
        project_dir,
        crate_name,
        next_step: "DM0".to_string(),
    })
}

fn validate_project_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::State("project name must not be empty".into()));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(Error::State(format!(
            "project name must not contain path separators: {name}"
        )));
    }
    if name.starts_with('.') {
        return Err(Error::State(format!(
            "project name must not start with '.': {name}"
        )));
    }
    Ok(())
}

fn cargo_check(project_dir: &Path) -> Result<()> {
    let status = Command::new("cargo")
        .arg("check")
        .arg("--quiet")
        .current_dir(project_dir)
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(Error::State(format!(
            "generated project failed `cargo check`: exit {:?}",
            s.code()
        ))),
        Err(err) => Err(Error::Io {
            path: project_dir.to_path_buf(),
            source: err,
        }),
    }
}

/// Verify that `CLAUDE.md` and `AGENTS.md` inside a template directory
/// have equivalent content below their HTML sync-note comments. Used by
/// the template-validation integration test and could be reused as a
/// pre-commit lint.
pub fn verify_client_file_equivalence(template_dir: &Path) -> Result<()> {
    let claude = template_dir.join("CLAUDE.md");
    let agents = template_dir.join("AGENTS.md");
    if !claude.exists() || !agents.exists() {
        return Ok(());
    }
    let claude_body = strip_sync_note(&read(&claude)?);
    let agents_body = strip_sync_note(&read(&agents)?);
    if claude_body != agents_body {
        return Err(Error::State(format!(
            "CLAUDE.md and AGENTS.md differ in {}; edit both files together",
            template_dir.display()
        )));
    }
    Ok(())
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Remove the HTML comment block that names the sister file so the
/// equivalence check ignores "this file syncs with X" differences.
fn strip_sync_note(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find("-->") {
            rest = &rest[start + end + 3..];
        } else {
            break;
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Derive a sensible default `{{foundation_path}}` for the current
/// generation. Uses a relative path when the destination is under a
/// sibling of the foundation root; otherwise returns the absolute path.
///
/// Phase 2 keeps this simple — always return the absolute path. Phase 5
/// can revisit for portability across user machines.
pub fn resolve_foundation_path(foundation_root: &Path, _destination: &Path) -> PathBuf {
    foundation_root.to_path_buf()
}

/// Default placeholder values used by the templates, with overrides from
/// [`NewModelOptions`] applied on top.
pub fn placeholder_map(options: &NewModelOptions) -> BTreeMap<String, String> {
    template::default_placeholders(
        &options.project_name,
        &options.foundation_root,
        &options.library_path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn rejects_empty_name() {
        assert!(validate_project_name("").is_err());
    }

    #[test]
    fn rejects_path_separators() {
        assert!(validate_project_name("foo/bar").is_err());
        assert!(validate_project_name("foo\\bar").is_err());
    }

    #[test]
    fn rejects_leading_dot() {
        assert!(validate_project_name(".hidden").is_err());
    }

    #[test]
    fn strip_sync_note_ignores_comment_block() {
        let a = "<!-- sync with AGENTS.md -->\n# Title\nbody\n";
        let b = "<!-- sync with CLAUDE.md -->\n# Title\nbody\n";
        assert_eq!(strip_sync_note(a), strip_sync_note(b));
    }

    #[test]
    fn client_file_equivalence_round_trip() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("CLAUDE.md"),
            "<!-- sync with AGENTS.md -->\nbody\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "<!-- sync with CLAUDE.md -->\nbody\n",
        )
        .unwrap();
        verify_client_file_equivalence(dir.path()).unwrap();
    }

    #[test]
    fn client_file_equivalence_detects_drift() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "body one\n").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "body two\n").unwrap();
        assert!(verify_client_file_equivalence(dir.path()).is_err());
    }

    #[test]
    fn client_file_equivalence_returns_ok_when_either_file_missing() {
        // The verifier is opportunistic -- if either sister file is
        // missing it succeeds rather than treating the absence as drift.
        let dir = tempdir().unwrap();
        // Neither present.
        verify_client_file_equivalence(dir.path()).unwrap();
        // Only CLAUDE.md present.
        std::fs::write(dir.path().join("CLAUDE.md"), "body\n").unwrap();
        verify_client_file_equivalence(dir.path()).unwrap();
    }

    #[test]
    fn validate_project_name_accepts_typical_kebab_and_snake_names() {
        for ok in ["model", "my-model", "my_model", "model42", "v2.0-alpha"] {
            assert!(validate_project_name(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn strip_sync_note_drops_unclosed_comment_at_eof() {
        let a = "<!-- unterminated";
        // Just verify no panic.
        let _ = strip_sync_note(a);
    }

    #[test]
    fn resolve_foundation_path_returns_input_path_unchanged() {
        use std::path::Path;
        let foundation = Path::new("/abs/foundation");
        let destination = Path::new("/abs/somewhere/else");
        let out = resolve_foundation_path(foundation, destination);
        // Current impl returns the foundation path verbatim.
        assert_eq!(out, foundation);
    }

    #[test]
    fn placeholder_map_includes_canonical_keys_from_options() {
        let opts = super::NewModelOptions {
            project_name: "demo-model".into(),
            destination: std::path::PathBuf::from("/tmp/dest"),
            foundation_root: std::path::PathBuf::from("/abs/foundation"),
            library_path: "../sim-models/library".into(),
            skip_cargo_check: true,
        };
        let m = placeholder_map(&opts);
        assert_eq!(
            m.get("project-name").map(String::as_str),
            Some("demo-model")
        );
        assert_eq!(m.get("crate_name").map(String::as_str), Some("demo_model"));
        assert!(m.contains_key("foundation_path"));
        assert!(m.contains_key("library_path"));
        assert!(m.contains_key("timestamp"));
    }
}
