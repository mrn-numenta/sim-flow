//! Child-process spawning for cargo / shell runners.
//!
//! Drains stdout + stderr on dedicated reader threads so a child
//! producing more than the OS pipe buffer's worth of output
//! (~64 KiB on Linux) never wedges; polls `try_wait` against a
//! deadline so a hung child surfaces as a timeout error rather than
//! hanging the whole orchestrator.

use std::path::Path;
use std::process::Command;

use crate::{Error, Result};

use super::RunnerOutput;

/// Default per-invocation timeout for cargo / shell runners.
/// Picks 5 minutes because that's safely above the longest
/// observed legitimate `cargo test` (~3 min on the rgb_toy + a
/// margin for lib loads) but tight enough to catch a hung test or
/// a wedged compiler. Override via `SIM_FLOW_CARGO_TIMEOUT_SECS`.
const DEFAULT_RUNNER_TIMEOUT_SECS: u64 = 300;

fn runner_timeout() -> std::time::Duration {
    let secs = std::env::var("SIM_FLOW_CARGO_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_RUNNER_TIMEOUT_SECS);
    std::time::Duration::from_secs(secs)
}

pub(super) fn spawn(project: &Path, cmd: &str, args: &[&str]) -> Result<RunnerOutput> {
    use std::io::Read;
    let mut command = Command::new(cmd);
    command
        .args(args)
        .current_dir(project)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|err| Error::Io {
        path: project.to_path_buf(),
        source: err,
    })?;

    // Drain stdout + stderr on dedicated threads so the child
    // never blocks on `write(stderr)` when its output exceeds the
    // OS pipe buffer (~64 KiB on Linux). Without this, cargo on
    // a real failing build produces well over the buffer in
    // diagnostics, wedges, and we hit the timeout below with
    // empty stdout/stderr because read_to_end only ran AFTER
    // try_wait. See orchestrator audit #3 (2026-05-16).
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_handle = stdout_pipe.map(|mut s| {
        std::thread::spawn(move || -> Vec<u8> {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });
    let stderr_handle = stderr_pipe.map(|mut s| {
        std::thread::spawn(move || -> Vec<u8> {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });

    // Poll for completion with a deadline. We can't use
    // `Command::output()` directly because it blocks until the
    // child exits -- a wedged cargo test would hang the whole
    // orchestrator. Polling at 100ms keeps idle CPU low while
    // still surfacing a hang within a fraction of a second of the
    // timeout.
    let timeout = runner_timeout();
    let started = std::time::Instant::now();
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Reader threads see EOF once kill closes the
                    // pipes; collect what they captured so the
                    // timeout diagnostic is more useful than the
                    // prior "empty" output.
                    let stdout = stdout_handle
                        .and_then(|h| h.join().ok())
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                        .unwrap_or_default();
                    let stderr = stderr_handle
                        .and_then(|h| h.join().ok())
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                        .unwrap_or_default();
                    return Ok(RunnerOutput {
                        command: format_command(cmd, args),
                        stdout_tail: tail(&stdout, 4_000),
                        stderr_tail: format!(
                            "runner timed out after {} seconds (override via SIM_FLOW_CARGO_TIMEOUT_SECS)\n{}",
                            timeout.as_secs(),
                            tail(&stderr, 8_000),
                        ),
                        exit_code: -1,
                    });
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(err) => {
                return Err(Error::Io {
                    path: project.to_path_buf(),
                    source: err,
                });
            }
        }
    };

    let stdout_buf = stdout_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let stderr_buf = stderr_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let stdout = String::from_utf8_lossy(&stdout_buf).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_buf).into_owned();
    let exit_code = exit_status.code().unwrap_or(-1);
    Ok(RunnerOutput {
        command: format_command(cmd, args),
        stdout_tail: tail(&stdout, 4_000),
        stderr_tail: tail(&stderr, 8_000),
        exit_code,
    })
}

pub(super) fn format_command(cmd: &str, args: &[&str]) -> String {
    let mut s = cmd.to_string();
    for a in args {
        s.push(' ');
        s.push_str(a);
    }
    s
}

pub(super) fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let start = s.len() - max;
    let trimmed = s.get(start..).unwrap_or(s);
    format!("...(truncated, last {max} bytes)\n{trimmed}")
}
