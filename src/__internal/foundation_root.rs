//! Resolve the sim-foundation repository root so the orchestrator can find
//! templates and instruction files.
//!
//! Precedence (highest first):
//!   1. explicit `--foundation-root` argument (caller-supplied)
//!   2. `SIM_FOUNDATION_ROOT` environment variable
//!   3. walk up from the binary location looking for a workspace Cargo.toml
//!      that declares the `sim-flow` member

use std::path::{Path, PathBuf};

use crate::{Error, Result};

const ENV_VAR: &str = "SIM_FOUNDATION_ROOT";

/// Resolve the sim-foundation root from an optional explicit override.
pub fn resolve(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return canonicalize(path);
    }
    if let Ok(value) = std::env::var(ENV_VAR) {
        return canonicalize(Path::new(&value));
    }
    walk_up_from_current_exe().or_else(|_| {
        let cwd = std::env::current_dir().map_err(|source| Error::Io {
            path: PathBuf::from("."),
            source,
        })?;
        walk_up(&cwd)
    })
}

fn canonicalize(path: &Path) -> Result<PathBuf> {
    path.canonicalize().map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn walk_up_from_current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().map_err(|source| Error::Io {
        path: PathBuf::from("<current_exe>"),
        source,
    })?;
    walk_up(&exe)
}

fn walk_up(start: &Path) -> Result<PathBuf> {
    let mut cursor = start.to_path_buf();
    loop {
        let candidate = cursor.join("Cargo.toml");
        if candidate.is_file() && is_sim_foundation_workspace(&candidate)? {
            return Ok(cursor);
        }
        if !cursor.pop() {
            return Err(Error::FoundationRoot(format!(
                "walked to filesystem root from {} without finding sim-foundation workspace",
                start.display()
            )));
        }
    }
}

fn is_sim_foundation_workspace(cargo_toml: &Path) -> Result<bool> {
    let text = std::fs::read_to_string(cargo_toml).map_err(|source| Error::Io {
        path: cargo_toml.to_path_buf(),
        source,
    })?;
    Ok(text.contains("tools/sim-flow"))
}
