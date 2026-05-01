//! Codex CLI subprocess wrapper.
//!
//! Interactive mode launches `codex` with the prompt as the initial user
//! message; OneShot mode uses `codex exec` and captures output. Codex
//! reads project context from `AGENTS.md` automatically.

use std::process::Command;

use crate::client::{Client, Invocation, Session, SessionMode};
use crate::config::CodexSettings;
use crate::{Error, Result};

const DEFAULT_BIN: &str = "codex";

#[derive(Debug, Clone)]
pub struct CodexClient {
    settings: CodexSettings,
}

impl CodexClient {
    pub fn new(settings: CodexSettings) -> Self {
        Self { settings }
    }

    fn resolve_bin() -> String {
        std::env::var("SIM_FLOW_CODEX_BIN").unwrap_or_else(|_| DEFAULT_BIN.to_string())
    }

    fn build_prompt(invocation: &Invocation) -> String {
        format!(
            "{}\n\n---\n\n{}",
            invocation.instructions, invocation.prompt
        )
    }
}

impl Client for CodexClient {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn invoke(&self, invocation: &Invocation) -> Result<Session> {
        let bin = Self::resolve_bin();
        let combined = Self::build_prompt(invocation);
        let mut cmd = Command::new(&bin);
        cmd.current_dir(&invocation.project_dir);
        if let Some(model) = &self.settings.model {
            cmd.arg("--model").arg(model);
        }
        match invocation.mode {
            SessionMode::Interactive => {
                // Interactive Codex: pass the prompt as the initial
                // message and inherit stdio so the user drives the TUI.
                cmd.arg(combined);
                let status = cmd.status().map_err(|source| Error::Io {
                    path: bin.clone().into(),
                    source,
                })?;
                Ok(Session {
                    exit_status: status.code().unwrap_or(-1),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
            SessionMode::OneShot => {
                cmd.arg("exec").arg(combined);
                if let Some(sandbox) = &self.settings.sandbox {
                    cmd.arg("-s").arg(sandbox);
                }
                if let Some(approval) = &self.settings.approval {
                    cmd.arg("-a").arg(approval);
                }
                let output = cmd.output().map_err(|source| Error::Io {
                    path: bin.into(),
                    source,
                })?;
                Ok(Session {
                    exit_status: output.status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                })
            }
        }
    }
}
