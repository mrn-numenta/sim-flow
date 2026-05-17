//! `CodexAgent` - subprocess wrapper for OpenAI's `codex` CLI.
//!
//! Codex's non-interactive surface is `codex exec <prompt>`, which
//! runs the agent against a one-shot prompt and prints its final
//! response to stdout. Multi-turn fidelity is best-effort: each
//! orchestrator turn re-runs `codex exec` with the entire rendered
//! history baked into a single prompt with role markers. Codex's
//! agent loop runs again per turn rather than resuming, so this is
//! correct but slower than `claude -p`.
//!
//! Marked experimental: codex CLI surface is evolving; if a flag
//! disappears or changes the agent will fail with the helpful
//! "exited N: <stderr>" error and the user can switch backends.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use super::cancel::{wait_with_cancel, wait_with_cancel_streaming};
use super::{AdvertisedToolCall, CliAgent, LlmCallMetrics, StreamingChunk, ToolAdvertise};
use crate::session::protocol::{LlmMessage, LlmRole};
use crate::{Error, Result};

pub struct CodexAgent {
    model: Option<String>,
    cancel_flag: Option<Arc<AtomicBool>>,
}

impl CodexAgent {
    pub fn new(model: Option<String>) -> Self {
        Self::new_with_cancel(model, None)
    }

    pub fn new_with_cancel(model: Option<String>, cancel_flag: Option<Arc<AtomicBool>>) -> Self {
        Self { model, cancel_flag }
    }

    /// Render the full message stack into a single `codex exec`
    /// prompt. Same role-tagged shape as `ClaudeAgent` so the model
    /// has clear turn boundaries.
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

impl CliAgent for CodexAgent {
    fn name(&self) -> &str {
        "codex"
    }

    fn dispatch(&self, messages: &[LlmMessage]) -> Result<(String, LlmCallMetrics)> {
        let started = std::time::Instant::now();
        let prompt = Self::render_prompt(messages);
        let mut cmd = Command::new("codex");
        cmd.arg("exec");
        if let Some(model) = &self.model {
            cmd.arg("--model").arg(model);
        }
        // Read the prompt from stdin so we don't hit argv length
        // limits on long histories.
        cmd.arg("-");
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|err| {
            Error::Client(format!(
                "codex CLI not found or failed to spawn: {err}. Install OpenAI Codex CLI (https://github.com/openai/codex) or pick a different `--llm-backend`."
            ))
        })?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(prompt.as_bytes()).map_err(|err| {
                Error::Client(format!("codex: failed to write prompt to stdin: {err}"))
            })?;
        }
        drop(child.stdin.take());

        // Cancel-aware wait: polls the shared flag on a 50ms cadence
        // and SIGTERMs the child if the dashboard pushes Stop via
        // the control socket while we're blocked here.
        let output = wait_with_cancel(child, self.cancel_flag.clone())?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Client(format!(
                "codex exited {}: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim(),
            )));
        }
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let metrics = LlmCallMetrics {
            tokens_in: None,
            tokens_out: None,
            wall_ms: started.elapsed().as_millis() as u64,
            cancelled: false,
        };
        Ok((text, metrics))
    }

    fn dispatch_streaming(
        &self,
        messages: &[LlmMessage],
        _tools: &[ToolAdvertise],
        on_chunk: &mut dyn FnMut(StreamingChunk),
    ) -> Result<(String, Vec<AdvertisedToolCall>, LlmCallMetrics)> {
        let started = std::time::Instant::now();
        let prompt = Self::render_prompt(messages);
        let mut cmd = Command::new("codex");
        cmd.arg("exec");
        if let Some(model) = &self.model {
            cmd.arg("--model").arg(model);
        }
        cmd.arg("-");
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|err| {
            Error::Client(format!(
                "codex CLI not found or failed to spawn: {err}. Install OpenAI Codex CLI (https://github.com/openai/codex) or pick a different `--llm-backend`."
            ))
        })?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(prompt.as_bytes()).map_err(|err| {
                Error::Client(format!("codex: failed to write prompt to stdin: {err}"))
            })?;
        }
        drop(child.stdin.take());

        let mut text = String::new();
        let (output, cancelled) = {
            let mut on_stream = |s: &str| {
                text.push_str(s);
                on_chunk(StreamingChunk::Text(s.to_string()));
            };
            wait_with_cancel_streaming(child, self.cancel_flag.clone(), &mut on_stream)?
        };
        if !cancelled && !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Client(format!(
                "codex exited {}: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim(),
            )));
        }
        let metrics = LlmCallMetrics {
            tokens_in: None,
            tokens_out: None,
            wall_ms: started.elapsed().as_millis() as u64,
            cancelled,
        };
        Ok((text, Vec::new(), metrics))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_prompt_preserves_role_order() {
        let prompt = CodexAgent::render_prompt(&[
            LlmMessage {
                role: LlmRole::System,
                content: "rules".into(),
                attachments: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning: None,
            },
            LlmMessage {
                role: LlmRole::User,
                content: "go".into(),
                attachments: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning: None,
            },
        ]);
        assert!(prompt.starts_with("[SYSTEM]\nrules"));
        assert!(prompt.ends_with("[USER]\ngo"));
    }
}
