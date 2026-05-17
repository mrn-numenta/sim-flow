//! Mid-dispatch cancellation helpers shared by the LLM backends.
//!
//! The orchestrator's `dispatch_with_tools` call blocks on whatever
//! blocking primitive the backend uses (subprocess `wait_with_output`,
//! HTTP `ureq::post(...).call()`, etc.). The dashboard's `Stop`
//! button writes a cancel event to a side-channel control socket
//! that flips a shared `Arc<AtomicBool>`; the helpers here let each
//! backend poll that flag on a short cadence and abort the blocking
//! call.
//!
//! Two flavors:
//!
//! - [`wait_with_cancel`]: spawns the child, then polls `try_wait`
//!   alongside the cancel flag. On flip, sends `SIGTERM` to the
//!   child's pid (via `libc::kill`) and returns `Error::Cancelled`.
//!   The Unix-only signal path is fine because sim-flow is already
//!   Unix-only by virtue of the `UnixListener`-based protocol socket.
//!
//! - [`run_cancellable`]: hands a synchronous blocking call (the
//!   ureq HTTP path is the typical caller) off to a worker thread,
//!   then `recv_timeout`-polls a channel for the result while also
//!   polling the cancel flag. On cancel the worker thread is
//!   abandoned -- its in-flight network call eventually completes
//!   and its result is silently dropped. That's an acceptable
//!   trade-off for the first cut: we get IMMEDIATE responsiveness
//!   from the orchestrator's perspective without restructuring the
//!   underlying HTTP transport (`ureq` has no built-in cancellation
//!   handle).
//!
//! Both helpers accept `Option<Arc<AtomicBool>>` so callers that
//! haven't wired a cancel channel yet (tests, in-process unit tests)
//! degrade gracefully to "no cancellation, just run to completion."

use std::process::{Child, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::{Error, Result};

/// Polling cadence for cancel-flag checks. Picked to balance
/// responsiveness against wakeup overhead; 50 ms is the same
/// cadence the existing `cap_exceeded_flag` watcher in auto.rs uses
/// for its coordinator → worker fan-in path.
const CANCEL_POLL_MS: u64 = 50;

/// Run a child process to completion, but tear it down with `SIGTERM`
/// if the shared cancel flag flips first. Mirrors
/// `Child::wait_with_output` for the no-cancel path -- streams
/// stdout / stderr on background threads so the child can't deadlock
/// on a full pipe buffer -- and returns the same `Output` shape on
/// clean completion.
///
/// `cancel_flag = None` skips cancellation entirely (equivalent to
/// the existing `child.wait_with_output()` behavior) so test paths
/// and code that hasn't wired a control socket yet keep working.
pub(crate) fn wait_with_cancel(
    mut child: Child,
    cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<Output> {
    // Drop stdin if the caller left it open; symmetric with
    // wait_with_output, which would otherwise block waiting for
    // pipe close.
    let _ = child.stdin.take();
    let pid = child.id();

    // Read stdout / stderr concurrently so the child can't block on
    // a full pipe buffer while we wait for it to exit.
    let stdout_handle = child.stdout.take().map(|mut s| {
        std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
            use std::io::Read;
            let mut buf = Vec::new();
            s.read_to_end(&mut buf)?;
            Ok(buf)
        })
    });
    let stderr_handle = child.stderr.take().map(|mut s| {
        std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
            use std::io::Read;
            let mut buf = Vec::new();
            s.read_to_end(&mut buf)?;
            Ok(buf)
        })
    });

    let cancelled = loop {
        match child.try_wait() {
            Ok(Some(_status)) => break false,
            Ok(None) => {
                if let Some(ref flag) = cancel_flag
                    && flag.load(Ordering::Acquire)
                {
                    // SIGTERM the child; let it shut down its IPC /
                    // cleanup paths before falling through to wait().
                    // SIGKILL would skip flush but `claude` / `codex`
                    // are usually well-behaved on SIGTERM.
                    // SAFETY: `pid` came from a still-running child;
                    // libc::kill on a non-existent pid is harmless
                    // (returns ESRCH).
                    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                    let _ = child.wait();
                    break true;
                }
                std::thread::sleep(Duration::from_millis(CANCEL_POLL_MS));
            }
            Err(err) => {
                return Err(Error::Llm(format!("wait for child process failed: {err}")));
            }
        }
    };

    if cancelled {
        return Err(Error::Cancelled);
    }

    // Drain the stdout / stderr reader threads. join() yields the
    // closure's `io::Result<Vec<u8>>`; map both the panic case
    // (Err from join) and the inner io::Error into `Error::Llm`
    // explicitly because the crate-wide `Error` has no
    // `From<std::io::Error>` impl.
    let stdout = match stdout_handle {
        Some(h) => h
            .join()
            .map_err(|_| Error::Llm("stdout reader thread panicked".into()))?
            .map_err(|err| Error::Llm(format!("stdout read failed: {err}")))?,
        None => Vec::new(),
    };
    let stderr = match stderr_handle {
        Some(h) => h
            .join()
            .map_err(|_| Error::Llm("stderr reader thread panicked".into()))?
            .map_err(|err| Error::Llm(format!("stderr read failed: {err}")))?,
        None => Vec::new(),
    };

    // We already consumed `status` via try_wait -- but Child::wait()
    // tries to consume again. Use try_wait one more time; the child
    // already exited so it returns Some immediately.
    let status = child
        .try_wait()
        .map_err(|err| Error::Llm(format!("status fetch: {err}")))?
        .ok_or_else(|| {
            Error::Llm("child exited but try_wait returned None on second call".into())
        })?;

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Run a synchronous blocking call on a worker thread and select
/// between its result and the cancel flag. On cancel the worker is
/// abandoned -- its result, when it eventually arrives, is dropped.
/// Suitable for HTTP transports (e.g. `ureq`) that don't expose a
/// cancellation handle.
///
/// `cancel_flag = None` makes this a straight `f()` call on a
/// worker thread; the channel select still happens but the cancel
/// branch never fires.
pub(crate) fn run_cancellable<T, F>(cancel_flag: Option<Arc<AtomicBool>>, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = f();
        // Best-effort: the receiver may have already abandoned us
        // via the cancel branch. Drop the SendError silently in
        // that case -- nothing to do, the result is discarded.
        let _ = tx.send(result);
    });

    loop {
        if let Some(ref flag) = cancel_flag
            && flag.load(Ordering::Acquire)
        {
            return Err(Error::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(CANCEL_POLL_MS)) {
            Ok(result) => return result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // The worker dropped its sender without sending --
                // either it panicked or the future was poisoned.
                // Surface as Llm rather than Cancelled because the
                // cancel branch handles its own return above.
                return Err(Error::Llm(
                    "LLM dispatcher worker thread terminated without producing a result".into(),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_cancellable_no_flag_passes_result_through() {
        let result: Result<u32> = run_cancellable(None, || Ok(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn run_cancellable_propagates_error_from_worker() {
        let result: Result<u32> = run_cancellable(None, || Err(Error::Llm("worker failed".into())));
        match result {
            Err(Error::Llm(msg)) => assert!(msg.contains("worker failed")),
            other => panic!("expected Llm error, got {other:?}"),
        }
    }

    #[test]
    fn run_cancellable_returns_cancelled_when_flag_flips_during_long_call() {
        let flag = Arc::new(AtomicBool::new(false));
        let flag_for_setter = flag.clone();
        // Setter thread flips the flag after a tiny delay so the
        // poll loop has a chance to observe it.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            flag_for_setter.store(true, Ordering::Release);
        });
        let result: Result<u32> = run_cancellable(Some(flag), || {
            // Long enough that the cancel branch wins the select.
            std::thread::sleep(Duration::from_secs(5));
            Ok(0)
        });
        assert!(matches!(result, Err(Error::Cancelled)));
    }
}
