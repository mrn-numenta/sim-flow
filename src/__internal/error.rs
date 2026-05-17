//! Crate-wide error and result aliases.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("toml parse error in {path}: {source}")]
    TomlParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("toml serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    /// The session host violated the wire protocol (out-of-order
    /// event, malformed JSON, unsafe path in an artifact / tool call,
    /// I/O on the protocol channel itself).
    #[error("protocol error: {0}")]
    Protocol(String),

    /// Host advertised a `protocolVersion` the orchestrator doesn't
    /// speak. Distinguished so callers can choose to recover by
    /// downgrading or by surfacing a version-skew warning.
    #[error("protocol version mismatch: host={host} orchestrator={orchestrator}")]
    ProtocolVersionMismatch { host: String, orchestrator: String },

    /// The session-host channel reached EOF where data was expected.
    /// `context` names the point in the protocol (e.g.
    /// `"before Hello"`, `"mid-turn"`).
    #[error("session: host closed ({0})")]
    HostClosed(String),

    /// LLM backend / agent client failure (subprocess spawn, PTY I/O,
    /// HTTP transport on managed agents, etc.). Distinct from
    /// `Protocol` so the orchestrator's turn loop can offer
    /// retry / cancel / switch-backend recovery instead of aborting.
    #[error("llm error: {0}")]
    Llm(String),

    /// Catch-all for failures that don't fit a typed cluster yet:
    /// mutex poisoning, worker-thread spawn failures, fs ops in spec
    /// ingestion, CLI JSON serialization, sqlite/tracking errors, and
    /// project-bootstrap validation. New typed variants should be
    /// peeled off as call-site clusters emerge.
    #[error("state error: {0}")]
    State(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("client error: {0}")]
    Client(String),

    #[error("gate failure: {0}")]
    Gate(String),

    #[error("instruction file not found: {0}")]
    InstructionMissing(PathBuf),

    /// LLM dispatch aborted mid-call because the shared cancel flag
    /// flipped (the dashboard pushed a cancel through the control
    /// socket while a dispatch was in flight). Distinct from `Llm`
    /// so the orchestrator's turn loop can route this back through
    /// its SessionEnd::Cancelled path -- semantically identical to
    /// the user-typed `/end-session` or the wire-level
    /// `HostEvent::Cancel`, just observed by the agent itself
    /// instead of by `host.recv()`.
    #[error("llm dispatch cancelled mid-call")]
    Cancelled,

    #[error("foundation root could not be resolved: {0}")]
    FoundationRoot(String),

    #[error("invalid step id: {0}")]
    InvalidStep(String),
}

pub type Result<T> = std::result::Result<T, Error>;
