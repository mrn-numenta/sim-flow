//! Copilot CLI subprocess wrapper.

use std::process::Command;

use crate::client::{Client, Invocation, Session, SessionMode};
use crate::config::CopilotSettings;
use crate::{Error, Result};

const DEFAULT_BIN: &str = "copilot";

#[derive(Debug, Clone)]
pub struct CopilotClient {
    settings: CopilotSettings,
}

impl CopilotClient {
    pub fn new(settings: CopilotSettings) -> Self {
        Self { settings }
    }

    fn resolve_bin() -> String {
        std::env::var("SIM_FLOW_COPILOT_BIN").unwrap_or_else(|_| DEFAULT_BIN.to_string())
    }

    fn build_prompt(invocation: &Invocation) -> String {
        format!(
            "{}\n\n---\n\n{}",
            invocation.instructions, invocation.prompt
        )
    }
}

impl Client for CopilotClient {
    fn name(&self) -> &'static str {
        "copilot"
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
                cmd.arg("--allow-all-tools");
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

impl CopilotClient {
    /// Test hook: same logic as `Client::invoke` but accepts an
    /// explicit binary.
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
                cmd.arg("--allow-all-tools");
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
    fn name_returns_copilot() {
        let client = CopilotClient::new(CopilotSettings::default());
        assert_eq!(client.name(), "copilot");
    }

    #[test]
    fn build_prompt_concatenates_with_separator() {
        let inv = invocation(SessionMode::OneShot, "do work", "rules");
        let out = CopilotClient::build_prompt(&inv);
        assert_eq!(out, "rules\n\n---\n\ndo work");
    }

    #[test]
    fn invoke_oneshot_always_passes_allow_all_tools() {
        let client = CopilotClient::new(CopilotSettings::default());
        let inv = invocation(SessionMode::OneShot, "hello", "rules");
        let session = client
            .invoke_with_bin(&inv, "/bin/echo")
            .expect("echo runs");
        assert!(session.success());
        assert!(
            session.stdout.contains("--allow-all-tools"),
            "Copilot always passes --allow-all-tools; got {:?}",
            session.stdout
        );
    }

    #[test]
    fn invoke_oneshot_forwards_model_flag() {
        let settings = CopilotSettings {
            model: Some("gpt-4o".into()),
            ..CopilotSettings::default()
        };
        let client = CopilotClient::new(settings);
        let inv = invocation(SessionMode::OneShot, "x", "y");
        let session = client
            .invoke_with_bin(&inv, "/bin/echo")
            .expect("echo runs");
        assert!(session.stdout.contains("--model") && session.stdout.contains("gpt-4o"));
    }

    #[test]
    fn invoke_with_missing_binary_returns_io_error() {
        let client = CopilotClient::new(CopilotSettings::default());
        let inv = invocation(SessionMode::OneShot, "x", "y");
        let result = client.invoke_with_bin(&inv, "/no/such/binary/here-on-purpose-3f8a");
        assert!(matches!(result, Err(Error::Io { .. })));
    }
}
