//! Orchestrator preflight checks.
//!
//! Today this module owns one check: making sure
//! [`cargo-tarpaulin`](https://crates.io/crates/cargo-tarpaulin) is
//! installed before the agent reaches the DM3c (Test Execution and
//! Coverage) step. The DM3c work session shells out to `cargo
//! tarpaulin` and, if the binary isn't on `PATH`, the agent gets a
//! `no such command: tarpaulin` error mid-session and burns LLM
//! budget retrying. The orchestrator can avoid that by running
//! `cargo install cargo-tarpaulin` once at startup; subsequent
//! invocations short-circuit on the version probe.
//!
//! The helpers are split so tests can exercise the install path
//! without actually invoking `cargo` -- a `Runner` trait lets the
//! orchestrator inject a real `std::process::Command` runner in
//! production and a recording stub in tests.

use std::process::Command;

/// Outcome of [`ensure_tarpaulin_installed`].
#[derive(Debug, PartialEq, Eq)]
pub enum TarpaulinStatus {
    /// Already on `PATH`; the cached version string is included.
    AlreadyInstalled { version: String },
    /// Wasn't on `PATH`; we ran `cargo install cargo-tarpaulin` and
    /// it succeeded. Subsequent calls in the same process should
    /// see `AlreadyInstalled`.
    JustInstalled,
}

/// Indirection for shelling out so tests can drive the install
/// path without actually running `cargo`. Production wires the
/// [`SystemRunner`] which delegates to `std::process::Command`.
pub trait Runner {
    /// Run `cargo tarpaulin --version`. Return the captured stdout
    /// when the process exits 0; return `None` otherwise (binary
    /// missing, exec failed, non-zero exit).
    fn probe_version(&mut self) -> Option<String>;
    /// Run `cargo install cargo-tarpaulin --locked`. Return `Ok(())`
    /// when the process exits 0; otherwise `Err(reason)`.
    fn install(&mut self) -> Result<(), String>;
}

/// Production [`Runner`] backed by `std::process::Command`. Pulls
/// from the ambient `PATH` so the orchestrator picks up whatever
/// `cargo` shim the user has selected (rustup, asdf, etc.).
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn probe_version(&mut self) -> Option<String> {
        let output = Command::new("cargo")
            .args(["tarpaulin", "--version"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            // Some installers print to stderr; fall back to that
            // before declaring the probe a failure.
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                return None;
            }
            return Some(stderr);
        }
        Some(stdout)
    }

    fn install(&mut self) -> Result<(), String> {
        // `--locked` so we resolve against tarpaulin's own
        // Cargo.lock; otherwise transient registry hiccups can
        // pull a different patch version on every install.
        let status = Command::new("cargo")
            .args(["install", "cargo-tarpaulin", "--locked"])
            .status()
            .map_err(|err| format!("failed to spawn `cargo install cargo-tarpaulin`: {err}"))?;
        if !status.success() {
            return Err(format!(
                "`cargo install cargo-tarpaulin` exited with status {status}"
            ));
        }
        Ok(())
    }
}

/// Ensure `cargo tarpaulin` is on `PATH`, installing it if not.
///
/// Sequence:
///   1. Probe `cargo tarpaulin --version` via `runner.probe_version`.
///      If it succeeds, return [`TarpaulinStatus::AlreadyInstalled`].
///   2. Otherwise log a "installing cargo-tarpaulin..." notice and
///      call `runner.install`. On success, return
///      [`TarpaulinStatus::JustInstalled`].
///   3. Surface install failures as `Err(reason)` so the caller can
///      decide whether to abort the run (DM3c-bound flows) or
///      continue (DS / DM0-DM2 / etc. that don't use tarpaulin).
///
/// `notify` is invoked exactly once with a human-readable status
/// line so the caller can route it to `eprintln!` (CLI) or a
/// host-event diagnostic (JSONL) without coupling this module to
/// either.
pub fn ensure_tarpaulin_installed(
    runner: &mut dyn Runner,
    mut notify: impl FnMut(&str),
) -> Result<TarpaulinStatus, String> {
    if let Some(version) = runner.probe_version() {
        return Ok(TarpaulinStatus::AlreadyInstalled { version });
    }
    notify(
        "sim-flow: cargo-tarpaulin not found on PATH; running `cargo install cargo-tarpaulin --locked` (this can take a few minutes)...",
    );
    runner.install()?;
    Ok(TarpaulinStatus::JustInstalled)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recording runner so tests can assert how many times each
    /// helper got called and feed in canned responses.
    struct StubRunner {
        version_responses: Vec<Option<String>>,
        version_calls: usize,
        install_response: Result<(), String>,
        install_calls: usize,
    }

    impl StubRunner {
        fn already_installed(version: &str) -> Self {
            Self {
                version_responses: vec![Some(version.to_string())],
                version_calls: 0,
                install_response: Err("install should not be called".into()),
                install_calls: 0,
            }
        }

        fn missing_then_installed() -> Self {
            Self {
                version_responses: vec![None],
                version_calls: 0,
                install_response: Ok(()),
                install_calls: 0,
            }
        }

        fn missing_install_fails() -> Self {
            Self {
                version_responses: vec![None],
                version_calls: 0,
                install_response: Err("network down".into()),
                install_calls: 0,
            }
        }
    }

    impl Runner for StubRunner {
        fn probe_version(&mut self) -> Option<String> {
            let i = self.version_calls;
            self.version_calls += 1;
            self.version_responses.get(i).cloned().unwrap_or_else(|| {
                panic!(
                    "probe_version called {} times beyond canned responses",
                    i + 1
                )
            })
        }

        fn install(&mut self) -> Result<(), String> {
            self.install_calls += 1;
            self.install_response.clone()
        }
    }

    #[test]
    fn already_installed_short_circuits_without_running_install() {
        let mut runner = StubRunner::already_installed("cargo-tarpaulin 0.35.4");
        let mut notes = Vec::<String>::new();
        let result = ensure_tarpaulin_installed(&mut runner, |line| notes.push(line.into()));
        assert_eq!(
            result,
            Ok(TarpaulinStatus::AlreadyInstalled {
                version: "cargo-tarpaulin 0.35.4".into()
            })
        );
        assert_eq!(runner.version_calls, 1);
        assert_eq!(runner.install_calls, 0);
        // No "installing..." chatter when we're already set up.
        assert!(
            notes.is_empty(),
            "expected no notify() calls, got {notes:?}"
        );
    }

    #[test]
    fn missing_triggers_install_and_returns_just_installed() {
        let mut runner = StubRunner::missing_then_installed();
        let mut notes = Vec::<String>::new();
        let result = ensure_tarpaulin_installed(&mut runner, |line| notes.push(line.into()));
        assert_eq!(result, Ok(TarpaulinStatus::JustInstalled));
        assert_eq!(runner.version_calls, 1);
        assert_eq!(runner.install_calls, 1);
        assert_eq!(notes.len(), 1, "expected one notify() call, got {notes:?}");
        assert!(
            notes[0].contains("cargo install cargo-tarpaulin"),
            "notify line should name the command being run, got {:?}",
            notes[0],
        );
    }

    #[test]
    fn install_failure_surfaces_as_err() {
        let mut runner = StubRunner::missing_install_fails();
        let result = ensure_tarpaulin_installed(&mut runner, |_| {});
        assert_eq!(result, Err("network down".into()));
        assert_eq!(runner.install_calls, 1);
    }

    #[test]
    fn already_installed_passes_through_version_string_unchanged() {
        // Some users have multi-line `cargo tarpaulin --version`
        // output (newer versions print build metadata). The helper
        // shouldn't trim past the first line; the caller decides
        // how much to display.
        let raw = "cargo-tarpaulin 0.35.4\nfeatures = []";
        let mut runner = StubRunner::already_installed(raw);
        let result = ensure_tarpaulin_installed(&mut runner, |_| {});
        match result {
            Ok(TarpaulinStatus::AlreadyInstalled { version }) => {
                assert_eq!(version, raw);
            }
            other => panic!("expected AlreadyInstalled, got {other:?}"),
        }
    }
}
