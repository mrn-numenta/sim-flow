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

    ensure_cargo_generate_installed()?;

    let placeholders = template::default_placeholders(&options.project_name, &options.library_path);
    run_cargo_generate(
        &template_dir,
        &options.destination,
        &options.project_name,
        &placeholders,
    )?;

    if options.skip_cargo_check {
        cargo_generate_lockfile(&project_dir)?;
    } else {
        cargo_check(&project_dir)?;
    }

    let crate_name = placeholders
        .get("crate_name")
        .cloned()
        .unwrap_or_else(|| template::crate_name(&options.project_name));
    Ok(NewModelOutcome {
        project_dir,
        crate_name,
        next_step: "DM0".to_string(),
    })
}

/// Verify `cargo generate` is available; install it if missing.
///
/// Detection: shell out to `cargo generate --version`. If the
/// invocation succeeds the subcommand is on PATH and we're done.
/// Otherwise run `cargo install cargo-generate` and re-probe. The
/// install is a one-time cost (cargo caches the binary under
/// `$CARGO_HOME/bin`) but takes minutes on first run; surface a
/// clear diagnostic so the caller knows that's the expected
/// behavior.
fn ensure_cargo_generate_installed() -> Result<()> {
    if cargo_generate_available() {
        return Ok(());
    }
    eprintln!(
        "[sim-flow new model] cargo-generate not found; installing via `cargo install cargo-generate` \
         (one-time, may take a few minutes)..."
    );
    let status = Command::new("cargo")
        .arg("install")
        .arg("cargo-generate")
        .arg("--locked")
        .status()
        .map_err(|source| Error::Io {
            path: PathBuf::from("cargo"),
            source,
        })?;
    if !status.success() {
        return Err(Error::State(format!(
            "failed to install cargo-generate: exit {:?}. \
             Install it manually with `cargo install cargo-generate` and retry.",
            status.code()
        )));
    }
    if !cargo_generate_available() {
        return Err(Error::State(
            "cargo-generate install reported success but `cargo generate --version` still fails. \
             Check $CARGO_HOME/bin is on PATH and retry."
                .into(),
        ));
    }
    Ok(())
}

fn cargo_generate_available() -> bool {
    Command::new("cargo")
        .arg("generate")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Invoke `cargo generate --path <template> --name <name> --destination <dest>
/// --silent --vcs none --define KEY=VALUE...`.
///
/// `--silent` requires every placeholder declared in the template's
/// `cargo-generate.toml` to have a value (either a default or a
/// `--define` override); we pass `--define` for each custom
/// placeholder so the run is fully deterministic.
///
/// `--vcs none` skips git init -- sim-flow projects are typically
/// committed under a parent repo (sim-models) and the auto-init would
/// produce nested `.git` dirs.
fn run_cargo_generate(
    template_dir: &Path,
    destination: &Path,
    project_name: &str,
    placeholders: &BTreeMap<String, String>,
) -> Result<()> {
    std::fs::create_dir_all(destination).map_err(|source| Error::Io {
        path: destination.to_path_buf(),
        source,
    })?;
    let mut cmd = Command::new("cargo");
    cmd.arg("generate")
        .arg("--path")
        .arg(template_dir)
        .arg("--name")
        .arg(project_name)
        .arg("--destination")
        .arg(destination)
        .arg("--silent")
        .arg("--vcs")
        .arg("none");
    // cargo-generate errors in headless/container environments where
    // `$USER` is unset. Normalize it from common alternates or use a
    // deterministic fallback so project scaffolding works in CI too.
    cmd.env("USER", cargo_generate_user_env_value());
    // Pass custom placeholders explicitly. The built-ins
    // (`project-name`, `crate_name`) are derived by cargo-generate
    // from `--name`, so we skip those here even though they appear
    // in the placeholder map.
    for (key, value) in placeholders {
        if key == "project-name" || key == "crate_name" {
            continue;
        }
        cmd.arg("--define").arg(format!("{key}={value}"));
    }
    let output = cmd.output().map_err(|source| Error::Io {
        path: PathBuf::from("cargo generate"),
        source,
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(Error::State(format!(
            "cargo generate failed (exit {:?}):\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            output.status.code()
        )));
    }
    // The template ships its `Cargo.toml` as `Cargo.toml.tmpl` so cargo's
    // package-discovery walk (run by consumers like sim-models when this
    // crate is a git dep) doesn't try to parse the file's `{{crate_name}}`
    // placeholder. cargo-generate expands the placeholders on copy but
    // preserves the filename, so we rename the expanded file back to its
    // real name here.
    let project_dir = destination.join(project_name);
    let tmpl = project_dir.join("Cargo.toml.tmpl");
    let real = project_dir.join("Cargo.toml");
    if tmpl.is_file() {
        std::fs::rename(&tmpl, &real).map_err(|source| Error::Io { path: tmpl, source })?;
    }
    Ok(())
}

fn cargo_generate_user_env_value() -> String {
    resolve_user_env_value(|key| std::env::var(key).ok())
}

fn resolve_user_env_value<F>(lookup: F) -> String
where
    F: Fn(&str) -> Option<String>,
{
    for key in ["USER", "USERNAME", "LOGNAME"] {
        if let Some(value) = lookup(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "sim-flow".to_string()
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

fn cargo_generate_lockfile(project_dir: &Path) -> Result<()> {
    let status = Command::new("cargo")
        .arg("generate-lockfile")
        .arg("--quiet")
        .current_dir(project_dir)
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(Error::State(format!(
            "generated project failed `cargo generate-lockfile`: exit {:?}",
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

/// Default placeholder values used by the templates, with overrides from
/// [`NewModelOptions`] applied on top.
pub fn placeholder_map(options: &NewModelOptions) -> BTreeMap<String, String> {
    template::default_placeholders(&options.project_name, &options.library_path)
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
        let a = "<!-- sync with AGENTS.md -->
# Title
body
";
        let b = "<!-- sync with CLAUDE.md -->
# Title
body
";
        assert_eq!(strip_sync_note(a), strip_sync_note(b));
    }

    #[test]
    fn client_file_equivalence_round_trip() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("CLAUDE.md"),
            "<!-- sync with AGENTS.md -->
body
",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "<!-- sync with CLAUDE.md -->
body
",
        )
        .unwrap();
        verify_client_file_equivalence(dir.path()).unwrap();
    }

    #[test]
    fn client_file_equivalence_detects_drift() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("CLAUDE.md"),
            "body one
",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "body two
",
        )
        .unwrap();
        assert!(verify_client_file_equivalence(dir.path()).is_err());
    }

    #[test]
    fn client_file_equivalence_returns_ok_when_either_file_missing() {
        let dir = tempdir().unwrap();
        verify_client_file_equivalence(dir.path()).unwrap();
        std::fs::write(
            dir.path().join("CLAUDE.md"),
            "body
",
        )
        .unwrap();
        verify_client_file_equivalence(dir.path()).unwrap();
    }

    #[test]
    fn resolve_user_env_value_prefers_user() {
        let user = resolve_user_env_value(|key| match key {
            "USER" => Some("alice".to_string()),
            "USERNAME" => Some("bob".to_string()),
            "LOGNAME" => Some("carol".to_string()),
            _ => None,
        });
        assert_eq!(user, "alice");
    }

    #[test]
    fn resolve_user_env_value_falls_back_to_username_then_logname() {
        let username = resolve_user_env_value(|key| match key {
            "USER" => Some("   ".to_string()),
            "USERNAME" => Some("builder".to_string()),
            "LOGNAME" => Some("ignored".to_string()),
            _ => None,
        });
        assert_eq!(username, "builder");

        let logname = resolve_user_env_value(|key| match key {
            "USER" => None,
            "USERNAME" => Some(String::new()),
            "LOGNAME" => Some("runner".to_string()),
            _ => None,
        });
        assert_eq!(logname, "runner");
    }

    #[test]
    fn resolve_user_env_value_uses_default_when_unset() {
        let user = resolve_user_env_value(|_| None);
        assert_eq!(user, "sim-flow");
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
        let _ = strip_sync_note(a);
    }

    #[test]
    fn placeholder_map_includes_canonical_keys_from_options() {
        let opts = super::NewModelOptions {
            project_name: "demo-model".into(),
            destination: std::path::PathBuf::from("/tmp/dest"),
            foundation_root: std::path::PathBuf::from("/abs/sim-flow"),
            library_path: "../sim-models/library".into(),
            skip_cargo_check: true,
        };
        let m = placeholder_map(&opts);
        assert_eq!(
            m.get("project-name").map(String::as_str),
            Some("demo-model")
        );
        assert_eq!(m.get("crate_name").map(String::as_str), Some("demo_model"));
        assert_eq!(
            m.get("foundation_repo").map(String::as_str),
            Some(template::SIM_FOUNDATION_GIT_URL)
        );
        assert_eq!(
            m.get("foundation_rev").map(String::as_str),
            Some(template::foundation_rev())
        );
        assert_eq!(
            m.get("library_path").map(String::as_str),
            Some("../sim-models/library")
        );
        assert_eq!(
            m.get("sim_flow_repo").map(String::as_str),
            Some(template::SIM_FLOW_GIT_URL)
        );
        assert_eq!(
            m.get("sim_flow_rev").map(String::as_str),
            Some(template::sim_flow_rev())
        );
        assert_eq!(
            m.get("sim_flow_version").map(String::as_str),
            Some(template::sim_flow_version())
        );
        assert!(m.contains_key("timestamp"));
    }
}
