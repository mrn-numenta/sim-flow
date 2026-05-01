//! Deterministic mock AI client for tests and the end-to-end smoke test.
//!
//! The mock does not talk to any LLM. It applies a set of instructions
//! staged on disk so that each session produces the artifacts and critique
//! file the orchestrator expects to see.
//!
//! Fixture layout:
//! ```text
//! <fixtures>/
//!     <step>.work/
//!         <relative-path-1>        # file to copy into project_dir
//!         ...
//!     <step>.critique/
//!         <relative-path-1>
//!         ...
//! ```
//!
//! For per-candidate steps, a fixture directory named
//! `<step>.work.<candidate>` is consulted before the generic fallback.
//!
//! The fixtures root can be configured either via
//! [`MockClient::with_fixtures`] or the `SIM_FLOW_MOCK_RESPONSES_DIR`
//! environment variable.

use std::path::{Path, PathBuf};

use crate::client::{Client, Invocation, Session, SessionKind};
use crate::{Error, Result};

#[derive(Debug, Default, Clone)]
pub struct MockClient {
    fixtures: Option<PathBuf>,
}

impl MockClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_fixtures(path: impl Into<PathBuf>) -> Self {
        Self {
            fixtures: Some(path.into()),
        }
    }

    pub fn from_env() -> Self {
        let fixtures = std::env::var("SIM_FLOW_MOCK_RESPONSES_DIR")
            .ok()
            .map(PathBuf::from);
        Self { fixtures }
    }

    fn fixture_dir(&self, invocation: &Invocation) -> Option<PathBuf> {
        let root = self.fixtures.as_ref()?;
        let kind = match invocation.kind {
            SessionKind::Work => "work",
            SessionKind::Critique => "critique",
        };
        if let Some(candidate) = invocation.candidate.as_deref() {
            let candidate_dir = root.join(format!("{}.{}.{}", invocation.step, kind, candidate));
            if candidate_dir.is_dir() {
                return Some(candidate_dir);
            }
        }
        let default_dir = root.join(format!("{}.{}", invocation.step, kind));
        if default_dir.is_dir() {
            Some(default_dir)
        } else {
            None
        }
    }
}

impl Client for MockClient {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn invoke(&self, invocation: &Invocation) -> Result<Session> {
        if let Some(src) = self.fixture_dir(invocation) {
            copy_tree(&src, &invocation.project_dir)?;
        }
        Ok(Session {
            exit_status: 0,
            stdout: format!(
                "mock client applied fixtures for step {} ({:?})",
                invocation.step, invocation.kind
            ),
            stderr: String::new(),
        })
    }
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    if !src.is_dir() {
        return Err(Error::Client(format!(
            "mock fixture source is not a directory: {}",
            src.display()
        )));
    }
    std::fs::create_dir_all(dst).map_err(|source| Error::Io {
        path: dst.to_path_buf(),
        source,
    })?;
    let entries = std::fs::read_dir(src).map_err(|source| Error::Io {
        path: src.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: src.to_path_buf(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| Error::Io {
            path: entry.path(),
            source,
        })?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            std::fs::copy(&from, &to).map_err(|source| Error::Io {
                path: from.clone(),
                source,
            })?;
        }
    }
    Ok(())
}
