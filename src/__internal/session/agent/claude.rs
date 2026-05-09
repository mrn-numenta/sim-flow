//! `ClaudeAgent` - subprocess wrapper for the `claude` CLI.
//!
//! Uses `claude -p <prompt>` (one-shot, prints to stdout) so we get
//! a clean non-interactive path. The orchestrator's full message
//! history is rendered into a single prompt with role markers per
//! turn; Claude then produces the next assistant turn.
//!
//! Multi-turn fidelity is good enough for spec/critique sessions
//! and the early stages of code-authoring. Phase 9 follow-ups can
//! switch to a session-resume mode if `claude` exposes one.

use std::io::Write;
use std::process::{Command, Stdio};

use super::{
    CLAUDE_CLI_RUNTIME, CliAgent, LlmCallMetrics, RuntimeCapabilityProfile,
    apply_reasoning_history_policy, resolve_model_family,
};
use crate::session::protocol::{LlmMessage, LlmRole};
use crate::{Error, Result};

pub struct ClaudeAgent {
    model: Option<String>,
    model_family_id: Option<String>,
    runtime_profile: RuntimeCapabilityProfile,
}

impl ClaudeAgent {
    pub fn new(model: Option<String>, model_family_id: Option<String>) -> Self {
        Self {
            model,
            model_family_id,
            runtime_profile: CLAUDE_CLI_RUNTIME,
        }
    }

    #[cfg(test)]
    pub(crate) fn runtime_profile(&self) -> RuntimeCapabilityProfile {
        self.runtime_profile
    }

    /// Render the message stack into a single prompt string for
    /// `claude -p`. Format:
    ///
    /// ```text
    /// [SYSTEM]
    /// <system content>
    ///
    /// [USER]
    /// <user message>
    ///
    /// [ASSISTANT]
    /// <prior assistant turn>
    ///
    /// [USER]
    /// <next user message>
    /// ```
    ///
    /// The trailing `[USER]` cues `claude` that the next turn is
    /// from us; Claude responds in its assistant voice. The string
    /// is passed via stdin (`claude -p -`) so we don't hit argv
    /// length limits on long histories.
    fn render_prompt(
        messages: &[LlmMessage],
        model_family_id: Option<&str>,
        model: Option<&str>,
    ) -> String {
        let family = resolve_model_family(model_family_id, model);
        let prepared = apply_reasoning_history_policy(messages, family);
        let mut out = String::new();
        for m in &prepared {
            let tag = match m.role {
                LlmRole::System => "[SYSTEM]",
                LlmRole::User => "[USER]",
                LlmRole::Assistant => "[ASSISTANT]",
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

impl CliAgent for ClaudeAgent {
    fn name(&self) -> &str {
        "claude"
    }

    fn dispatch(&self, messages: &[LlmMessage]) -> Result<(String, LlmCallMetrics)> {
        debug_assert_eq!(self.runtime_profile.request_format, "subprocess_prompt");
        let started = std::time::Instant::now();
        let prompt = Self::render_prompt(
            messages,
            self.model_family_id.as_deref(),
            self.model.as_deref(),
        );
        let mut cmd = Command::new("claude");
        cmd.arg("-p");
        if let Some(model) = &self.model {
            cmd.arg("--model").arg(normalize_model_for_cli(model));
        }
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|err| {
            Error::Client(format!(
                "claude CLI not found or failed to spawn: {err}. Install Claude Code (https://docs.claude.com/claude-code) or pick a different `--llm-backend`."
            ))
        })?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(prompt.as_bytes()).map_err(|err| {
                Error::Client(format!(
                    "claude CLI: failed to write prompt to stdin: {err}"
                ))
            })?;
        }
        // Drop the stdin handle so claude's read_to_end completes.
        drop(child.stdin.take());

        let output = child
            .wait_with_output()
            .map_err(|err| Error::Client(format!("claude CLI: wait failed: {err}")))?;
        if !output.status.success() {
            // Include BOTH stdout and stderr because `claude` writes
            // some failures (e.g. "unknown model") to stdout and the
            // user-facing error would otherwise be empty.
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let pieces: Vec<String> = [stdout.trim(), stderr.trim()]
                .iter()
                .filter(|p| !p.is_empty())
                .map(|p| (*p).to_string())
                .collect();
            let detail = if pieces.is_empty() {
                "(no output)".into()
            } else {
                pieces.join(" | ")
            };
            return Err(Error::Client(format!(
                "claude CLI exited {}: {detail}",
                output.status.code().unwrap_or(-1),
            )));
        }
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        // CLI doesn't surface token usage; only wall time is meaningful.
        let metrics = LlmCallMetrics {
            tokens_in: None,
            tokens_out: None,
            wall_ms: started.elapsed().as_millis() as u64,
        };
        Ok((stdout, metrics))
    }
}

/// Normalize a configured model string into the form the `claude` CLI's
/// `--model` flag accepts. Handles two foot-guns the dashboard exposes:
///
/// - The `vscode` LM-source dropdown emits `vendor/family` strings like
///   `claude-code/claude-sonnet-4.6` so it can disambiguate Copilot's
///   `claude-sonnet-4.6` from Claude Code's. The CLI doesn't understand
///   that prefix; we strip it.
/// - The LM-API surface uses dotted ids (`claude-sonnet-4.6`) while the
///   CLI expects dashed (`claude-sonnet-4-6`). We convert dots between
///   ASCII digits to dashes.
///
/// CLI-native aliases (`sonnet`, `opus`, `haiku`) and already-dashed ids
/// pass through unchanged.
pub(crate) fn normalize_model_for_cli(raw: &str) -> String {
    let stripped = raw
        .strip_prefix("claude-code/")
        .or_else(|| raw.strip_prefix("anthropic/"))
        .unwrap_or(raw);
    let mut out = String::with_capacity(stripped.len());
    let chars: Vec<char> = stripped.chars().collect();
    for (i, ch) in chars.iter().enumerate() {
        let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
        let next_digit = i + 1 < chars.len() && chars[i + 1].is_ascii_digit();
        if *ch == '.' && prev_digit && next_digit {
            out.push('-');
        } else {
            out.push(*ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> LlmMessage {
        LlmMessage {
            role: LlmRole::User,
            content: text.into(),
            attachments: Vec::new(),
        }
    }
    fn system(text: &str) -> LlmMessage {
        LlmMessage {
            role: LlmRole::System,
            content: text.into(),
            attachments: Vec::new(),
        }
    }
    fn assistant(text: &str) -> LlmMessage {
        LlmMessage {
            role: LlmRole::Assistant,
            content: text.into(),
            attachments: Vec::new(),
        }
    }

    #[test]
    fn render_prompt_orders_messages_with_role_tags() {
        let prompt = ClaudeAgent::render_prompt(
            &[
                system("rules"),
                user("hi"),
                assistant("hello"),
                user("more"),
            ],
            None,
            None,
        );
        assert!(prompt.starts_with("[SYSTEM]\nrules"));
        assert!(prompt.contains("[USER]\nhi"));
        assert!(prompt.contains("[ASSISTANT]\nhello"));
        assert!(prompt.ends_with("[USER]\nmore"));
    }

    #[test]
    fn render_prompt_handles_empty_message_list() {
        assert_eq!(ClaudeAgent::render_prompt(&[], None, None), "");
    }

    #[test]
    fn normalize_model_for_cli_strips_vendor_prefix() {
        // The dashboard's vscode-source dropdown writes `vendor/family`
        // strings like `claude-code/claude-sonnet-4.6` so the LM API
        // can disambiguate vendors. The CLI doesn't understand the
        // prefix, so we strip it before passing through.
        assert_eq!(
            normalize_model_for_cli("claude-code/claude-sonnet-4.6"),
            "claude-sonnet-4-6",
        );
        assert_eq!(
            normalize_model_for_cli("anthropic/claude-opus-4.7"),
            "claude-opus-4-7",
        );
    }

    #[test]
    fn normalize_model_for_cli_converts_dotted_ids_to_dashed() {
        // LM API surfaces `claude-sonnet-4.6`; the CLI wants
        // `claude-sonnet-4-6`. Only digit.digit dots get rewritten so
        // we don't mangle hostnames or future dotted alias schemes.
        assert_eq!(
            normalize_model_for_cli("claude-sonnet-4.6"),
            "claude-sonnet-4-6",
        );
        assert_eq!(
            normalize_model_for_cli("claude-haiku-4.5"),
            "claude-haiku-4-5",
        );
    }

    #[test]
    fn normalize_model_for_cli_passes_through_aliases_and_dashed_ids() {
        // CLI-native forms are unchanged.
        for input in [
            "sonnet",
            "opus",
            "haiku",
            "claude-sonnet-4-6",
            "claude-opus-4-7",
            "claude-3-5-sonnet-20241022",
        ] {
            assert_eq!(normalize_model_for_cli(input), input, "input={input}");
        }
    }

    #[test]
    fn normalize_model_for_cli_leaves_non_digit_dots_alone() {
        // Defensive: only rewrite a dot when both sides are digits.
        // A future model id like `claude.something` should round-trip
        // unchanged so we don't accidentally break it.
        assert_eq!(normalize_model_for_cli("v1.0-rc"), "v1-0-rc");
        assert_eq!(normalize_model_for_cli("foo.bar"), "foo.bar");
    }
}
