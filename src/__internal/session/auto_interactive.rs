//! Interactive auto driver: spawns the configured CLI agent (currently
//! `claude` only) on a PTY for each step, injects that step's prompt
//! verbatim, and transparent-proxies user keystrokes / agent output
//! through to the controlling terminal until the user types `/exit`.
//!
//! Compared to the JSONL-host `run_auto` driver:
//!
//!   - No turn loop. The orchestrator hands control to the agent and
//!     the user; the agent decides what tool calls to make and what
//!     artifacts to write. Sim-flow just inspects the filesystem
//!     after the user `/exit`s.
//!   - No protocol. There's no host on the other side; the driver
//!     owns the PTY directly.
//!   - The user can interject mid-step. They share the terminal with
//!     the agent and can type anything they want.
//!
//! Two session-mode flavors:
//!
//!   - **per-step**: each step spawns a fresh PTY child. Implemented
//!     here.
//!   - **single**: one PTY child for the whole flow, lazy re-spawned
//!     across `/exit`s. Pass 2; not implemented yet.
//!
//! Step transition logic:
//!   1. Build the step's prompt (same `build_initial_messages` helper
//!      the JSONL path uses) and render it as a single markdown blob
//!      with role headers.
//!   2. Spawn a fresh `claude` on a PTY.
//!   3. Inject the rendered prompt as the first stdin write.
//!   4. `proxy_until_exit` -- user works, claude writes artifacts.
//!   5. Run the structural gate against disk.
//!   6. If clean, mark the step passed and advance `current_step`.
//!      Otherwise stop with a clear diagnostic showing the failures.
//!   7. Loop to the next step.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::client::SessionKind;
use crate::session::agent::interactive_pty::{
    InteractivePtySession, PtyWriter, finish_proxy, proxy_until_exit, start_pty_proxy,
};
use crate::session::control_socket::{
    ControlCommand, ControlEvent, ControlListener, GateFailure, default_socket_path,
};
use crate::session::orchestrator::{MessageBundle, OrchestratorOptions, build_initial_messages};
use crate::session::protocol::{LlmMessage, LlmRole};
use crate::session::signal_cleanup::install_signal_cleanup;
use crate::state::State;
use crate::steps::registry_for;
use crate::{Error, Result, gate};

/// Inputs for the interactive auto driver. Subset of `AutoOptions`
/// minus the host-protocol caps (max_auto_iters / max_critique_iters
/// don't apply here -- the user decides when each step is "done" by
/// typing `/exit`).
pub struct AutoInteractiveOptions {
    pub project_dir: PathBuf,
    pub foundation_root: PathBuf,
    /// Backend name. Currently only `claude` / `claude-cli` is
    /// supported in this driver; other backends fall through to the
    /// JSONL `run_auto` path.
    pub llm_backend: String,
    pub llm_model: Option<String>,
    /// Optional `--dm0-interactive` parity (no-op in this driver --
    /// every step is "interactive" by definition).
    pub dm0_interactive: bool,
}

/// Top-level entry: walk the remaining steps, spawning a fresh agent
/// per step until the flow is done, the gate fails on a step, or the
/// agent dies with a non-zero exit.
pub fn run_auto_interactive(opts: AutoInteractiveOptions) -> Result<()> {
    // No socket in per-step mode, but we still want to restore
    // cooked terminal mode if the user Ctrl-Cs out of a session.
    install_signal_cleanup(None);
    let dot = opts.project_dir.join(".sim-flow");
    let initial_state = State::load(&dot)?;
    let registry = registry_for(initial_state.flow);
    let order = registry.order_for(initial_state.flow);
    let starting_idx = order
        .iter()
        .position(|s| *s == initial_state.current_step.as_str())
        .ok_or_else(|| {
            Error::State(format!(
                "auto-interactive: current step `{}` is not in the {} flow",
                initial_state.current_step,
                initial_state.flow.as_str()
            ))
        })?;
    let remaining: Vec<&'static str> = order[starting_idx..].to_vec();

    // Status messages go to stderr so they coexist with the agent's
    // PTY output on stdout without clobbering it.
    let _ = writeln!(
        std::io::stderr(),
        "sim-flow auto-interactive: starting at {} (backend: {}; {} step(s) remaining)",
        initial_state.current_step,
        opts.llm_backend,
        remaining.len(),
    );

    for step_id in remaining {
        // Re-load state per step so a manual edit between steps is
        // respected.
        let state = State::load(&dot)?;
        let step = registry
            .get(step_id)
            .ok_or_else(|| Error::InvalidStep(format!("{step_id} not in registry")))?
            .clone();

        // Run work, then critique.
        for kind in [SessionKind::Work, SessionKind::Critique] {
            run_step_interactive(&opts, &state, &step, kind)?;
        }

        // Run the structural gate. On failure, stop -- the user can
        // re-launch sim-flow to retry the step.
        let report = gate::evaluate(&opts.project_dir, &step.gate_checks)?;
        if !report.is_clean() {
            let _ = writeln!(
                std::io::stderr(),
                "\nsim-flow auto-interactive: gate not clean for {} ({} failure(s)). Stopping.",
                step_id,
                report.failures.len(),
            );
            for failure in &report.failures {
                let _ = writeln!(
                    std::io::stderr(),
                    "  - {}: {}",
                    failure.description,
                    failure.reason,
                );
            }
            return Ok(());
        }

        // Advance: commit step artifacts, then mark passed, bump
        // current_step. Commit first so the git history shows each
        // gate-clean checkpoint -- if commit fails (no git, hooks,
        // etc.) we still advance so flow progress isn't held
        // hostage to git config.
        let next_step_id: Option<&'static str> = order
            .iter()
            .position(|s| *s == step.id)
            .and_then(|i| order.get(i + 1).copied());
        let outcome =
            crate::git_commit::commit_step_advance(&opts.project_dir, step.id, next_step_id);
        if let Some(msg) = crate::git_commit::outcome_message(&outcome) {
            let _ = writeln!(std::io::stderr(), "{msg}");
        }
        let mut state_mut = State::load(&dot)?;
        state_mut.mark_passed(step.id, current_iso8601());
        if let Some(next) = next_step_id {
            state_mut.current_step = next.to_string();
        }
        state_mut.save(&dot)?;
        let _ = writeln!(
            std::io::stderr(),
            "\nsim-flow auto-interactive: {} passed; current step is now {}",
            step.id,
            state_mut.current_step,
        );
    }
    Ok(())
}

fn run_step_interactive(
    opts: &AutoInteractiveOptions,
    state: &State,
    step: &crate::steps::StepDescriptor,
    kind: SessionKind,
) -> Result<()> {
    // Reuse the orchestrator's prompt-building helper so the
    // interactive driver and the JSONL driver send the SAME prompt
    // for a given step. Differences here would mean the model gets
    // different context depending on which driver is used.
    // Use Default to inherit the runaway-loop guards
    // (`max_llm_requests` / `max_identical_responses`). Interactive
    // PTY mode doesn't go through the orchestrator turn loop where
    // those caps fire, but we keep the defaults in case the helper
    // is reused later by a JSONL-host-backed driver.
    let session_opts = OrchestratorOptions {
        project_dir: opts.project_dir.clone(),
        foundation_root: opts.foundation_root.clone(),
        step_id: step.id.to_string(),
        kind,
        candidate: None,
        llm_backend: opts.llm_backend.clone(),
        llm_model: opts.llm_model.clone(),
        auto: !opts.dm0_interactive || step.id != "DM0",
        max_auto_iters: 0,
        // PTY-driven CLI agents (claude / codex / gh-copilot) have
        // native Write/Edit tools and the orchestrator never parses
        // fenced ` ```<path>` blocks out of their PTY stream. Tell
        // the agent to use its tools rather than the artifact-write
        // convention -- otherwise it emits the fence, no writer
        // exists, and it round-trips back with "the spec was
        // generated but never written".
        agent_has_native_fs_tools: true,
        ..OrchestratorOptions::default()
    };
    let MessageBundle { messages, tools: _ } = build_initial_messages(&session_opts, step)?;
    log_conventions_bootstrap(&session_opts);
    let prompt = render_messages_for_terminal(&messages);

    let banner = format!(
        "\n========================================\n\
         sim-flow: starting {} {} session\n\
         (Claude session opens below; type `/exit` when you're done.)\n\
         ========================================\n",
        step.id,
        match kind {
            SessionKind::Work => "work",
            SessionKind::Critique => "critique",
        },
    );
    let _ = writeln!(std::io::stderr(), "{banner}");

    let argv = build_claude_argv(opts.llm_model.as_deref());
    let mut session =
        InteractivePtySession::new(argv, Some(opts.project_dir.clone()), Vec::<String>::new());
    session.spawn()?;

    // Inject the rendered prompt. Newline already appended by `inject`
    // if the body doesn't end with one.
    session.inject(&prompt)?;

    let exit = proxy_until_exit(&mut session)?;
    if !exit.clean {
        return Err(Error::State(format!(
            "claude exited non-zero (code={:?}) during {} {} session",
            exit.code,
            step.id,
            match kind {
                SessionKind::Work => "work",
                SessionKind::Critique => "critique",
            },
        )));
    }
    let _ = state; // silence lint when unused (kept for future use)
    Ok(())
}

/// Build the argv for spawning the `claude` CLI in interactive mode.
/// Differs from `ClaudeAgent`'s one-shot `-p` use:
///   - No `-p` flag (interactive, not print-and-exit).
///   - Optional `--model` if the user configured one.
fn build_claude_argv(model: Option<&str>) -> Vec<String> {
    let mut argv = vec!["claude".to_string()];
    if let Some(m) = model {
        argv.push("--model".to_string());
        argv.push(crate::session::agent::normalize_model_for_cli(m));
    }
    argv
}

/// Render a `Vec<LlmMessage>` into a single markdown blob with role
/// headers, suitable for piping straight into an interactive `claude`
/// session as the user's first message. Mirrors the `ClaudeAgent`
/// one-shot prompt format so the agent sees the same shape regardless
/// of driver.
pub fn render_messages_for_terminal(messages: &[LlmMessage]) -> String {
    let mut out = String::new();
    for m in messages {
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

fn current_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    dur.as_secs().to_string()
}

/// Stub kept here so the dashboard / CLI flag plumbing in Pass 2 has a
/// stable name to import. The actual function lives in `session::auto`.
#[allow(dead_code)]
pub(crate) fn _project_dir_unused(_p: &Path) {}

// =====================================================================
// Single-session mode (Pass 2a).
// =====================================================================

/// Drive a single, long-lived `claude` session for the entire flow.
///
/// Lifecycle:
///   1. Open the control socket so the dashboard can connect.
///   2. Spawn `claude` and inject the current step's prompt.
///   3. Park in `proxy_until_exit` while the user works. Concurrently,
///      a control-listener thread reads commands off the socket and
///      forwards them as actions:
///        - `Inject(text)`: write into claude's stdin while it's
///          running (no respawn).
///        - `RunGate(step?)`: evaluate gate locally, inject the
///          formatted result.
///        - `Advance(step?)`: gate + mark passed + bump current_step
///          + inject the next step's prompt (without exiting claude
///            if it's still alive).
///        - `Shutdown`: kill claude and break the loop.
///   4. When claude exits (user typed `/exit`), the driver stays
///      alive blocking on the next socket command. Next command
///      lazy-respawns claude before injecting.
pub fn run_auto_interactive_single(opts: AutoInteractiveOptions) -> Result<()> {
    let dot = opts.project_dir.join(".sim-flow");
    let initial_state = State::load(&dot)?;
    let registry = registry_for(initial_state.flow);

    // Bind the control socket BEFORE we spawn claude so the dashboard
    // can connect during startup.
    let socket_path = default_socket_path(&opts.project_dir);
    let listener = ControlListener::bind(socket_path.clone())?;
    // Wire Ctrl-C / SIGTERM cleanup: restore cooked terminal mode and
    // remove the socket file so the next dashboard click doesn't see
    // a stale-but-unbound file (the TS client now handles that case
    // too, but proactive cleanup avoids the round-trip).
    install_signal_cleanup(Some(&socket_path));
    let _ = writeln!(
        std::io::stderr(),
        "sim-flow auto-interactive (single): control socket at {}",
        socket_path.display(),
    );

    // Build the initial step prompt and seed it into a fresh claude.
    let first_step_id = initial_state.current_step.clone();
    let first_step = registry
        .get(&first_step_id)
        .ok_or_else(|| Error::InvalidStep(format!("{first_step_id} not in registry")))?
        .clone();
    let argv = build_claude_argv(opts.llm_model.as_deref());
    let mut session = InteractivePtySession::new(
        argv.clone(),
        Some(opts.project_dir.clone()),
        Vec::<String>::new(),
    );

    let _ = writeln!(
        std::io::stderr(),
        "sim-flow: spawning {} (model {})...",
        argv.first().map(String::as_str).unwrap_or("claude"),
        opts.llm_model.as_deref().unwrap_or("(default)"),
    );
    session.spawn()?;

    // Spawn a control-listener thread that handles socket commands
    // IN PARALLEL with the main proxy loop. Critical design
    // constraint: the main thread holds `session_arc.lock()`
    // throughout `finish_proxy` (which blocks on the child exiting).
    // If the listener also tried to lock the session for every
    // command, dashboard button clicks would deadlock until the
    // user typed `/exit`.
    //
    // Solution: cache a `PtyWriter` clone in `pty_writer_arc` that's
    // independent of the session mutex. Inject / RunGate / Advance /
    // Reset commands write through that cached handle without ever
    // touching `session_arc`. Only the rare cases that actually need
    // the session struct itself -- Shutdown (kill child) and
    // lazy-respawn after `/exit` -- bother locking it.
    let session_arc = std::sync::Arc::new(std::sync::Mutex::new(session));
    // Capture the initial writer immediately after the spawn above.
    // The listener uses this to send dashboard-driven injections
    // without acquiring `session_arc`. When the user `/exit`s and
    // the listener later respawns claude, it refreshes this slot
    // with a new PtyWriter clone.
    let pty_writer_arc = std::sync::Arc::new(std::sync::Mutex::new(Some({
        let mut s = session_arc
            .lock()
            .map_err(|_| Error::State("session mutex poisoned".into()))?;
        s.writer()?
    })));
    let listener_arc = std::sync::Arc::new(listener);
    let opts_arc = std::sync::Arc::new(opts);
    let session_for_listener = session_arc.clone();
    let writer_for_listener = pty_writer_arc.clone();
    let listener_for_thread = listener_arc.clone();
    let opts_for_listener = opts_arc.clone();
    let _listener_thread = std::thread::Builder::new()
        .name("sim-flow-control-dispatch".into())
        .spawn(move || {
            while let Some(cmd) = listener_for_thread.recv() {
                let result = dispatch_command_via_writer(
                    &cmd,
                    &session_for_listener,
                    &writer_for_listener,
                    &opts_for_listener,
                    &listener_for_thread,
                );
                if let Err(err) = result {
                    let _ = writeln!(std::io::stderr(), "sim-flow: control command failed: {err}",);
                }
                if matches!(cmd, ControlCommand::Shutdown) {
                    break;
                }
            }
        })
        .map_err(|err| Error::State(format!("control-dispatch thread: {err}")))?;

    // Main proxy loop. Each iteration: start the PTY proxy threads
    // (reader is now draining claude's output), inject the next
    // step's prompt if claude is fresh, wait for claude to exit.
    // After /exit, the listener thread's next command will lazy-
    // respawn claude and re-inject; we loop back to start a fresh
    // proxy for the new child.
    let mut first_iteration = true;
    loop {
        let proxy_handle = {
            let mut s = session_arc
                .lock()
                .map_err(|_| Error::State("session mutex poisoned".into()))?;
            if !s.is_alive() {
                // Listener-thread shutdown or initial bootstrap with
                // a dead child. Wait briefly; if still dead, exit.
                drop(s);
                std::thread::sleep(std::time::Duration::from_millis(100));
                let mut s = session_arc
                    .lock()
                    .map_err(|_| Error::State("session mutex poisoned".into()))?;
                if !s.is_alive() {
                    break;
                }
                start_pty_proxy(&mut s)?
            } else {
                start_pty_proxy(&mut s)?
            }
        };
        if first_iteration {
            // First iteration: inject the very first step's prompt.
            // Only do this once; on subsequent iterations the
            // listener (or a respawn-injection) supplies the next
            // prompt.
            let mut s = session_arc
                .lock()
                .map_err(|_| Error::State("session mutex poisoned".into()))?;
            inject_step_prompt(&mut s, &opts_arc, &first_step, SessionKind::Work)?;
            drop(s);
            first_iteration = false;
        }
        let _ = writeln!(
            std::io::stderr(),
            "sim-flow: agent active. Type `/exit` to leave; orchestrator stays alive listening on {}",
            socket_path.display(),
        );
        let exit = {
            let mut s = session_arc
                .lock()
                .map_err(|_| Error::State("session mutex poisoned".into()))?;
            finish_proxy(&mut s, proxy_handle)?
        };
        let _ = writeln!(
            std::io::stderr(),
            "\nsim-flow: agent exited (code={:?}). Waiting for the next dashboard command, or Ctrl-C to quit.",
            exit.code,
        );
        // Block here on a sentinel command from the listener that
        // lazy-respawns claude. Simplest: poll session.is_alive every
        // 100ms; the listener's dispatch will spawn claude when a
        // command lands.
        loop {
            let alive = {
                let mut s = session_arc
                    .lock()
                    .map_err(|_| Error::State("session mutex poisoned".into()))?;
                s.is_alive()
            };
            if alive {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    Ok(())
}

/// Listener-thread dispatcher that prefers the
/// cached `PtyWriter` over locking the session. Only `Shutdown` and
/// the lazy-respawn fallback ever take `session_arc`.
fn dispatch_command_via_writer(
    cmd: &ControlCommand,
    session_arc: &std::sync::Arc<std::sync::Mutex<InteractivePtySession>>,
    pty_writer: &std::sync::Arc<std::sync::Mutex<Option<PtyWriter>>>,
    opts: &AutoInteractiveOptions,
    listener: &ControlListener,
) -> Result<()> {
    use std::sync::Mutex;

    /// Try to write `text` via the cached writer. Returns `Ok(true)`
    /// when the write landed; `Ok(false)` when the writer is dead /
    /// missing (caller should respawn). `Err` only on a real I/O
    /// failure other than "claude not running."
    fn try_write(slot: &Mutex<Option<PtyWriter>>, text: &str) -> Result<bool> {
        let guard = slot
            .lock()
            .map_err(|_| Error::State("pty writer mutex poisoned".into()))?;
        let Some(w) = guard.as_ref() else {
            return Ok(false);
        };
        match w.inject(text) {
            Ok(()) => Ok(true),
            Err(_) => Ok(false), // writer is dead; caller respawns
        }
    }

    /// Slow path: claude exited. Lock the session, respawn, store
    /// the new writer in the shared slot, retry.
    fn respawn_and_write(
        session_arc: &std::sync::Arc<Mutex<InteractivePtySession>>,
        pty_writer: &std::sync::Arc<Mutex<Option<PtyWriter>>>,
        text: &str,
    ) -> Result<()> {
        let new_writer = {
            let mut s = session_arc
                .lock()
                .map_err(|_| Error::State("session mutex poisoned".into()))?;
            s.writer()?
        };
        new_writer.inject(text)?;
        let mut slot = pty_writer
            .lock()
            .map_err(|_| Error::State("pty writer mutex poisoned".into()))?;
        *slot = Some(new_writer);
        Ok(())
    }

    fn write_or_respawn(
        session_arc: &std::sync::Arc<Mutex<InteractivePtySession>>,
        pty_writer: &std::sync::Arc<Mutex<Option<PtyWriter>>>,
        text: &str,
    ) -> Result<()> {
        if try_write(pty_writer, text)? {
            return Ok(());
        }
        respawn_and_write(session_arc, pty_writer, text)
    }

    let dot = opts.project_dir.join(".sim-flow");
    match cmd {
        ControlCommand::Inject { text } => {
            write_or_respawn(session_arc, pty_writer, text)?;
            listener.broadcast(ControlEvent::Injected);
            Ok(())
        }
        ControlCommand::RunGate { step } => {
            let state = State::load(&dot)?;
            let target = step.clone().unwrap_or_else(|| state.current_step.clone());
            let registry = registry_for(state.flow);
            let descriptor = registry
                .get(&target)
                .ok_or_else(|| Error::InvalidStep(format!("{target} not in registry")))?;
            let report = gate::evaluate(&opts.project_dir, &descriptor.gate_checks)?;
            let failures: Vec<GateFailure> = report
                .failures
                .iter()
                .map(|f| GateFailure {
                    description: f.description.clone(),
                    reason: f.reason.clone(),
                })
                .collect();
            let injection = format_gate_result(&target, report.is_clean(), &failures);
            write_or_respawn(session_arc, pty_writer, &injection)?;
            listener.broadcast(ControlEvent::GateResult {
                step: target,
                clean: report.is_clean(),
                failures,
            });
            Ok(())
        }
        ControlCommand::Advance { step } => {
            let state = State::load(&dot)?;
            let target = step.clone().unwrap_or_else(|| state.current_step.clone());
            let registry = registry_for(state.flow);
            let descriptor = registry
                .get(&target)
                .ok_or_else(|| Error::InvalidStep(format!("{target} not in registry")))?
                .clone();
            let report = gate::evaluate(&opts.project_dir, &descriptor.gate_checks)?;
            if !report.is_clean() {
                let failures: Vec<GateFailure> = report
                    .failures
                    .iter()
                    .map(|f| GateFailure {
                        description: f.description.clone(),
                        reason: f.reason.clone(),
                    })
                    .collect();
                let injection = format_gate_result(&target, false, &failures);
                write_or_respawn(session_arc, pty_writer, &injection)?;
                listener.broadcast(ControlEvent::GateResult {
                    step: target,
                    clean: false,
                    failures,
                });
                return Ok(());
            }
            let mut state_mut = State::load(&dot)?;
            let order = registry.order_for(state_mut.flow);
            let next = order
                .iter()
                .position(|s| *s == descriptor.id)
                .and_then(|i| order.get(i + 1).copied());

            // Commit gate-clean step artifacts before mutating
            // sim-flow state. Failures are non-fatal; they log via
            // the control-socket broadcast below so the dashboard
            // surfaces the problem without blocking advance.
            let outcome =
                crate::git_commit::commit_step_advance(&opts.project_dir, descriptor.id, next);
            if let Some(msg) = crate::git_commit::outcome_message(&outcome) {
                let _ = writeln!(std::io::stderr(), "{msg}");
            }

            state_mut.mark_passed(descriptor.id, current_iso8601());
            if let Some(n) = next {
                state_mut.current_step = n.to_string();
            }
            state_mut.save(&dot)?;
            // Just announce the advance -- don't inject the next
            // step's prompt automatically. The user explicitly
            // starts the next step by clicking "Run Step" in the
            // dashboard (which sends an `Inject` command with the
            // step's prompt). Auto-injecting on advance was a
            // surprise: the user had no chance to review the
            // gate-clean state before the next session started
            // running. /advance now mirrors `git checkout`-style
            // semantics: move the pointer, leave the working
            // session idle.
            let notice = match next {
                Some(next_id) => format!(
                    "[SYSTEM]\nStep {} passed. Current step is now {}. \
                     Click \"Run Step\" in the dashboard to start it.",
                    descriptor.id, next_id,
                ),
                None => format!(
                    "[SYSTEM]\nStep {} passed. This was the final step in the flow.",
                    descriptor.id,
                ),
            };
            write_or_respawn(session_arc, pty_writer, &notice)?;
            listener.broadcast(ControlEvent::StateAdvanced {
                from: descriptor.id.into(),
                to: next.map(String::from),
            });
            Ok(())
        }
        ControlCommand::Reset { step } => {
            let mut state = State::load(&dot)?;
            let registry = registry_for(state.flow);
            let order = registry.order_for(state.flow);
            state.reset(step, &order)?;
            state.current_step = step.clone();
            state.save(&dot)?;
            let injection = format!(
                "[SYSTEM]\nStep {} has been reset. State now points to {}.",
                step, state.current_step,
            );
            write_or_respawn(session_arc, pty_writer, &injection)?;
            listener.broadcast(ControlEvent::Injected);
            Ok(())
        }
        ControlCommand::Shutdown => {
            // Shutdown DOES need the session lock -- we have to kill
            // the child. Acceptable: the user is closing things and a
            // brief block here doesn't strand a button click.
            if let Ok(mut s) = session_arc.lock() {
                s.kill();
            }
            // Also clear the writer slot so any further commands that
            // sneak in fail-fast.
            if let Ok(mut slot) = pty_writer.lock() {
                *slot = None;
            }
            listener.broadcast(ControlEvent::Shutdown);
            Ok(())
        }
    }
}

/// Build the markdown-formatted gate result we inject into claude.
fn format_gate_result(step: &str, clean: bool, failures: &[GateFailure]) -> String {
    if clean {
        format!(
            "[SYSTEM]\nStructural gate for {step} is CLEAN. (No failures.) You can ask the user to /advance, or continue iterating.",
        )
    } else {
        let lines: String = failures
            .iter()
            .map(|f| format!("  - {}: {}", f.description, f.reason))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "[SYSTEM]\nStructural gate for {step} is NOT clean. {} failure(s):\n{lines}",
            failures.len(),
        )
    }
}

/// Common helper: render the step's prompt and inject it. Used both at
/// startup and after `Advance`.
fn inject_step_prompt(
    session: &mut InteractivePtySession,
    opts: &AutoInteractiveOptions,
    step: &crate::steps::StepDescriptor,
    kind: SessionKind,
) -> Result<()> {
    // Use Default to inherit the runaway-loop guards
    // (`max_llm_requests` / `max_identical_responses`). Interactive
    // PTY mode doesn't go through the orchestrator turn loop where
    // those caps fire, but we keep the defaults in case the helper
    // is reused later by a JSONL-host-backed driver.
    //
    // `auto: false` -- this helper is only called from the single-
    // session PTY driver, where the user is in the loop and clicks
    // Run Step / Run Critique / Advance from the dashboard. The
    // AUTO_MODE_SYSTEM convention (which would otherwise be inlined)
    // tells the agent "AUTOMATED mode is ACTIVE; the user will not
    // respond; do NOT ask questions" -- exactly wrong for manual
    // dashboard-driven mode. The per-step PTY driver's
    // `run_step_interactive` is the genuinely auto-walking path and
    // keeps `auto: true`; the JSONL `run_auto` driver also keeps it.
    let session_opts = OrchestratorOptions {
        project_dir: opts.project_dir.clone(),
        foundation_root: opts.foundation_root.clone(),
        step_id: step.id.to_string(),
        kind,
        candidate: None,
        llm_backend: opts.llm_backend.clone(),
        llm_model: opts.llm_model.clone(),
        auto: false,
        max_auto_iters: 0,
        agent_has_native_fs_tools: true,
        ..OrchestratorOptions::default()
    };
    let MessageBundle { messages, tools: _ } = build_initial_messages(&session_opts, step)?;
    log_conventions_bootstrap(&session_opts);
    let prompt = render_messages_for_terminal(&messages);
    session.inject(&prompt)
}

/// Log the convention paths the agent was asked to Read on first
/// turn. Fires only in native-tools mode (PTY/CLI) -- in JSONL mode
/// the conventions are inlined into the system stack and there's
/// nothing for the agent to fetch. The eprintln lands in the
/// terminal where `sim-flow auto` is running, which is the same
/// terminal the user just clicked Run/Resume against, so the trail
/// is visible alongside claude's startup output. If the user
/// suspects the agent ignored conventions on a turn, this line
/// confirms which files it was supposed to read.
fn log_conventions_bootstrap(opts: &OrchestratorOptions) {
    if !opts.agent_has_native_fs_tools {
        return;
    }
    let primary = crate::prompts::convention_path(&opts.foundation_root, "native-tools");
    let mut line = format!(
        "sim-flow: conventions delivered via bootstrap directive (agent will Read `{}`",
        primary.display(),
    );
    if opts.auto {
        let auto = crate::prompts::convention_path(&opts.foundation_root, "auto-mode");
        line.push_str(&format!(" + `{}`", auto.display()));
    }
    line.push(')');
    let _ = writeln!(std::io::stderr(), "{line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_messages_matches_claude_one_shot_format() {
        let messages = vec![
            LlmMessage {
                role: LlmRole::System,
                content: "be helpful".into(),
                attachments: Vec::new(),
            },
            LlmMessage {
                role: LlmRole::User,
                content: "hi".into(),
                attachments: Vec::new(),
            },
        ];
        let rendered = render_messages_for_terminal(&messages);
        assert!(rendered.starts_with("[SYSTEM]\nbe helpful"));
        assert!(rendered.ends_with("[USER]\nhi"));
    }

    #[test]
    fn render_messages_handles_empty_input() {
        assert_eq!(render_messages_for_terminal(&[]), "");
    }

    #[test]
    fn build_claude_argv_includes_normalized_model_when_set() {
        let argv = build_claude_argv(Some("claude-code/claude-sonnet-4.6"));
        assert_eq!(
            argv,
            vec![
                "claude".to_string(),
                "--model".to_string(),
                "claude-sonnet-4-6".to_string(),
            ]
        );
    }

    #[test]
    fn build_claude_argv_omits_model_when_none() {
        let argv = build_claude_argv(None);
        assert_eq!(argv, vec!["claude".to_string()]);
    }
}
