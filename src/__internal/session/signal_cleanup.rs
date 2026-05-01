//! Process-wide Ctrl-C / SIGINT / SIGTERM handler that runs cleanup
//! tasks the interactive PTY drivers register at startup.
//!
//! Two cleanup tasks live here:
//!
//!   1. Restore the controlling terminal to cooked mode. Without
//!      this, killing sim-flow with Ctrl-C while the PTY proxy is
//!      running leaves the user's terminal in raw mode -- no echo,
//!      no line buffering, hard to recover without `reset`.
//!
//!   2. Remove any control-socket files registered by the
//!      single-session driver. Without this, the next dashboard
//!      click hits the stale socket with `ECONNREFUSED`. (The TS
//!      client now self-heals stale sockets, but cleaning up
//!      proactively avoids the round-trip.)
//!
//! `ctrlc::set_handler` can only be installed once per process; this
//! module wraps that constraint behind `install_signal_cleanup`,
//! which is idempotent: subsequent callers append their cleanup
//! paths to a shared registry and the existing handler picks them
//! up on the next signal.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Paths the signal handler will `remove_file` on Ctrl-C / SIGTERM.
/// `OnceLock` so we don't pay for any synchronization on the common
/// (no-signal) path; `Mutex` so multiple drivers can register paths
/// without racing.
fn registered_paths() -> &'static Mutex<Vec<PathBuf>> {
    static REGISTRY: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

/// True after `install_signal_cleanup` has wired the ctrlc handler.
fn handler_installed() -> &'static Mutex<bool> {
    static FLAG: OnceLock<Mutex<bool>> = OnceLock::new();
    FLAG.get_or_init(|| Mutex::new(false))
}

/// Register a path the signal handler should remove on shutdown.
/// Idempotent: duplicate paths are merged. Installs the ctrlc handler
/// on first call. Subsequent calls only update the registry.
pub fn install_signal_cleanup(socket_path: Option<&Path>) {
    if let Some(p) = socket_path
        && let Ok(mut paths) = registered_paths().lock()
        && !paths.iter().any(|existing| existing == p)
    {
        paths.push(p.to_path_buf());
    }
    let mut installed = match handler_installed().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if *installed {
        return;
    }
    let result = ctrlc::set_handler(on_signal);
    if result.is_ok() {
        *installed = true;
    }
}

/// Best-effort cleanup invoked from the signal-handler thread. We
/// don't have access to any specific orchestrator state from here;
/// only the global registry. Exits the process with the conventional
/// SIGINT exit code (130) so callers / shells see the right status.
fn on_signal() {
    // Best-effort cooked-mode restore. Safe even if we never entered
    // raw mode (no-op) and idempotent across repeated signals.
    let _ = crossterm::terminal::disable_raw_mode();

    // Drain the registered paths and remove each. Drop poisoning is
    // ignored -- this is teardown, not a hot path.
    if let Ok(mut paths) = registered_paths().lock() {
        for p in paths.drain(..) {
            let _ = std::fs::remove_file(&p);
        }
    }

    // Try to give the user a recognizable exit message before we
    // hand off to `process::exit`. eprintln! is signal-safe-enough
    // for our purposes; if it deadlocks on a poisoned stderr lock
    // we fall through to `exit` below regardless.
    let _ = std::io::Write::write_all(
        &mut std::io::stderr(),
        b"\nsim-flow: interrupted, cleanup ran. Exiting (130).\n",
    );

    std::process::exit(130);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registering_multiple_paths_dedups() {
        // Use a fresh path so this test is robust to other tests in
        // the same process polluting the registry.
        let p1 = std::env::temp_dir().join("sim-flow-test-cleanup-A.sock");
        install_signal_cleanup(Some(&p1));
        install_signal_cleanup(Some(&p1));
        let count = registered_paths()
            .lock()
            .unwrap()
            .iter()
            .filter(|x| **x == p1)
            .count();
        assert_eq!(count, 1, "duplicates should be deduped");
    }

    #[test]
    fn registering_none_just_installs_handler() {
        // Should not panic; should not add a path. We can't easily
        // test that the ctrlc handler was set without invoking the
        // signal, but at least we can confirm the call succeeds.
        install_signal_cleanup(None);
    }
}
