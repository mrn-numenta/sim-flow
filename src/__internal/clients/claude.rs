//! Claude Code subprocess wrapper.
//!
//! In [`SessionMode::Interactive`] we launch `claude` with the step
//! prompt + instructions concatenated as the first user message and
//! inherit the parent's stdio so the user drives the TUI. Exit via
//! `/exit` or Ctrl-D returns control to the orchestrator.
//!
//! In [`SessionMode::OneShot`] we use `claude -p <prompt>` and capture
//! stdout/stderr. The optional `SIM_FLOW_CLAUDE_BIN` env var overrides
//! the binary name for tests.

use std::process::Command;

use crate::client::{Client, Invocation, Session, SessionMode};
use crate::config::ClaudeSettings;
use crate::{Error, Result};

const DEFAULT_BIN: &str = "claude";

#[derive(Debug, Clone)]
pub struct ClaudeClient {
    settings: ClaudeSettings,
}

impl ClaudeClient {
    pub fn new(settings: ClaudeSettings) -> Self {
        Self { settings }
    }

    fn resolve_bin() -> String {
        std::env::var("SIM_FLOW_CLAUDE_BIN").unwrap_or_else(|_| DEFAULT_BIN.to_string())
    }

    fn build_prompt(invocation: &Invocation) -> String {
        format!(
            "{}\n\n---\n\n{}",
            invocation.instructions, invocation.prompt
        )
    }
}

impl Client for ClaudeClient {
    fn name(&self) -> &'static str {
        "claude"
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
                // Seed the TUI with the prompt as the first user message,
                // then hand control to the user. Inherited stdio (Rust's
                // default) attaches the child to the terminal.
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
                cmd.arg("-p").arg(combined);
                if !self.settings.allowed_tools.is_empty() {
                    cmd.arg("--allowedTools")
                        .arg(self.settings.allowed_tools.join(","));
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
