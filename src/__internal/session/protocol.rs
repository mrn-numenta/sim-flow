//! Wire-format types for the sim-flow session protocol.
//!
//! This module is the source of truth for what flows over stdio
//! between the orchestrator and a host (VS Code extension, RustRover
//! plugin, the in-process TerminalHost, etc.). Hosts that aren't
//! Rust generate types from `session-protocol.schema.json` (emitted
//! from the `JsonSchema` derives below).
//!
//! Spec: docs/architecture/ai-flow/07-session-protocol.md.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Protocol revision. Bumped on breaking changes (removed events,
/// changed field semantics). Adding new optional fields or new event
/// variants is non-breaking and tolerated by older hosts.
pub const PROTOCOL_VERSION: &str = "1";

/// Event direction: orchestrator -> host.
///
/// Serialized as a tagged JSON object with `event` carrying the
/// variant name and the payload fields flattened next to it. We use
/// kebab-case for the tag to match the JSON convention in the
/// extension's existing types.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    /// Reply to the host's `Hello`. Carries the resolved step
    /// descriptor so the host knows what kind of session it is
    /// hosting and can render header / phase information.
    HelloAck {
        protocol_version: String,
        sim_flow_version: String,
        session: SessionTag,
        step_descriptor: StepDescriptorOut,
    },
    /// A chunk of assistant text to render in the chat UI. `final` is
    /// true on the last chunk of a single turn.
    AssistantText { text: String, final_chunk: bool },
    /// Pause the orchestrator and wait for a `UserMessage` on stdin.
    /// Hosts use the optional hints to focus or label the input area.
    RequestUserInput {
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
    },
    /// Ask the host to run an LLM call. Host streams the response
    /// back as `LlmChunk` / `LlmEnd` host events tagged with the same
    /// `request_id`.
    RequestLlmResponse {
        request_id: String,
        backend: String,
        model: Option<String>,
        messages: Vec<LlmMessage>,
        /// Tool catalog for backends that support native tool-use.
        /// Empty in M2; populated in M3.
        #[serde(default)]
        tools: Vec<LlmTool>,
    },
    /// The orchestrator wrote a project file. Informational only;
    /// the host displays "wrote spec.md (123 bytes)" or similar.
    ArtifactWritten { path: String, bytes: u64 },
    /// The orchestrator executed a tool. Informational; host renders
    /// "reading src/model/lib.rs..." etc. Phase 9 M3 wires real tool
    /// invocations; M2 emits this only for artifact writes.
    ToolInvoked {
        name: String,
        args_summary: String,
        status: String,
        duration_ms: u64,
    },
    /// Session moved to a new phase. Phase 9 M3 introduces the
    /// build/test/coverage iteration loop; M2 emits a single
    /// `chat` phase for the duration of the session.
    PhaseChanged { phase: String },
    /// Output from a build or test runner during a code-step phase.
    /// Empty in M2; present in the schema for forward compatibility.
    BuildOutput {
        command: String,
        stdout_tail: String,
        stderr_tail: String,
        exit_code: i32,
    },
    /// Result of a gate evaluation. Emitted opportunistically (e.g.
    /// after each turn that wrote artifacts) and definitively at
    /// session end. Emitting `clean: true` does not by itself
    /// advance state - that happens via `StateAdvanced` after
    /// successful gate + mark_passed.
    GateResult {
        step: String,
        clean: bool,
        failures: Vec<GateFailureOut>,
    },
    /// The orchestrator advanced `current_step` from one step to
    /// another. Emitted right after `mark_passed` + `save`.
    StateAdvanced { from: String, to: Option<String> },
    /// Suggested next action for the host to render as a quick-pick
    /// button. Optional capability; hosts that don't advertise
    /// `followups` will not receive these.
    Followup { label: String, action: String },
    /// Non-fatal diagnostic for the host to display.
    Diagnostic {
        level: DiagnosticLevel,
        message: String,
    },
    /// Session finished. `reason` is `completed`, `cancelled`, or
    /// `error`; `message` is an optional human-readable detail.
    SessionEnd {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// The orchestrator's step-axis mode flag changed. Emitted on
    /// every flip — by user-issued `SetStepMode`, by the
    /// cap-exceeded "drop to interactive" path, or by any future
    /// internal trigger. The dashboard listens and reflects the
    /// current mode in its toggle UI so the visible state always
    /// matches the orchestrator's truth.
    StepModeChanged { mode: StepMode },
    /// A sub-session (work or critique) is starting. Emitted by the
    /// run loop just before `run_session` begins handling the
    /// sub-session. The dashboard uses this to disable per-step
    /// buttons while the orchestrator is busy — the pair
    /// `SubSessionStarted` / `SubSessionEnded` brackets a contiguous
    /// span of LLM streaming + tool calls during which dashboard
    /// commands cannot be dispatched.
    SubSessionStarted { step: String, kind: SessionKindOut },
    /// A sub-session ended. Emitted by the run loop right after
    /// `run_session` returns (success, error, or cancellation). The
    /// orchestrator typically parks at the next decision point in
    /// manual mode, or proceeds to the next sub-session in auto.
    /// `outcome` is `"completed"`, `"cancelled"`, or `"error"`.
    SubSessionEnded {
        step: String,
        kind: SessionKindOut,
        outcome: String,
    },
}

/// Event direction: host -> orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum HostEvent {
    /// Initial handshake. Host announces its protocol version and
    /// optional capabilities.
    Hello {
        protocol_version: String,
        host: HostInfo,
        #[serde(default)]
        capabilities: Vec<String>,
    },
    /// User typed (or pasted) the next message in the chat.
    UserMessage { text: String },
    /// Streaming LLM response chunk fulfilling a prior
    /// `RequestLlmResponse`.
    LlmChunk { request_id: String, text: String },
    /// LLM response finished.
    LlmEnd {
        request_id: String,
        #[serde(default)]
        stop_reason: Option<String>,
    },
    /// LLM dispatch failed.
    LlmError {
        request_id: String,
        kind: String,
        message: String,
    },
    /// User clicked a `Followup` quick-action.
    FollowupSelected { action: String },
    /// User requested cancellation. The orchestrator stops, evaluates
    /// the gate one last time (informational), and emits SessionEnd.
    Cancel,
    /// Manual-mode command: run a step's work or critique sub-session.
    /// Rejected (with a Diagnostic) when the orchestrator is in auto
    /// mode — the auto loop owns step execution there.
    RunStep { step: String, kind: SessionKindOut },
    /// Manual-mode command: run a step's critique sub-session. Alias
    /// for `RunStep { kind: critique }` for symmetry with the
    /// dashboard buttons.
    RunCritique { step: String },
    /// Manual-mode command: evaluate the gate for a step and emit a
    /// `GateResult` event. Does NOT advance — see `Advance`.
    RunGate { step: String },
    /// Manual-mode command: gate-check + git commit + mark passed +
    /// bump `current_step`. Emits `GateResult` and `StateAdvanced`.
    /// If the gate is unclean, emits a Diagnostic and does not
    /// advance. Critique-clean alone never auto-advances; the user
    /// must issue this command.
    Advance { step: String },
    /// Manual-mode command: clear a step's gate state and downstream
    /// gates (matching the existing `sim-flow reset` CLI semantics).
    Reset { step: String },
    /// Flip the orchestrator's step-axis mode flag. Takes effect at
    /// the next decision point in the run loop; never interrupts an
    /// in-flight sub-session. Auto → manual: current sub-session
    /// finishes, loop parks. Manual → auto: orchestrator stays parked
    /// until the user's next `RunStep` / `Advance`; after that
    /// command's sub-session, the loop sees `auto` and continues
    /// iterating from the now-current step.
    SetStepMode { mode: StepMode },
    /// Tear the orchestrator down. Cancels any in-flight sub-session
    /// at the next safe boundary, emits `SessionEnd`, and exits.
    Shutdown,
}

/// Step-axis mode of the orchestrator. Wire-stable enum serialized
/// as `"auto"` / `"manual"`. See `docs/brainstorming/manual-step-mode.md`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StepMode {
    /// Orchestrator walks `current_step` to end of flow without user
    /// input.
    Auto,
    /// Orchestrator parks after the hello handshake and dispatches
    /// sub-sessions only in response to manual-mode host commands.
    Manual,
}

/// Mirror of `client::SessionKind` exposed in the protocol. Kept
/// independent of the internal type so the wire format stays stable
/// even if the internal representation changes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionKindOut {
    Work,
    Critique,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionTag {
    pub step: String,
    pub kind: SessionKindOut,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HostInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LlmMessage {
    pub role: LlmRole,
    pub content: String,
    /// Optional binary attachments associated with this message.
    /// Most messages have none; tool-output user messages may carry
    /// image bytes that the agent should treat as inline content
    /// (multimodal). The host (extension) is responsible for forwarding
    /// these to the underlying LLM via whatever multimodal API the
    /// backend supports; if the backend can't accept images the
    /// attachments are dropped and a Diagnostic is emitted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<LlmAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LlmAttachment {
    /// MIME type, e.g. "image/jpeg" or "image/png".
    pub mime: String,
    /// Base64-encoded payload bytes. Hosts decode and forward to the
    /// LLM. Base64 is used because the protocol is JSONL.
    pub data: String,
    /// Project-relative source path, surfaced for tracing in logs.
    /// Optional because not every attachment originates from a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LlmRole {
    System,
    User,
    Assistant,
}

/// Tool catalog descriptor for a single advertised tool. Phase 9 M3
/// fills the `args_schema` with real JSON Schema for arg shapes.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LlmTool {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool's argument object.
    pub args_schema: serde_json::Value,
}

/// Step descriptor sent in `HelloAck`. Same data as `sim-flow
/// describe`'s output, but using protocol-stable field names.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StepDescriptorOut {
    pub step: String,
    pub kind: SessionKindOut,
    pub flow: String,
    pub prerequisite: Option<String>,
    pub instruction_path: String,
    pub work_artifacts: Vec<String>,
    pub predecessor_inputs: Vec<String>,
    pub per_candidate: bool,
    /// Per-step phases. M2 uses `["chat"]`; M3 expands for code
    /// steps to e.g. `["author", "build", "test", "coverage"]`.
    pub phases: Vec<String>,
    /// Tool catalog the orchestrator will advertise to the LLM.
    /// Empty in M2; populated by step + kind in M3.
    #[serde(default)]
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GateFailureOut {
    pub description: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    Info,
    Warning,
    Error,
}

/// Generate the JSON Schema for the protocol surface (both
/// directions). Used by the schema-export binary to refresh the
/// committed schema file at
/// `tools/sim-flow/docs/flow/session-protocol.schema.json`.
pub fn protocol_schema() -> serde_json::Value {
    let event = schemars::schema_for!(Event);
    let host_event = schemars::schema_for!(HostEvent);
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "sim-flow session protocol",
        "description": format!(
            "Versioned wire format for sim-flow session events. \
             protocolVersion = {}.",
            PROTOCOL_VERSION
        ),
        "oneOf": [
            { "title": "Event (orchestrator -> host)", "schema": event },
            { "title": "HostEvent (host -> orchestrator)", "schema": host_event },
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_text_round_trips() {
        let e = Event::AssistantText {
            text: "hello".into(),
            final_chunk: true,
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"event\":\"assistant-text\""));
        assert!(s.contains("\"text\":\"hello\""));
        let parsed: Event = serde_json::from_str(&s).unwrap();
        match parsed {
            Event::AssistantText { text, final_chunk } => {
                assert_eq!(text, "hello");
                assert!(final_chunk);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn host_event_user_message_round_trips() {
        let h = HostEvent::UserMessage { text: "go".into() };
        let s = serde_json::to_string(&h).unwrap();
        let parsed: HostEvent = serde_json::from_str(&s).unwrap();
        match parsed {
            HostEvent::UserMessage { text } => assert_eq!(text, "go"),
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn cancel_serializes_with_no_payload() {
        let h = HostEvent::Cancel;
        let s = serde_json::to_string(&h).unwrap();
        assert_eq!(s, "{\"event\":\"cancel\"}");
    }

    #[test]
    fn protocol_schema_compiles_to_valid_json() {
        let schema = protocol_schema();
        // Sanity: round-trips and has both top-level oneOf entries.
        let s = serde_json::to_string(&schema).unwrap();
        assert!(s.contains("Event (orchestrator -> host)"));
        assert!(s.contains("HostEvent (host -> orchestrator)"));
    }
}
