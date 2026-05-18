//! `ask_user(question, context?, kind?, choices?, default?,
//! record_as?, thread_id?)` -- the user-interaction tool that
//! suspends the LLM turn at a tool-call boundary and resumes with
//! the user's reply on the next turn (Architecture §4.5).
//!
//! Unlike the retrieval tools, the dispatch path here doesn't
//! return a synchronous answer. Instead it calls
//! `AskUserRuntime::suspend_for_user_ask`, which stashes the call,
//! persists checkpoints, and signals the orchestrator's outer
//! dispatch loop to exit the LLM turn cleanly. The
//! `ToolResult::display` carries a sentinel marker
//! (`ASK_USER_SUSPENDED_MARKER`) so the dispatch loop can detect
//! the suspension without having to extend the universal `Tool`
//! contract.
//!
//! `step_mode_before` and the auto→manual flip side-effect are
//! handled by an optional `flip` callback the orchestrator wires in;
//! tests pass a noop sink.

use std::sync::Arc;

use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolResult};
use crate::__internal::session::ask_user::{
    AskUserKind, AskUserRuntime, PendingUserAsk, RecordAs, flip_step_mode_for_ask_user,
    mode_flip::NoopSink,
};
use crate::__internal::session::protocol::StepMode;
use crate::Result;

/// Stable marker the orchestrator's dispatch loop scans for to detect
/// "this tool call suspended the LLM turn". The marker is followed by
/// the JSON-shaped pending-call payload so the orchestrator can emit
/// `RequestUserInput` directly off the tool result.
pub const ASK_USER_SUSPENDED_MARKER: &str = "ASK_USER_SUSPENDED::";

/// Soft turn-cap warning per Architecture §4.5 chaining section.
pub const ASK_USER_TURN_CAP: u32 = 5;

pub struct AskUserTool {
    runtime: Arc<AskUserRuntime>,
}

impl AskUserTool {
    pub fn new(runtime: Arc<AskUserRuntime>) -> Self {
        Self { runtime }
    }
}

impl Tool for AskUserTool {
    fn name(&self) -> &'static str {
        "ask_user"
    }

    fn description(&self) -> &'static str {
        "Surface a question to the user and pause the LLM turn until their reply lands. Use this as the LAST tool call of a turn when forward progress requires an answer the spec doesn't give you. Do NOT use it for retrievable information (use `api_semantic_search` / `spec_semantic_search` instead). Calling `ask_user` during an automated run flips the run to manual mode for the remainder of the session. Chain follow-up clarifications by re-using the returned `thread_id` and `record_as = \"none\"` on intermediate calls; only the closing call sets the persistable `record_as`."
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["question"],
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to surface to the user. Keep it focused and answerable in one short reply."
                },
                "context": {
                    "type": "string",
                    "description": "Optional. One short paragraph explaining why you're asking and what's blocked."
                },
                "kind": {
                    "type": "string",
                    "enum": ["free-form", "yes-no", "choice", "value"],
                    "default": "free-form"
                },
                "choices": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Required when kind='choice'."
                },
                "default": {
                    "type": "string",
                    "description": "Optional default returned if the user reply is empty."
                },
                "record_as": {
                    "type": "string",
                    "enum": ["open-question", "auto-decision", "none"],
                    "default": "open-question",
                    "description": "How to persist this Q+A in spec.md. Intermediate calls in a chained thread should use 'none'; the closing call sets the resolved record_as."
                },
                "thread_id": {
                    "type": "string",
                    "description": "Optional. Omit on a fresh question; pass the returned thread_id to chain a follow-up clarification."
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &Value) -> Result<ToolResult> {
        // Validate args.
        let question = match args.get("question").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim().to_string(),
            _ => {
                return Ok(ToolResult::err("ask_user: missing or empty `question` arg"));
            }
        };
        let context_str = args
            .get("context")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let kind = match args.get("kind").and_then(|v| v.as_str()) {
            Some(k) => AskUserKind::parse(k).unwrap_or_else(|| {
                tracing::warn!("ask_user: unknown kind `{k}`, defaulting to free-form");
                AskUserKind::FreeForm
            }),
            None => AskUserKind::FreeForm,
        };
        let choices: Vec<String> = args
            .get("choices")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let default_value = args
            .get("default")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let record_as = match args.get("record_as").and_then(|v| v.as_str()) {
            Some(r) => match RecordAs::parse(r) {
                Some(v) => v,
                None => {
                    return Ok(ToolResult::err(format!(
                        "ask_user: unknown record_as `{r}` (allowed: open-question, auto-decision, none)"
                    )));
                }
            },
            None => RecordAs::OpenQuestion,
        };
        if matches!(kind, AskUserKind::Choice) && choices.is_empty() {
            return Ok(ToolResult::err(
                "ask_user: kind=choice requires non-empty `choices`",
            ));
        }
        let thread_id = args
            .get("thread_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();

        // Determine the step-mode flip side-effect (only on the FIRST
        // turn of a thread per milestone 5.6 / Arch §6.5.2). If the
        // thread is already open, we leave the mode alone.
        let mode_before = if thread_id.is_empty() {
            // Fresh thread: perform the flip side-effect.
            let mut sink = NoopSink;
            let _ = flip_step_mode_for_ask_user(ctx.project_dir, &mut sink);
            // `read_current_step_mode` after the flip gives the
            // "after" value; we don't have an easy way to compute
            // "before" here, so the orchestrator's emitter (which
            // observes the StepModeChanged event) is the source of
            // truth for the mode_changed answer field. The tool
            // records what we can.
            StepMode::Manual
        } else {
            // Continuation: don't touch mode.
            StepMode::Manual
        };

        // Determine the tool-call id. Without orchestrator wiring we
        // synthesize one; the orchestrator should overwrite this via
        // the suspended-marker payload before persisting.
        let tool_call_id = args
            .get("__tool_call_id")
            .and_then(|v| v.as_str())
            .unwrap_or("ask_user-call")
            .to_string();

        let pending = PendingUserAsk {
            question,
            context: context_str,
            kind,
            choices: choices.clone(),
            default: default_value,
            record_as,
            tool_call_id,
            triggered_at_ms: 0, // populated by runtime
            step_mode_before: mode_before,
            thread_id,
            thread_turn_index: 0, // populated by runtime for continuations
            step_id: self.runtime.step_id(),
        };

        let outcome = match self.runtime.suspend_for_user_ask(pending) {
            Ok(o) => o,
            Err(e) => {
                return Ok(ToolResult::err(format!("ask_user: {e}")));
            }
        };

        // Soft turn-cap warning per Architecture §4.5: emit a
        // diagnostic-grade marker when the new turn reaches the cap.
        if outcome.pending.thread_turn_index >= ASK_USER_TURN_CAP {
            tracing::warn!(
                target: "sim_flow::ask_user",
                "thread {} exceeded {} turns; consider closing",
                outcome.pending.thread_id,
                ASK_USER_TURN_CAP
            );
        }

        // Construct the suspended payload the orchestrator's dispatch
        // loop scans for.
        let payload = json!({
            "question": outcome.pending.question,
            "context": outcome.pending.context,
            "kind": outcome.pending.kind.as_str(),
            "choices": outcome.pending.choices,
            "default": outcome.pending.default,
            "thread_id": outcome.pending.thread_id,
            "thread_turn_index": outcome.pending.thread_turn_index,
            "fresh_thread": outcome.fresh_thread,
            "tool_call_id": outcome.pending.tool_call_id,
            "turn_cap_warning": outcome.pending.thread_turn_index >= ASK_USER_TURN_CAP,
        });

        // The first-call metric event lives in the runtime's
        // record_turn callsite; the suspend itself emits an
        // "ask_user_suspend" event so we can measure how often the
        // turn parks before the user replies.
        tracing::info!(
            target: "sim_flow::metrics",
            event = "ask_user_suspend",
            step = self.runtime.step_id().as_str(),
            thread_id = outcome.pending.thread_id.as_str(),
            thread_turn_index = outcome.pending.thread_turn_index,
            fresh_thread = outcome.fresh_thread,
            kind = outcome.pending.kind.as_str(),
            record_as = outcome.pending.record_as.as_str(),
        );

        Ok(ToolResult::ok(format!(
            "{ASK_USER_SUSPENDED_MARKER}{payload}"
        )))
    }
}

/// Test whether a `ToolResult::display` is the suspended-marker shape.
pub fn is_suspended_result(display: &str) -> bool {
    display.starts_with(ASK_USER_SUSPENDED_MARKER)
}

/// Parse the JSON payload that follows the suspended marker.
pub fn parse_suspended_payload(display: &str) -> Option<Value> {
    let body = display.strip_prefix(ASK_USER_SUSPENDED_MARKER)?;
    serde_json::from_str(body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(tmp: &tempfile::TempDir, step: &str) -> AskUserTool {
        let rt = Arc::new(AskUserRuntime::new(
            tmp.path().to_path_buf(),
            step.to_string(),
        ));
        AskUserTool::new(rt)
    }

    fn ctx_for<'a>(tmp: &'a tempfile::TempDir) -> ToolContext<'a> {
        ToolContext::new(tmp.path(), None, None, None)
    }

    #[test]
    fn missing_question_arg_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = make_tool(&tmp, "DM0");
        let ctx = ctx_for(&tmp);
        let r = tool.invoke(&ctx, &json!({})).expect("invoke");
        assert!(!r.ok);
        assert!(r.display.contains("missing"));
    }

    #[test]
    fn kind_choice_without_choices_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = make_tool(&tmp, "DM0");
        let ctx = ctx_for(&tmp);
        let r = tool
            .invoke(&ctx, &json!({"question": "x", "kind": "choice"}))
            .expect("invoke");
        assert!(!r.ok);
        assert!(r.display.contains("requires non-empty"));
    }

    #[test]
    fn unknown_record_as_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = make_tool(&tmp, "DM0");
        let ctx = ctx_for(&tmp);
        let r = tool
            .invoke(&ctx, &json!({"question": "x", "record_as": "weird"}))
            .expect("invoke");
        assert!(!r.ok);
        assert!(r.display.contains("unknown record_as"));
    }

    #[test]
    fn fresh_call_emits_suspended_marker_and_suspends_runtime() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = make_tool(&tmp, "DM0");
        let ctx = ctx_for(&tmp);
        let r = tool
            .invoke(&ctx, &json!({"question": "What's the clock freq?"}))
            .expect("invoke");
        assert!(r.ok);
        assert!(is_suspended_result(&r.display));
        let payload = parse_suspended_payload(&r.display).expect("payload parses");
        assert_eq!(payload["question"], "What's the clock freq?");
        assert!(payload["fresh_thread"].as_bool().unwrap());
        assert_eq!(payload["thread_turn_index"], 0);
        assert!(tool.runtime.has_pending());
    }

    #[test]
    fn second_concurrent_ask_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = make_tool(&tmp, "DM0");
        let ctx = ctx_for(&tmp);
        tool.invoke(&ctx, &json!({"question": "a"})).expect("first");
        let r = tool
            .invoke(&ctx, &json!({"question": "b"}))
            .expect("invoke");
        assert!(!r.ok);
        assert!(r.display.contains("already pending"));
    }

    #[test]
    fn auto_mode_flips_to_manual_on_first_turn() {
        use crate::__internal::session::ask_user::mode_flip::{
            read_current_step_mode, write_current_step_mode,
        };
        let tmp = tempfile::tempdir().unwrap();
        write_current_step_mode(tmp.path(), StepMode::Auto).unwrap();
        let tool = make_tool(&tmp, "DM0");
        let ctx = ctx_for(&tmp);
        tool.invoke(&ctx, &json!({"question": "x"}))
            .expect("invoke");
        assert_eq!(read_current_step_mode(tmp.path()), StepMode::Manual);
    }

    #[test]
    fn continuation_call_with_unknown_thread_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = make_tool(&tmp, "DM0");
        let ctx = ctx_for(&tmp);
        let r = tool
            .invoke(&ctx, &json!({"question": "x", "thread_id": "made-up"}))
            .expect("invoke");
        assert!(!r.ok);
        assert!(r.display.contains("unknown thread"));
    }

    #[test]
    fn chained_continuation_picks_up_correct_turn_index() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = make_tool(&tmp, "DM0");
        let ctx = ctx_for(&tmp);
        // Turn 0.
        let r = tool
            .invoke(&ctx, &json!({"question": "q0", "record_as": "none"}))
            .expect("invoke 0");
        let payload = parse_suspended_payload(&r.display).unwrap();
        let tid = payload["thread_id"].as_str().unwrap().to_string();
        tool.runtime
            .resume_from_user_ask("a0", false, false)
            .expect("resume 0");

        // Turn 1.
        let r = tool
            .invoke(
                &ctx,
                &json!({
                    "question": "q1",
                    "record_as": "none",
                    "thread_id": tid,
                }),
            )
            .expect("invoke 1");
        let payload = parse_suspended_payload(&r.display).unwrap();
        assert_eq!(payload["thread_turn_index"], 1);
        assert_eq!(payload["fresh_thread"], false);
    }
}
