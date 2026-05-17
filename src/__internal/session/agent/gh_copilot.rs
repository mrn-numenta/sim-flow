//! `GhCopilotAgent` - subprocess wrapper for the `gh copilot` CLI.
//!
//! GitHub's Copilot CLI is primarily designed for shell-command
//! suggestions and explanations, not general chat. This agent is a
//! best-effort adapter: it pipes the rendered message history into
//! `gh copilot suggest -t shell` (the only widely-available
//! non-interactive subcommand) and returns whatever Copilot writes
//! back. Output quality for spec / critique work is highly
//! dependent on the Copilot CLI version.
//!
//! Marked experimental. For Copilot-backed sessions in VS Code,
//! prefer the `vscode` LLM source - it goes through the Language
//! Model API and gives you full Copilot Chat capabilities.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use super::cancel::wait_with_cancel;
use super::{CliAgent, LlmCallMetrics};
use crate::session::protocol::{LlmMessage, LlmRole};
use crate::{Error, Result};

pub struct GhCopilotAgent {
    cancel_flag: Option<Arc<AtomicBool>>,
}

impl GhCopilotAgent {
    pub fn new() -> Self {
        Self::new_with_cancel(None)
    }

    pub fn new_with_cancel(cancel_flag: Option<Arc<AtomicBool>>) -> Self {
        Self { cancel_flag }
    }

    fn render_prompt(messages: &[LlmMessage]) -> String {
        let mut out = String::new();
        for m in messages {
            let tag = match m.role {
                LlmRole::System => "[SYSTEM]",
                LlmRole::User => "[USER]",
                LlmRole::Assistant => "[ASSISTANT]",
                LlmRole::Tool => "[TOOL-RESULT]",
            };
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(tag);
            out.push('\n');
            out.push_str(&m.content);
        }
        out
    }
}

impl Default for GhCopilotAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl CliAgent for GhCopilotAgent {
    fn name(&self) -> &str {
        "gh-copilot"
    }

    fn dispatch(&self, messages: &[LlmMessage]) -> Result<(String, LlmCallMetrics)> {
        let started = std::time::Instant::now();
        let prompt = Self::render_prompt(messages);
        let mut cmd = Command::new("gh");
        cmd.args(["copilot", "suggest", "-t", "shell", "--no-spinner"]);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|err| {
            Error::Client(format!(
                "`gh copilot` not found or failed to spawn: {err}. Install GitHub CLI + the Copilot extension (`gh extension install github/gh-copilot`), or pick a different `--llm-backend`."
            ))
        })?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(prompt.as_bytes()).map_err(|err| {
                Error::Client(format!(
                    "gh-copilot: failed to write prompt to stdin: {err}"
                ))
            })?;
        }
        drop(child.stdin.take());

        // Cancel-aware wait; polls the shared cancel flag and
        // SIGTERMs the `gh` subprocess on Stop click.
        let output = wait_with_cancel(child, self.cancel_flag.clone())?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Client(format!(
                "`gh copilot` exited {}: {}\n\nNote: gh-copilot is best-effort outside VS Code; for full Copilot Chat use the `vscode` LLM source.",
                output.status.code().unwrap_or(-1),
                stderr.trim(),
            )));
        }
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let metrics = LlmCallMetrics {
            tokens_in: None,
            tokens_out: None,
            wall_ms: started.elapsed().as_millis() as u64,
        };
        Ok((text, metrics))
    }
}
