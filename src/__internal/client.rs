//! AI client abstraction.
//!
//! Every session (work or critique) is a single `invoke` call that runs a
//! subprocess to completion. Sessions run in one of two modes:
//!
//! - [`SessionMode::Interactive`] attaches the child to the user's
//!   terminal (inherited stdio) and seeds it with the step prompt as the
//!   initial user message. The user drives the conversation until they
//!   exit the AI CLI (e.g. `/exit` or Ctrl-D), at which point the
//!   subprocess terminates and the orchestrator proceeds to the next
//!   session.
//! - [`SessionMode::OneShot`] uses the CLI's non-interactive entry point
//!   (`claude -p`, `codex exec`, `copilot -p`), captures stdout/stderr,
//!   and returns them for inspection. Primarily used by the mock client
//!   and by tests.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Work,
    Critique,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    /// Child attached to the user's TTY; user types until exit.
    Interactive,
    /// Child runs to completion without user interaction; stdout/stderr
    /// captured.
    OneShot,
}

/// A single invocation request.
#[derive(Debug, Clone)]
pub struct Invocation {
    pub step: String,
    pub kind: SessionKind,
    pub mode: SessionMode,
    /// User-facing prompt (usually loaded from `instructions/`).
    pub prompt: String,
    /// Long-form step guidance.
    pub instructions: String,
    /// Working directory for the subprocess.
    pub project_dir: PathBuf,
    /// Optional candidate scope (DS5a/DS5b).
    pub candidate: Option<String>,
    /// Optional per-session timeout; `None` means no enforced timeout.
    pub timeout_seconds: Option<u64>,
}

/// Outcome of an `invoke` call.
///
/// For [`SessionMode::Interactive`], stdout and stderr are empty because
/// output was written directly to the user's terminal and not captured.
#[derive(Debug, Clone)]
pub struct Session {
    pub exit_status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Session {
    pub fn success(&self) -> bool {
        self.exit_status == 0
    }
}

/// Abstract AI client. Each implementation wraps a specific CLI.
pub trait Client: Send + Sync {
    fn name(&self) -> &'static str;
    fn invoke(&self, invocation: &Invocation) -> crate::Result<Session>;
}
