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
pub mod capture_host;
pub mod compaction;
pub mod control_socket;
pub mod debug_log;
pub mod event_tap;
pub mod host;
pub mod llm_adapter;
pub mod llm_metrics;
pub mod lsp;
pub mod orchestrator;
pub mod pdfium_loader;
pub mod presenter;
pub mod protocol;
pub mod runners;
pub mod signal_cleanup;
pub mod socket_host;
pub mod spec_ingest;
pub mod spec_md;
pub mod stderr_presenter;
pub mod tool_timings;
pub mod tools;

pub use auto::{AutoOptions, run_auto};
pub use auto_interactive::{AutoInteractiveOptions, run_auto_interactive};
pub use capture_host::{CapturePresenter, JsonlCapture};
pub use control_socket::{ControlCommand, ControlEvent, ControlListener, default_socket_path};
pub use spec_ingest::{SpecIngestSummary, ingest_spec_file};

pub use agent::{
    AdvertisedToolCall, AgentAdaptationSummary, AgentConfig, ClaudeAgent, CliAgent, CodexAgent,
    GhCopilotAgent, KNOWN_AGENTS, LlmCallMetrics, MockAgent, OllamaAgent, OpenAiCompatAgent,
    ToolAdvertise, build_cli_agent,
};
pub use event_tap::{
    EventTap, TappedPresenter, WatchRegistration, list_registrations as list_watch_registrations,
};
pub use host::{JsonlHost, TestHost};
pub use llm_adapter::LlmAdapter;
pub use orchestrator::{OrchestratorOptions, run_session};
pub use presenter::Presenter;
pub use protocol::{
    DiagnosticLevel, Event, GateFailureOut, HostEvent, HostInfo, LlmMessage, LlmRole, LlmTool,
    PROTOCOL_VERSION, SessionKindOut, SessionTag, StepDescriptorOut,
};
pub use socket_host::SocketPresenter;
pub use stderr_presenter::StderrPresenter;
