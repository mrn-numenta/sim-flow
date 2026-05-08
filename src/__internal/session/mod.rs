//! Session orchestrator.
//!
//! `sim-flow session <step>.<kind>` runs an interactive work or
//! critique session under orchestrator control. The orchestrator
//! loads instructions, drives the LLM turn loop, parses + writes
//! artifacts, and emits gate / advance events. The user-facing
//! surface is supplied by an implementation of the [`Host`] trait;
//! the protocol between them is documented in
//! `docs/architecture/ai-flow/07-session-protocol.md`.

pub mod agent;
pub mod auto;
pub mod auto_interactive;
pub mod control_socket;
pub mod debug_log;
pub mod event_tap;
pub mod host;
pub mod orchestrator;
pub mod pdfium_loader;
pub mod protocol;
pub mod runners;
pub mod signal_cleanup;
pub mod socket_host;
pub mod spec_ingest;
pub mod tools;

pub use auto::{AutoOptions, run_auto};
pub use auto_interactive::{AutoInteractiveOptions, run_auto_interactive};
pub use control_socket::{ControlCommand, ControlEvent, ControlListener, default_socket_path};
pub use spec_ingest::{SpecIngestSummary, ingest_spec_file};

pub use agent::{
    AgentConfig, ClaudeAgent, CliAgent, CodexAgent, GhCopilotAgent, KNOWN_AGENTS, LlmCallMetrics,
    MockAgent, OllamaAgent, OpenAiCompatAgent, build_cli_agent,
};
pub use event_tap::{
    EventTap, TappedHost, WatchRegistration, list_registrations as list_watch_registrations,
};
pub use host::{Host, JsonlHost, TerminalHost, TestHost};
pub use orchestrator::{OrchestratorOptions, run_session};
pub use protocol::{
    DiagnosticLevel, Event, GateFailureOut, HostEvent, HostInfo, LlmMessage, LlmRole, LlmTool,
    PROTOCOL_VERSION, SessionKindOut, SessionTag, StepDescriptorOut,
};
pub use socket_host::SocketHost;
