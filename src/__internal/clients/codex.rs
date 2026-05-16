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

impl CodexClient {
    /// Test hook mirroring `Client::invoke` but with an explicit
    /// binary so tests don't need to touch process-wide env vars.
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
    fn name_returns_codex() {
        let client = CodexClient::new(CodexSettings::default());
        assert_eq!(client.name(), "codex");
    }

    #[test]
    fn build_prompt_concatenates_with_separator() {
        let inv = invocation(SessionMode::OneShot, "do work", "rules");
        let out = CodexClient::build_prompt(&inv);
        assert_eq!(out, "rules\n\n---\n\ndo work");
    }

    #[test]
    fn invoke_oneshot_uses_exec_subcommand() {
        let client = CodexClient::new(CodexSettings::default());
        let inv = invocation(SessionMode::OneShot, "hello", "rules");
        let session = client
            .invoke_with_bin(&inv, "/bin/echo")
            .expect("echo runs");
        assert!(session.success());
        // /bin/echo prints all args; expect "exec" to appear since
        // OneShot prefixes it before the prompt.
        assert!(
            session.stdout.contains("exec"),
            "expected `exec` arg in stdout; got {:?}",
            session.stdout
        );
    }

    #[test]
    fn invoke_oneshot_forwards_sandbox_and_approval_flags() {
        let settings = CodexSettings {
            sandbox: Some("workspace-write".into()),
            approval: Some("never".into()),
            ..CodexSettings::default()
        };
        let client = CodexClient::new(settings);
        let inv = invocation(SessionMode::OneShot, "x", "y");
        let session = client
            .invoke_with_bin(&inv, "/bin/echo")
            .expect("echo runs");
        assert!(session.stdout.contains("-s") && session.stdout.contains("workspace-write"));
        assert!(session.stdout.contains("-a") && session.stdout.contains("never"));
    }

    #[test]
    fn invoke_with_missing_binary_returns_io_error() {
        let client = CodexClient::new(CodexSettings::default());
        let inv = invocation(SessionMode::OneShot, "x", "y");
        let result = client.invoke_with_bin(&inv, "/no/such/binary/here-on-purpose-3f8a");
        assert!(matches!(result, Err(Error::Io { .. })));
    }
}
