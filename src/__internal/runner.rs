//! Step runner: executes a step's work + critique session pair and runs
//! gate validation.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::client::{Client, Invocation, SessionKind, SessionMode};
use crate::config::Config;
use crate::gate::{self, GateCheck, GateReport};
use crate::prompts;
use crate::state::State;
use crate::steps::{StepDescriptor, StepRegistry};
use crate::{Error, Result};

pub const DOT_SIM_FLOW: &str = ".sim-flow";
/// Critiques live under the project's `docs/` tree (and NOT under
/// `.sim-flow/`) so the agent can write to them via its own tools
/// without being granted write access to orchestrator state. The
/// agent is forbidden from touching anything under `.sim-flow/`.
pub const CRITIQUES_DIR: &str = "docs/critiques";
pub const LOGS_DIR: &str = ".sim-flow/logs";

#[derive(Debug)]
pub struct RunOutcome {
    pub gate_report: GateReport,
    pub work_stdout: String,
    pub critique_stdout: String,
}

pub struct StepRunner<'a> {
    pub project_dir: &'a Path,
    pub foundation_root: &'a Path,
    pub registry: &'a StepRegistry,
    pub config: &'a Config,
    pub now: Box<dyn Fn() -> String + 'a>,
    /// Optional injected client override. When set, the runner uses this
    /// client for both work and critique sessions instead of building a
    /// client from config. Primarily for tests.
    pub client_override: Option<Arc<dyn Client>>,
}

impl<'a> StepRunner<'a> {
    pub fn new(
        project_dir: &'a Path,
        foundation_root: &'a Path,
        registry: &'a StepRegistry,
        config: &'a Config,
    ) -> Self {
        Self {
            project_dir,
            foundation_root,
            registry,
            config,
            now: Box::new(system_now),
            client_override: None,
        }
    }

    /// Inject a specific client instance (used by tests).
    pub fn with_client(mut self, client: Arc<dyn Client>) -> Self {
        self.client_override = Some(client);
        self
    }

    /// Run a single step (work + critique + gate validation).
    ///
    /// On success, updates `state` to record a gate pass. On failure,
    /// returns the gate report without mutating state. The caller is
    /// responsible for persisting state.
    pub fn run(
        &self,
        step: &StepDescriptor,
        state: &mut State,
        candidate: Option<&str>,
    ) -> Result<RunOutcome> {
        self.check_prerequisite(step, state)?;
        self.prepare_dirs()?;

        let work_instructions = prompts::load(
            self.foundation_root,
            step.instruction_slug,
            SessionKind::Work,
        )?;
        let critique_instructions = prompts::load(
            self.foundation_root,
            step.instruction_slug,
            SessionKind::Critique,
        )?;

        let client_name = self.config.effective_client(step.id);
        let client: Arc<dyn Client> = match &self.client_override {
            Some(c) => Arc::clone(c),
            None => crate::clients::build(self.config, client_name),
        };

        let session_mode = self.session_mode_for(client.as_ref());
        print_banner(step.id, SessionKind::Work, session_mode);
        let work_invocation = Invocation {
            step: step.id.to_string(),
            kind: SessionKind::Work,
            mode: session_mode,
            prompt: default_prompt(step.id, SessionKind::Work),
            instructions: work_instructions,
            project_dir: self.project_dir.to_path_buf(),
            candidate: candidate.map(|c| c.to_string()),
            timeout_seconds: self
                .config
                .steps
                .get(step.id)
                .and_then(|s| s.timeout_seconds),
        };
        let work_session = client.invoke(&work_invocation)?;
        self.write_log(step.id, SessionKind::Work, candidate, &work_session)?;
        if !work_session.success() {
            return Err(Error::Client(format!(
                "work session for {} exited with status {}",
                step.id, work_session.exit_status
            )));
        }

        // Fresh client instance so no session state leaks between work and
        // critique. For subprocess clients this is already the case, but
        // being explicit matches the architectural invariant.
        let critique_client: Arc<dyn Client> = match &self.client_override {
            Some(c) => Arc::clone(c),
            None => crate::clients::build(self.config, client_name),
        };
        print_banner(step.id, SessionKind::Critique, session_mode);
        let critique_invocation = Invocation {
            step: step.id.to_string(),
            kind: SessionKind::Critique,
            mode: session_mode,
            prompt: default_prompt(step.id, SessionKind::Critique),
            instructions: critique_instructions,
            project_dir: self.project_dir.to_path_buf(),
            candidate: candidate.map(|c| c.to_string()),
            timeout_seconds: self
                .config
                .steps
                .get(step.id)
                .and_then(|s| s.timeout_seconds),
        };
        let critique_session = critique_client.invoke(&critique_invocation)?;
        self.write_log(step.id, SessionKind::Critique, candidate, &critique_session)?;
        if !critique_session.success() {
            return Err(Error::Client(format!(
                "critique session for {} exited with status {}",
                step.id, critique_session.exit_status
            )));
        }

        let checks = resolve_gate_checks(step, candidate);
        let report = gate::evaluate(self.project_dir, &checks)?;
        if report.is_clean() {
            let now = (self.now)();
            if let Some(candidate_name) = candidate {
                state.mark_candidate_passed(step.id, candidate_name, now);
            } else {
                state.mark_passed(step.id, now);
            }
            state.current_step = step.id.to_string();
        }

        Ok(RunOutcome {
            gate_report: report,
            work_stdout: work_session.stdout,
            critique_stdout: critique_session.stdout,
        })
    }

    fn check_prerequisite(&self, step: &StepDescriptor, state: &State) -> Result<()> {
        if let Some(prereq) = step.prerequisite
            && !state.is_passed(prereq)
        {
            return Err(Error::State(format!(
                "cannot run {}: prerequisite {} has not passed",
                step.id, prereq
            )));
        }
        Ok(())
    }

    fn prepare_dirs(&self) -> Result<()> {
        for rel in [CRITIQUES_DIR, LOGS_DIR] {
            let dir = self.project_dir.join(rel);
            std::fs::create_dir_all(&dir).map_err(|source| Error::Io { path: dir, source })?;
        }
        Ok(())
    }

    /// Pick the session mode based on the client. The mock client
    /// remains OneShot so tests keep working; every other client runs
    /// Interactive so the user can drive the TUI.
    fn session_mode_for(&self, client: &dyn Client) -> SessionMode {
        if client.name() == "mock" {
            SessionMode::OneShot
        } else {
            SessionMode::Interactive
        }
    }

    fn write_log(
        &self,
        step_id: &str,
        kind: SessionKind,
        candidate: Option<&str>,
        session: &crate::client::Session,
    ) -> Result<()> {
        let kind_str = match kind {
            SessionKind::Work => "work",
            SessionKind::Critique => "critique",
        };
        let mut name = format!("{step_id}-{kind_str}");
        if let Some(c) = candidate {
            name.push('-');
            name.push_str(c);
        }
        name.push('-');
        name.push_str(&timestamp_for_log((self.now)()));
        name.push_str(".log");
        let path = self.project_dir.join(LOGS_DIR).join(name);
        let body = format!(
            "exit: {}\nstdout:\n{}\nstderr:\n{}\n",
            session.exit_status, session.stdout, session.stderr
        );
        std::fs::write(&path, body).map_err(|source| Error::Io { path, source })
    }
}

/// Replace path placeholders (`<candidate>`) in gate-check paths with the
/// actual candidate name.
fn resolve_gate_checks(step: &StepDescriptor, candidate: Option<&str>) -> Vec<GateCheck> {
    step.gate_checks
        .iter()
        .map(|check| match (check, candidate) {
            (GateCheck::FileExists { path, description }, Some(c)) => GateCheck::FileExists {
                path: substitute_candidate(path, c),
                description: description.clone(),
            },
            (
                GateCheck::FileMatches {
                    path,
                    pattern,
                    description,
                },
                Some(c),
            ) => GateCheck::FileMatches {
                path: substitute_candidate(path, c),
                pattern: pattern.clone(),
                description: description.clone(),
            },
            (GateCheck::CritiqueClean { path, description }, Some(c)) => GateCheck::CritiqueClean {
                path: substitute_candidate(path, c),
                description: description.clone(),
            },
            (other, _) => other.clone(),
        })
        .collect()
}

fn substitute_candidate(path: &Path, candidate: &str) -> PathBuf {
    let s = path.to_string_lossy();
    PathBuf::from(s.replace("<candidate>", candidate))
}

fn system_now() -> String {
    match SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => format!("{}", d.as_secs()),
        Err(_) => "0".to_string(),
    }
}

fn timestamp_for_log(stamp: String) -> String {
    stamp
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
}

fn default_prompt(step_id: &str, kind: SessionKind) -> String {
    match kind {
        SessionKind::Work => format!(
            "You are executing step {step_id} (work session). Follow the accompanying instructions."
        ),
        SessionKind::Critique => format!(
            "You are a critique session reviewing the work produced for step {step_id}. Follow the accompanying instructions and write the critique file."
        ),
    }
}

/// Print a short banner before each session so the user can tell which
/// session (work vs critique) is starting. The banner is skipped in
/// OneShot mode since the test harness captures stdout.
fn print_banner(step_id: &str, kind: SessionKind, mode: SessionMode) {
    if mode != SessionMode::Interactive {
        return;
    }
    let (label, hint) = match kind {
        SessionKind::Work => (
            "work session",
            "iterate with the agent; type /exit or Ctrl-D when ready for the critique.",
        ),
        SessionKind::Critique => (
            "critique session",
            "review the critique with the agent; type /exit or Ctrl-D to close the session \
             and run the gate.",
        ),
    };
    eprintln!("\n=== {step_id} {label} ===");
    eprintln!("{hint}\n");
}
