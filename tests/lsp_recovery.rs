//! End-to-end test for the LSP client's "evict dead rust-analyzer"
//! behavior. Backs the `api_*` tools (`api_search`, `api_hover`,
//! `api_impls`, `api_references`, `api_expand_macro`).
//!
//! Why a separate test binary: the test points
//! `SIM_FLOW_RUST_ANALYZER` at a fake script and drives the static
//! `CLIENT` mutex through two back-to-back `with_client` calls.
//! Keeping it in its own integration-test file isolates that
//! process-global state from the other integration suites (which
//! never spawn rust-analyzer) so there's no cross-test interference.
//!
//! Unix-only: the fake is a `#!/usr/bin/env sh` script with `chmod
//! +x`. The `api_*` tools haven't been validated on Windows, so we
//! gate this test the same way.

#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sim_flow::session::lsp::{
    __test_client_is_attached, RUST_ANALYZER_BIN_ENV, shutdown_client, with_client,
};

/// Write an executable shell script that records each invocation's
/// PID on a new line in `pid_log` and exits cleanly without reading
/// or writing any LSP frames. Used as a fake rust-analyzer to drive
/// the "subprocess exits immediately" failure path -- the same shape
/// the user saw in production when rust-analyzer died mid-session.
fn write_fake_rust_analyzer(dir: &Path, pid_log: &Path) -> PathBuf {
    let path = dir.join("fake-rust-analyzer.sh");
    let script = format!(
        "#!/usr/bin/env sh\n\
         echo $$ >> {log}\n\
         exit 0\n",
        log = pid_log.display(),
    );
    let mut f = fs::File::create(&path).expect("create fake script");
    f.write_all(script.as_bytes())
        .expect("write fake script body");
    let mut perms = fs::metadata(&path).expect("stat fake").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod fake +x");
    path
}

#[test]
fn dead_rust_analyzer_is_evicted_so_next_call_respawns() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pid_log = tmp.path().join("invocations.log");
    let fake = write_fake_rust_analyzer(tmp.path(), &pid_log);

    // SAFETY: this test runs in its own integration-test binary
    // (`tests/lsp_recovery.rs`), which holds the only `#[test]` in
    // this file, so no other thread is reading SIM_FLOW_RUST_ANALYZER
    // or touching the CLIENT static concurrently.
    unsafe {
        std::env::set_var(RUST_ANALYZER_BIN_ENV, &fake);
    }
    // Make sure no stale client from a previous run survives in the
    // static (irrelevant on a clean test binary, but cheap insurance).
    shutdown_client();

    let ws = tempfile::tempdir().expect("ws tempdir");

    // First call: spawn succeeds, then the fake exits before
    // responding to `initialize`. The handshake surfaces a fatal
    // error -- either `LspError::Exited` (reader thread saw EOF) or
    // `LspError::Protocol("rust-analyzer exited: ...")` (try_wait
    // observed the exit first), or `LspError::Io(BrokenPipe)` (the
    // write to stdin lost the race). All three are fatal for
    // eviction purposes.
    let err1 = with_client(ws.path(), |c| c.workspace_symbol("Anything")).unwrap_err();
    assert!(
        err1.is_fatal(),
        "first-call error should be fatal so the dead client is evicted; got: {err1:?}"
    );

    // The bug this test guards against: before the fix, the dead
    // client stayed in the static and every subsequent `api_*` call
    // replayed the same "subprocess exited unexpectedly" error
    // against the corpse. The fix evicts on fatal errors.
    //
    // Note: in this particular failure shape `start()` itself fails
    // and the static was never populated, so eviction was a no-op on
    // the first call. The real value of the assertion is to nail
    // down the invariant for callers and document the intended state
    // transition -- if a future refactor regresses the static back
    // to "dead client lingers" the assertion fires.
    assert!(
        !__test_client_is_attached(),
        "expected the dead client to be evicted from the static after a fatal error"
    );

    // Second call: must re-spawn the fake (a fresh subprocess) and
    // fail again with another fatal error. Two distinct PIDs in the
    // log prove the second call did NOT reuse the dead client.
    let err2 = with_client(ws.path(), |c| c.workspace_symbol("Anything")).unwrap_err();
    assert!(
        err2.is_fatal(),
        "second-call error should also be fatal; got: {err2:?}"
    );
    assert!(
        !__test_client_is_attached(),
        "static should still be empty after the second fatal error"
    );

    let pids: Vec<u32> = fs::read_to_string(&pid_log)
        .expect("read pid log")
        .lines()
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    assert_eq!(
        pids.len(),
        2,
        "expected exactly two fake-rust-analyzer invocations (one per with_client call); got {pids:?}"
    );
    assert_ne!(
        pids[0], pids[1],
        "two distinct PIDs prove a fresh spawn on the second call rather than a reused-dead-client replay"
    );

    // Tidy: don't leak the env var into anything that might run
    // afterward in the same binary.
    unsafe {
        std::env::remove_var(RUST_ANALYZER_BIN_ENV);
    }
    // Idempotent; also documents that shutdown_client is safe even
    // when the static is already empty.
    shutdown_client();
}
