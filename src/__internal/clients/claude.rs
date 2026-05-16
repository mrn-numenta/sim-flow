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

impl ClaudeClient {
    /// Test hook: same logic as `Client::invoke` but accepts an
    /// explicit binary so tests don't have to mutate process-wide
    /// env vars. `pub(crate)` because the only call site outside
    /// the `Client` trait impl is `#[cfg(test)]`.
    #[cfg(test)]
    pub(crate) fn invoke_with_bin(&self, invocation: &Invocation, bin: &str) -> Result<Session> {
        let combined = Self::build_prompt(invocation);
        let mut cmd = Command::new(bin);
        cmd.current_dir(&invocation.project_dir);
        if let Some(model) = &self.settings.model {
            cmd.arg("--model").arg(model);
        }
        match invocation.mode {
            SessionMode::Interactive => {
                cmd.arg(combined);
                let status = cmd.status().map_err(|source| Error::Io {
                    path: bin.into(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::SessionKind;
    use std::path::PathBuf;

    fn invocation(mode: SessionMode, prompt: &str, instructions: &str) -> Invocation {
        Invocation {
            step: "DM0".into(),
            kind: SessionKind::Work,
            mode,
            prompt: prompt.into(),
            instructions: instructions.into(),
            project_dir: PathBuf::from("."),
            candidate: None,
            timeout_seconds: None,
        }
    }

    #[test]
    fn name_returns_claude() {
        let client = ClaudeClient::new(ClaudeSettings::default());
        assert_eq!(client.name(), "claude");
    }

    #[test]
    fn build_prompt_concatenates_with_separator() {
        let inv = invocation(SessionMode::OneShot, "do work", "rules: be careful");
        let out = ClaudeClient::build_prompt(&inv);
        assert_eq!(out, "rules: be careful\n\n---\n\ndo work");
    }

    #[test]
    fn build_prompt_handles_empty_instructions() {
        let inv = invocation(SessionMode::OneShot, "do work", "");
        let out = ClaudeClient::build_prompt(&inv);
        assert_eq!(out, "\n\n---\n\ndo work");
    }

    #[test]
    fn invoke_oneshot_captures_stdout_from_real_subprocess() {
        // Use /bin/echo (POSIX standard) as a stand-in so we
        // exercise the full Command-spawn -> output -> Session
        // path without depending on `claude` being installed.
        // /bin/echo prints its argv joined by spaces; we expect
        // to see the prompt body in stdout.
        let client = ClaudeClient::new(ClaudeSettings::default());
        let inv = invocation(SessionMode::OneShot, "hello", "rules");
        let session = client
            .invoke_with_bin(&inv, "/bin/echo")
            .expect("echo runs successfully");
        assert!(session.success());
        assert_eq!(session.exit_status, 0);
        assert!(
            session.stdout.contains("hello") && session.stdout.contains("rules"),
            "echo should have surfaced the combined prompt; got {:?}",
            session.stdout
        );
    }

    #[test]
    fn invoke_oneshot_includes_allowed_tools_flag_when_set() {
        let settings = ClaudeSettings {
            allowed_tools: vec!["read_file".into(), "write_file".into()],
            ..ClaudeSettings::default()
        };
        let client = ClaudeClient::new(settings);
        let inv = invocation(SessionMode::OneShot, "x", "y");
        let session = client
            .invoke_with_bin(&inv, "/bin/echo")
            .expect("echo runs successfully");
        assert!(
            session.stdout.contains("--allowedTools")
                && session.stdout.contains("read_file,write_file"),
            "expected --allowedTools flag with comma-joined list; got {:?}",
            session.stdout
        );
    }

    #[test]
    fn invoke_with_missing_binary_returns_io_error() {
        let client = ClaudeClient::new(ClaudeSettings::default());
        let inv = invocation(SessionMode::OneShot, "x", "y");
        let result = client.invoke_with_bin(&inv, "/no/such/binary/here-on-purpose-3f8a");
        match result {
            Err(Error::Io { .. }) => {}
            other => panic!("expected Error::Io for missing binary; got {:?}", other),
        }
    }
}
