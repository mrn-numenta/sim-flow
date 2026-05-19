//! Minimal blocking LSP client for `rust-analyzer`.
//!
//! Backs the `api_*` discovery tools (currently `api_search`; more
//! to follow) that let the agent query the live framework workspace
//! instead of reading the static `foundation-docs/api/pages/*.md`
//! snapshot. Scoping notes live in
//! `docs/brainstorming/rust-analyzer-lsp-discovery.md`.
//!
//! Lifecycle: one `rust-analyzer` subprocess per sim-flow process,
//! spawned lazily on the first `api_*` tool call, kept alive for
//! the life of the process. The static `CLIENT` mutex is the
//! single owner. If the binary isn't on `PATH`, the first call
//! returns an `LspError::Spawn` and the tool surfaces a friendly
//! "rust-analyzer unavailable" message; subsequent calls are
//! cached by the static.
//!
//! Threading: rust-analyzer LSP is request/response with optional
//! interleaved notifications. The client has one dedicated reader
//! thread that decodes frames off rust-analyzer's stdout and
//! pushes them over a bounded `sync_channel` to the main thread,
//! which serializes all requests behind the static `Mutex`. The
//! reader thread exists only so the main thread can honor
//! `recv_timeout` deadlines -- a direct blocking read on stdout
//! would ignore the timeout and let a wedged rust-analyzer hang
//! the agent indefinitely. Notifications received while waiting
//! for a response are dropped except for
//! `experimental/serverStatus`, which `wait_for_quiescent`
//! consumes during startup to know when initial indexing has
//! finished.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

/// Maximum wall time for the initial workspace-indexing phase
/// between `initialize` and the first `experimental/serverStatus`
/// notification with `quiescent=true`. Cold-indexing the full
/// sim-foundation workspace observed at ~2 min 30 s on an Apple
/// M-series; 5 min leaves headroom for slower disks / CI without
/// hanging the agent indefinitely. The previous 120 s value
/// "passed" only because the blocking read on stdout didn't
/// actually honor the deadline (see PHASE-3 critique).
const READY_TIMEOUT: Duration = Duration::from_secs(300);

/// Per-request timeout for `workspace/symbol`, `textDocument/*`,
/// and `rust-analyzer/expandMacro`. Once indexing is done these
/// are typically sub-second; the 60 s ceiling exists so a wedged
/// or very-slow response is surfaced as `LspError::Timeout` rather
/// than hanging the agent.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Capacity of the channel between the reader thread (decoding
/// frames off rust-analyzer's stdout) and the main thread
/// (consuming responses/notifications). Bounded so a notification
/// flood during indexing can't grow unbounded, and so the reader
/// thread back-pressures rust-analyzer via the OS pipe buffer when
/// the main thread is slow. 64 is enough to absorb the
/// $/progress + experimental/serverStatus burst during a cold
/// indexing without ever filling up in practice.
const INCOMING_CHANNEL_CAPACITY: usize = 64;

/// Override for the rust-analyzer binary location. Useful when the
/// default `rust-analyzer` on `PATH` is a rustup shim that resolves
/// to a toolchain without the component installed -- pointing at
/// the binary the VS Code rust-analyzer extension already bundles
/// (`~/.vscode/extensions/rust-lang.rust-analyzer-*/server/rust-analyzer`)
/// avoids the install dance entirely.
pub const RUST_ANALYZER_BIN_ENV: &str = "SIM_FLOW_RUST_ANALYZER";

#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("failed to spawn rust-analyzer: {0}")]
    Spawn(std::io::Error),
    #[error("rust-analyzer i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("rust-analyzer protocol error: {0}")]
    Protocol(String),
    #[error("rust-analyzer returned error: {0}")]
    Server(String),
    #[error("rust-analyzer timed out after {0:?}")]
    Timeout(Duration),
    #[error("rust-analyzer subprocess exited unexpectedly")]
    Exited,
}

/// Marker prefix used by `read_message_with_deadline` when
/// `try_wait()` finds rust-analyzer has already exited; surfaced as
/// an `LspError::Protocol` because we want to include the exit
/// status. [`LspError::is_fatal`] looks for this substring to treat
/// such Protocol errors the same as [`LspError::Exited`] for
/// eviction purposes.
const EXITED_PROTOCOL_PREFIX: &str = "rust-analyzer exited:";

impl LspError {
    /// True when this error means the in-process [`RustAnalyzerClient`]
    /// is unusable for further requests -- the subprocess has died,
    /// its stdin pipe is closed, or its stdout has EOF'd. The caller
    /// in [`with_client`] uses this to evict the dead client from the
    /// static mutex so the next `api_*` tool call spawns a fresh
    /// rust-analyzer instead of replaying the same error against a
    /// corpse.
    ///
    /// Non-fatal:
    /// - `Spawn` -- by definition the client never made it into the
    ///   static, so eviction is moot.
    /// - `Server` -- rust-analyzer is alive and well; it just
    ///   declined the request.
    /// - `Timeout` -- the process may still be working through a slow
    ///   request; keep the client alive for the next call.
    /// - `Protocol` for everything other than the
    ///   `"rust-analyzer exited:"` shape -- e.g. a stray bad frame
    ///   we'd rather investigate than re-spawn on every error.
    pub fn is_fatal(&self) -> bool {
        match self {
            LspError::Spawn(_) => false,
            LspError::Io(_) => true,
            LspError::Protocol(msg) => msg.starts_with(EXITED_PROTOCOL_PREFIX),
            LspError::Server(_) => false,
            LspError::Timeout(_) => false,
            LspError::Exited => true,
        }
    }
}

pub type LspResult<T> = std::result::Result<T, LspError>;

pub struct RustAnalyzerClient {
    child: Child,
    stdin: ChildStdin,
    /// Frames decoded by the reader thread arrive here. `Ok(Value)`
    /// for a successfully-parsed JSON-RPC message; `Err(...)` when
    /// rust-analyzer exited or sent something we couldn't parse.
    /// The sender side is owned exclusively by the reader thread,
    /// so a `Disconnected` recv is conclusive evidence the reader
    /// thread has exited (and rust-analyzer's stdout is closed).
    incoming: Receiver<LspResult<Value>>,
    /// Handle to the reader thread. Joined in `Drop` after we kill
    /// the child so its stdout EOFs and the thread's `read_line`
    /// returns. Wrapped in `Option` so `Drop` can `take()` it.
    reader_thread: Option<JoinHandle<()>>,
    next_id: i64,
    workspace_root: PathBuf,
}

impl RustAnalyzerClient {
    /// Spawn `rust-analyzer` rooted at `workspace_root`, run the
    /// LSP handshake (`initialize` + `initialized`), and block
    /// until `experimental/serverStatus` reports `quiescent=true`
    /// (initial indexing complete). Returns `LspError::Timeout`
    /// if indexing exceeds [`READY_TIMEOUT`].
    pub fn start(workspace_root: &Path) -> LspResult<Self> {
        let bin = std::env::var(RUST_ANALYZER_BIN_ENV)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "rust-analyzer".to_string());
        let mut child = Command::new(&bin)
            .current_dir(workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Surface rust-analyzer's own stderr to the orchestrator's
            // stderr so the user sees indexing diagnostics if anything
            // goes wrong. Quiet runs print nothing.
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(LspError::Spawn)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError::Protocol("rust-analyzer stdin missing".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError::Protocol("rust-analyzer stdout missing".into()))?;
        let (tx, rx) = mpsc::sync_channel::<LspResult<Value>>(INCOMING_CHANNEL_CAPACITY);
        let reader_thread = std::thread::Builder::new()
            .name("rust-analyzer-reader".into())
            .spawn(move || reader_loop(BufReader::new(stdout), tx))
            .map_err(LspError::Io)?;
        let mut client = Self {
            child,
            stdin,
            incoming: rx,
            reader_thread: Some(reader_thread),
            next_id: 0,
            workspace_root: workspace_root.to_path_buf(),
        };
        client.handshake()?;
        client.wait_for_quiescent()?;
        Ok(client)
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// `workspace/symbol` request. Returns the raw JSON response
    /// value (either `SymbolInformation[]` or `WorkspaceSymbol[]`
    /// shape depending on what rust-analyzer's capability
    /// advertises; callers normalize).
    pub fn workspace_symbol(&mut self, query: &str) -> LspResult<Value> {
        self.request(
            "workspace/symbol",
            json!({ "query": query }),
            REQUEST_TIMEOUT,
        )
    }

    /// `textDocument/hover` request. `uri` should be a full
    /// `file://` URI (call [`path_to_uri`] if you have a `&Path`);
    /// `line` and `character` are zero-based per the LSP spec.
    /// Returns the raw hover response: `{ contents, range? }` on a
    /// hit, `Null` when rust-analyzer has nothing to say at that
    /// position.
    pub fn text_document_hover(
        &mut self,
        uri: &str,
        line: u64,
        character: u64,
    ) -> LspResult<Value> {
        self.request(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
            }),
            REQUEST_TIMEOUT,
        )
    }

    /// `textDocument/implementation` request. For a position on a
    /// trait declaration, rust-analyzer returns every `impl` of
    /// that trait; for a position on a generic type, it returns
    /// the concrete instantiations. Response is a `Location[]` (or
    /// `Null` when there's nothing to point at).
    pub fn text_document_implementation(
        &mut self,
        uri: &str,
        line: u64,
        character: u64,
    ) -> LspResult<Value> {
        self.request(
            "textDocument/implementation",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
            }),
            REQUEST_TIMEOUT,
        )
    }

    /// `textDocument/references` request. Returns `Location[]` for
    /// every reference to the symbol at the given position.
    /// `include_declaration=true` keeps the definition site in the
    /// result; agents usually want to see it for orientation.
    pub fn text_document_references(
        &mut self,
        uri: &str,
        line: u64,
        character: u64,
        include_declaration: bool,
    ) -> LspResult<Value> {
        self.request(
            "textDocument/references",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
                "context": { "includeDeclaration": include_declaration },
            }),
            REQUEST_TIMEOUT,
        )
    }

    /// `rust-analyzer/expandMacro` extension. Given a position
    /// inside a macro invocation (derive attribute, macro_rules
    /// call, attribute macro, ...) returns
    /// `{ name: string, expansion: string }` -- the macro name and
    /// the expanded source. Returns `Null` when the position isn't
    /// inside a macro call. Documented at
    /// `https://rust-analyzer.github.io/book/contributing/lsp-extensions.html#expand-macro`.
    pub fn rust_analyzer_expand_macro(
        &mut self,
        uri: &str,
        line: u64,
        character: u64,
    ) -> LspResult<Value> {
        self.request(
            "rust-analyzer/expandMacro",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
            }),
            REQUEST_TIMEOUT,
        )
    }

    fn handshake(&mut self) -> LspResult<()> {
        let root_uri = path_to_uri(&self.workspace_root)?;
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "sim-flow", "version": env!("CARGO_PKG_VERSION") },
            "rootUri": root_uri,
            "capabilities": {
                "workspace": {
                    "symbol": {
                        "dynamicRegistration": false,
                        "symbolKind": { "valueSet": (1..=26).collect::<Vec<i32>>() }
                    }
                },
                "textDocument": {
                    "hover": { "contentFormat": ["markdown", "plaintext"] }
                },
                "experimental": {
                    // Lets rust-analyzer send experimental/serverStatus
                    // notifications so wait_for_quiescent can observe
                    // "quiescent" without polling.
                    "serverStatusNotification": true
                }
            },
            "workspaceFolders": [{ "uri": root_uri, "name": "workspace" }]
        });
        let _resp: Value = self.request("initialize", params, READY_TIMEOUT)?;
        self.notify("initialized", json!({}))?;
        Ok(())
    }

    /// Drain notifications until rust-analyzer reports it has
    /// finished initial indexing. The `experimental/serverStatus`
    /// notification (gated by the `serverStatusNotification`
    /// capability we advertise above) carries `{ health, quiescent,
    /// message? }`; we wait for the first one with `quiescent=true`.
    fn wait_for_quiescent(&mut self) -> LspResult<()> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if Instant::now() >= deadline {
                return Err(LspError::Timeout(READY_TIMEOUT));
            }
            let msg = self.read_message_with_deadline(deadline)?;
            if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                if method == "experimental/serverStatus" {
                    let quiescent = msg
                        .get("params")
                        .and_then(|p| p.get("quiescent"))
                        .and_then(|q| q.as_bool())
                        .unwrap_or(false);
                    if quiescent {
                        return Ok(());
                    }
                }
                // Other notifications (window/logMessage,
                // $/progress, etc.) are dropped during startup.
                continue;
            }
            // Stray response with no in-flight request; ignore.
        }
    }

    fn request<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: &str,
        params: P,
        timeout: Duration,
    ) -> LspResult<R> {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_message(&msg)?;
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(LspError::Timeout(timeout));
            }
            let msg = self.read_message_with_deadline(deadline)?;
            let msg_id = msg.get("id").and_then(|v| v.as_i64());
            if msg_id != Some(id) {
                // Notification or unrelated response -- drop.
                continue;
            }
            if let Some(err) = msg.get("error") {
                return Err(LspError::Server(err.to_string()));
            }
            let result = msg.get("result").cloned().unwrap_or(Value::Null);
            return serde_json::from_value(result)
                .map_err(|e| LspError::Protocol(format!("decode {method} result: {e}")));
        }
    }

    fn notify<P: Serialize>(&mut self, method: &str, params: P) -> LspResult<()> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_message(&msg)
    }

    fn write_message(&mut self, msg: &Value) -> LspResult<()> {
        let body =
            serde_json::to_vec(msg).map_err(|e| LspError::Protocol(format!("encode: {e}")))?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len())?;
        self.stdin.write_all(&body)?;
        self.stdin.flush()?;
        Ok(())
    }

    fn read_message_with_deadline(&mut self, deadline: Instant) -> LspResult<Value> {
        // Cheap liveness probe: if rust-analyzer has already
        // exited, surface the exit status (more specific than the
        // `Exited` we'd get once the channel disconnects). The
        // `EXITED_PROTOCOL_PREFIX` keeps `LspError::is_fatal` in sync
        // with whatever status text we tack on here.
        if let Some(status) = self.child.try_wait()? {
            return Err(LspError::Protocol(format!(
                "{EXITED_PROTOCOL_PREFIX} {status}"
            )));
        }
        recv_message(&self.incoming, deadline)
    }
}

/// Receive one message from the reader thread, honoring `deadline`.
/// Returns:
/// - `Ok(value)` when a frame arrives in time.
/// - `Err(LspError::Timeout)` when no frame arrives by `deadline`.
/// - `Err(LspError::Exited)` when the reader thread has dropped its
///   `Sender` (i.e. rust-analyzer's stdout closed and the thread
///   exited).
/// - Whatever error the reader thread sent (e.g. a frame parse
///   failure) is propagated as-is.
fn recv_message(rx: &Receiver<LspResult<Value>>, deadline: Instant) -> LspResult<Value> {
    let now = Instant::now();
    let remaining = deadline.saturating_duration_since(now);
    if remaining.is_zero() {
        return Err(LspError::Timeout(Duration::ZERO));
    }
    match rx.recv_timeout(remaining) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e),
        Err(RecvTimeoutError::Timeout) => Err(LspError::Timeout(remaining)),
        Err(RecvTimeoutError::Disconnected) => Err(LspError::Exited),
    }
}

/// Reader-thread entry point: loop forwarding each decoded frame
/// over `tx` until rust-analyzer's stdout closes (EOF) or a frame
/// fails to parse. The thread exits when either happens or when
/// the main thread drops the `Receiver` (send returns `Err`).
fn reader_loop(mut reader: BufReader<ChildStdout>, tx: SyncSender<LspResult<Value>>) {
    loop {
        match read_frame(&mut reader) {
            Ok(value) => {
                if tx.send(Ok(value)).is_err() {
                    // Main thread dropped the receiver -- we're
                    // being shut down, exit quietly.
                    return;
                }
            }
            Err(e) => {
                // Best-effort report the failure, then exit.
                let _ = tx.send(Err(e));
                return;
            }
        }
    }
}

/// Read one LSP frame (`Content-Length: N\r\n\r\n<N bytes of JSON>`)
/// from `reader` and decode it as a JSON value. Returns
/// `LspError::Exited` on EOF before any header, `LspError::Protocol`
/// on malformed headers or unparseable JSON. Other headers
/// (`Content-Type`, etc.) are accepted and ignored.
fn read_frame<R: BufRead>(reader: &mut R) -> LspResult<Value> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Err(LspError::Exited);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            let len: usize = rest
                .trim()
                .parse()
                .map_err(|e| LspError::Protocol(format!("bad Content-Length: {e}")))?;
            content_length = Some(len);
        }
    }
    let len =
        content_length.ok_or_else(|| LspError::Protocol("missing Content-Length header".into()))?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|e| LspError::Protocol(format!("decode: {e}")))
}

impl Drop for RustAnalyzerClient {
    fn drop(&mut self) {
        // Best-effort graceful shutdown so rust-analyzer doesn't
        // log "client exited without proper shutdown sequence" to
        // stderr. Spec sequence: shutdown request -> exit
        // notification. We don't strictly need the shutdown
        // response, so fire-and-forget.
        self.next_id += 1;
        let id = self.next_id;
        let shutdown = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "shutdown",
            "params": null,
        });
        let _ = self.write_message(&shutdown);
        let _ = self.notify("exit", json!(null));
        let _ = self.child.kill();
        let _ = self.child.wait();
        // After kill+wait, the child's stdout is closed; the
        // reader thread's blocking read_line returns 0 (EOF) and
        // the thread exits. Join so we don't leak it. Drop on a
        // `static Mutex<Option<...>>` is unreachable today, but
        // direct uses of the client elsewhere benefit from clean
        // teardown.
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }
}

/// `file://...` URI for a filesystem path, canonicalized so the
/// URI matches whatever rust-analyzer sees during workspace
/// discovery. Public so tools can convert a path the agent
/// supplied into a URI for `textDocument/*` requests without
/// reimplementing the platform-specific bits.
pub fn path_to_uri(p: &Path) -> LspResult<String> {
    let canonical = p
        .canonicalize()
        .map_err(|e| LspError::Protocol(format!("canonicalize {p:?}: {e}")))?;
    let s = canonical.to_string_lossy();
    // file://<absolute-path> with platform-appropriate separators.
    // rust-analyzer accepts this form on macOS/Linux; the Windows
    // path form would need drive-letter handling, which we don't
    // exercise yet.
    if cfg!(windows) {
        Ok(format!("file:///{}", s.replace('\\', "/")))
    } else {
        Ok(format!("file://{s}"))
    }
}

static CLIENT: Mutex<Option<RustAnalyzerClient>> = Mutex::new(None);

/// Tear down the lazily-spawned `rust-analyzer` subprocess, if any.
/// Idempotent and panic-safe.
///
/// Rust does not run `Drop` on values held inside `static` storage,
/// so the `RustAnalyzerClient::drop` shutdown sequence (LSP
/// `shutdown` -> `exit` -> `kill` -> `wait` -> join reader thread)
/// would otherwise never execute. Without this hook, every
/// sim-flow run that touched an `api_*` tool exits leaving
/// rust-analyzer to log "client exited without proper shutdown
/// sequence" to stderr. Callers in `main` (success path, error
/// path, panic hook) and the signal handler invoke this so the
/// subprocess gets a chance to leave gracefully.
pub fn shutdown_client() {
    // A poisoned mutex still gives us the inner value; we'd
    // rather kill rust-analyzer cleanly than skip cleanup
    // because some unrelated request panicked.
    let mut guard = match CLIENT.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    // `take()` drops the client in place, running its Drop.
    let _ = guard.take();
}

/// Run `f` against the shared `rust-analyzer` client, spawning it
/// against `workspace_root` on first use. The mutex serializes
/// every `api_*` tool call; rust-analyzer handles one request at
/// a time anyway, and the contention is negligible at LLM-turn
/// granularity. If the spawned client was rooted at a different
/// workspace, returns an error -- one client per process for now.
///
/// **Reentrancy contract:** `f` MUST NOT call `with_client`
/// recursively (directly or transitively). The CLIENT mutex is
/// held for the duration of `f`, so a nested call would deadlock
/// on `CLIENT.lock()`. The mutex is `std::sync::Mutex`, not
/// `parking_lot::ReentrantMutex` -- intentionally, because every
/// `api_*` tool today makes one or more sequential requests on
/// the client and never delegates back into the LSP layer.
/// See LSP-discovery post-impl critique #7 (2026-05-16).
///
/// Both `workspace_root` arguments are canonicalized (symlinks
/// resolved, `..` collapsed, trailing slashes normalized) before
/// comparison; otherwise a caller passing `crates/framework/..`
/// or a non-canonical path would erroneously fail the
/// "already-attached" check against an existing client that
/// happened to store the same directory under a different
/// spelling. See LSP-discovery post-impl critique #6
/// (2026-05-16).
pub fn with_client<F, T>(workspace_root: &Path, f: F) -> LspResult<T>
where
    F: FnOnce(&mut RustAnalyzerClient) -> LspResult<T>,
{
    let mut guard = CLIENT
        .lock()
        .map_err(|_| LspError::Protocol("rust-analyzer client mutex poisoned".into()))?;
    if let Some(existing) = guard.as_ref() {
        let stored = canonicalize_or_self(existing.workspace_root());
        let incoming = canonicalize_or_self(workspace_root);
        if stored != incoming {
            return Err(LspError::Protocol(format!(
                "rust-analyzer already attached to {:?}; refusing to re-attach to {:?}",
                existing.workspace_root(),
                workspace_root,
            )));
        }
    } else {
        let client = RustAnalyzerClient::start(workspace_root)?;
        *guard = Some(client);
    }
    let client = guard.as_mut().expect("client populated above");
    let result = f(client);
    // If the operation surfaced a "client unusable" error (subprocess
    // exited, stdin pipe closed, reader-thread EOF), evict the dead
    // client from the static so the next `api_*` tool call re-spawns
    // rust-analyzer instead of replaying the same error against a
    // corpse. Observed in the wild: a single rust-analyzer crash
    // mid-session followed by three back-to-back
    // "subprocess exited unexpectedly" failures because the dead
    // client kept getting reused.
    evict_if_fatal(&mut *guard, &result);
    result
}

/// Drop the contents of `guard` when `result` indicates the in-process
/// client is no longer usable. Extracted into a generic helper so the
/// eviction decision can be unit-tested without constructing a real
/// `RustAnalyzerClient` (which would require spawning a subprocess).
///
/// `Option::take()` runs the contained value's `Drop`, so a real
/// `RustAnalyzerClient` cleans up via [`RustAnalyzerClient::drop`]
/// (best-effort shutdown + kill + reader-thread join); for any other
/// `T` it just clears the slot. The non-fatal branch is a no-op.
fn evict_if_fatal<T, U>(guard: &mut Option<T>, result: &LspResult<U>) {
    if let Err(err) = result
        && err.is_fatal()
    {
        let _ = guard.take();
    }
}

fn canonicalize_or_self(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Test-only peek at whether the static `CLIENT` currently holds a
/// live `RustAnalyzerClient`. Integration tests in `tests/` use this
/// to verify [`with_client`]'s "evict on fatal error" behavior --
/// after an operation that returned a fatal error, the static must
/// be empty so the next call spawns a fresh rust-analyzer.
///
/// Not gated on `#[cfg(test)]` because integration tests live in a
/// separate compilation unit and compile against the non-test build;
/// keeping this helper public-but-undocumented is the standard
/// workaround. The function is otherwise zero-cost.
#[doc(hidden)]
pub fn __test_client_is_attached() -> bool {
    let guard = match CLIENT.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.is_some()
}

/// Spawn rust-analyzer against `workspace_root` in a background
/// thread so the agent's first `api_*` tool call doesn't pay the
/// cold-start indexing tax (2-3 min on a cold sim-foundation
/// workspace). Idempotent: if the client is already attached (or
/// spawn already in flight via a prior call), the new thread is a
/// cheap no-op because `with_client` short-circuits on the
/// already-populated `CLIENT`.
///
/// Errors are logged via `tracing::warn` and swallowed — pre-warm
/// is best-effort. A failed pre-warm just means the eventual
/// `api_*` call still pays the cold-start tax; the user-facing
/// behavior degrades to "no pre-warm" rather than failing the
/// session.
///
/// Returns the spawned thread's `JoinHandle` so callers in tests
/// can synchronize on completion; production callers ignore the
/// handle.
pub fn prewarm(workspace_root: std::path::PathBuf) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("sim-flow:lsp-prewarm".into())
        .spawn(move || {
            // `with_client` with a no-op closure triggers the
            // lazy spawn + initial indexing pass. The result is
            // intentionally discarded; tracing the warning on
            // failure is enough.
            if let Err(e) = with_client(&workspace_root, |_c| Ok(())) {
                tracing::warn!(
                    target: "sim_flow::diagnostics",
                    workspace_root = %workspace_root.display(),
                    error = %e,
                    "LSP pre-warm failed; first api_* call will pay the cold-start tax",
                );
            }
        })
        .expect("thread spawn for LSP pre-warm")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build a single LSP frame: `Content-Length: N\r\n\r\n<body>`.
    fn frame(body: &str) -> Vec<u8> {
        format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
    }

    #[test]
    fn parses_well_formed_frame() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":42}"#;
        let mut buf = BufReader::new(Cursor::new(frame(body)));
        let msg = read_frame(&mut buf).unwrap();
        assert_eq!(msg["id"], 1);
        assert_eq!(msg["result"], 42);
    }

    #[test]
    fn parses_two_frames_back_to_back() {
        let a = r#"{"jsonrpc":"2.0","method":"a"}"#;
        let b = r#"{"jsonrpc":"2.0","method":"b"}"#;
        let mut bytes = frame(a);
        bytes.extend(frame(b));
        let mut buf = BufReader::new(Cursor::new(bytes));
        let m1 = read_frame(&mut buf).unwrap();
        let m2 = read_frame(&mut buf).unwrap();
        assert_eq!(m1["method"], "a");
        assert_eq!(m2["method"], "b");
    }

    #[test]
    fn parses_frame_with_extra_header() {
        let body = r#"{"jsonrpc":"2.0","id":7}"#;
        let bytes = format!(
            "Content-Length: {len}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{body}",
            len = body.len(),
        )
        .into_bytes();
        let mut buf = BufReader::new(Cursor::new(bytes));
        let msg = read_frame(&mut buf).unwrap();
        assert_eq!(msg["id"], 7);
    }

    /// Live end-to-end against rust-analyzer rooted at this very
    /// sim-foundation workspace. `#[ignore]` because it spawns
    /// a heavy subprocess and waits 30-60s for indexing -- not
    /// suitable for the default test run. Invoke with:
    /// `cargo test -p sim-flow -- --ignored live`.
    ///
    /// The hover variant piggybacks on the same client so we only
    /// pay the indexing cost once when running both in the same
    /// process.
    #[test]
    #[ignore]
    fn live_workspace_symbol_and_hover() {
        let here = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = here
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root above tools/sim-flow")
            .to_path_buf();

        let symbols = super::with_client(&workspace_root, |c| c.workspace_symbol("HasLogic"))
            .expect("workspace_symbol HasLogic");
        let arr = symbols.as_array().expect("array");
        assert!(!arr.is_empty(), "expected at least one HasLogic hit");

        let hit = arr
            .iter()
            .find(|item| item.get("name").and_then(|n| n.as_str()) == Some("HasLogic"))
            .expect("an entry literally named HasLogic");
        let uri = hit["location"]["uri"].as_str().unwrap().to_string();
        let line = hit["location"]["range"]["start"]["line"].as_u64().unwrap();
        let character = hit["location"]["range"]["start"]["character"]
            .as_u64()
            .unwrap();

        let hover = super::with_client(&workspace_root, |c| {
            c.text_document_hover(&uri, line, character)
        })
        .expect("hover");
        let value = hover
            .get("contents")
            .and_then(|c| c.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        assert!(
            value.contains("HasLogic"),
            "hover content should mention HasLogic; got: {value}"
        );
        eprintln!(
            "[live] hover head:\n{}",
            value.lines().take(5).collect::<Vec<_>>().join("\n")
        );
    }

    /// Older single-call live test, kept for diagnostic runs in
    /// case `live_workspace_symbol_and_hover` is failing only on
    /// the hover leg. Same invocation guard as above.
    #[test]
    #[ignore]
    fn live_workspace_symbol() {
        // Walk up from this source file: src/__internal/session/lsp.rs
        // -> sim-flow/ -> tools/ -> sim-foundation/.
        let here = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = here
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root above tools/sim-flow")
            .to_path_buf();
        let out = super::with_client(&workspace_root, |c| c.workspace_symbol("HasLogic"))
            .expect("workspace_symbol HasLogic");
        let count = out.as_array().map(|a| a.len()).unwrap_or(0);
        assert!(count > 0, "expected at least one HasLogic hit; got: {out}");
        eprintln!("[live] HasLogic -> {count} hits; first = {}", &out[0]);
    }

    #[test]
    fn rejects_missing_content_length() {
        let bytes = b"\r\n{}".to_vec();
        let mut buf = BufReader::new(Cursor::new(bytes));
        let err = read_frame(&mut buf).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("missing Content-Length"), "got: {msg}");
    }

    // ---- recv_message: deadline + reader-thread contract ----

    #[test]
    fn recv_message_returns_value_when_sender_sent() {
        let (tx, rx) = mpsc::sync_channel::<LspResult<Value>>(1);
        tx.send(Ok(json!({ "id": 1 }))).unwrap();
        let deadline = Instant::now() + Duration::from_millis(50);
        let v = recv_message(&rx, deadline).unwrap();
        assert_eq!(v["id"], 1);
    }

    #[test]
    fn recv_message_returns_timeout_when_nothing_sent() {
        let (_tx, rx) = mpsc::sync_channel::<LspResult<Value>>(1);
        let deadline = Instant::now() + Duration::from_millis(20);
        let err = recv_message(&rx, deadline).unwrap_err();
        assert!(
            matches!(err, LspError::Timeout(_)),
            "expected Timeout, got: {err:?}"
        );
    }

    #[test]
    fn recv_message_returns_timeout_for_already_past_deadline() {
        let (_tx, rx) = mpsc::sync_channel::<LspResult<Value>>(1);
        // Deadline in the past; recv_timeout shouldn't be called
        // with zero duration (we short-circuit), and we surface
        // Timeout immediately.
        let deadline = Instant::now() - Duration::from_secs(1);
        let err = recv_message(&rx, deadline).unwrap_err();
        assert!(
            matches!(err, LspError::Timeout(d) if d == Duration::ZERO),
            "expected Timeout(0), got: {err:?}"
        );
    }

    #[test]
    fn recv_message_returns_exited_when_sender_dropped() {
        let (tx, rx) = mpsc::sync_channel::<LspResult<Value>>(1);
        drop(tx);
        let deadline = Instant::now() + Duration::from_millis(50);
        let err = recv_message(&rx, deadline).unwrap_err();
        assert!(
            matches!(err, LspError::Exited),
            "expected Exited, got: {err:?}"
        );
    }

    #[test]
    fn recv_message_propagates_reader_thread_error() {
        let (tx, rx) = mpsc::sync_channel::<LspResult<Value>>(1);
        tx.send(Err(LspError::Protocol("bad frame".into())))
            .unwrap();
        let deadline = Instant::now() + Duration::from_millis(50);
        let err = recv_message(&rx, deadline).unwrap_err();
        match err {
            LspError::Protocol(s) => assert_eq!(s, "bad frame"),
            other => panic!("expected Protocol, got: {other:?}"),
        }
    }

    // ---- reader_loop end-to-end via an in-memory pipe ----

    /// Drive `reader_loop` against a `BufRead` that wraps an
    /// in-memory byte stream so we can exercise the reader thread
    /// without spawning a real subprocess. We can't use the real
    /// `reader_loop` here because it takes a `BufReader<ChildStdout>`;
    /// instead use the underlying `read_frame` + manual loop, which is
    /// the same shape.
    #[test]
    fn read_frame_then_eof_is_clean() {
        let body = r#"{"id":7}"#;
        let bytes = format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes();
        let mut buf = BufReader::new(Cursor::new(bytes));
        let v = read_frame(&mut buf).unwrap();
        assert_eq!(v["id"], 7);
        let err = read_frame(&mut buf).unwrap_err();
        assert!(matches!(err, LspError::Exited));
    }

    // ---- path_to_uri ----

    #[test]
    fn path_to_uri_renders_existing_path_as_file_url() {
        let tmp = tempfile::tempdir().unwrap();
        let uri = path_to_uri(tmp.path()).expect("uri");
        if cfg!(windows) {
            assert!(uri.starts_with("file:///"));
        } else {
            assert!(uri.starts_with("file://"));
        }
        // The URI must contain the canonical path; tempdir's path
        // resolves through /private on macOS so we check both
        // forms.
        let canonical = tmp.path().canonicalize().unwrap();
        assert!(uri.contains(&canonical.to_string_lossy().to_string()));
    }

    #[test]
    fn path_to_uri_errors_on_nonexistent_path() {
        let result = path_to_uri(Path::new("/no/such/path/here-on-purpose-3f8a"));
        assert!(matches!(result, Err(LspError::Protocol(_))));
    }

    // ---- shutdown_client (idempotency) ----

    // ---- LspError::is_fatal ----

    #[test]
    fn is_fatal_true_for_subprocess_exited_and_io_errors() {
        // Direct exit-detection paths.
        assert!(LspError::Exited.is_fatal());
        // try_wait-based detection emits Protocol with the
        // EXITED_PROTOCOL_PREFIX shape.
        assert!(LspError::Protocol(format!("{EXITED_PROTOCOL_PREFIX} exit code: 0")).is_fatal());
        assert!(LspError::Protocol(format!("{EXITED_PROTOCOL_PREFIX} signal: 9")).is_fatal());
        // Broken pipe / EOF on write surfaces as Io.
        assert!(
            LspError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "stdin closed",
            ))
            .is_fatal()
        );
    }

    #[test]
    fn is_fatal_false_for_transient_or_pre_attach_errors() {
        // Spawn never reaches the populated static so eviction is
        // moot.
        assert!(
            !LspError::Spawn(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no rust-analyzer on PATH",
            ))
            .is_fatal()
        );
        // Server returned a JSON-RPC error -- alive and well.
        assert!(!LspError::Server("method not found".into()).is_fatal());
        // Slow request; the client may still recover.
        assert!(!LspError::Timeout(Duration::from_secs(60)).is_fatal());
        // Generic Protocol error that doesn't look like an exit
        // notification (e.g. malformed frame mid-stream).
        assert!(!LspError::Protocol("missing Content-Length header".into()).is_fatal());
        assert!(!LspError::Protocol("decode: invalid utf-8".into()).is_fatal());
    }

    // ---- evict_if_fatal: the with_client eviction predicate ----

    #[test]
    fn evict_if_fatal_clears_option_on_subprocess_exited() {
        let mut slot: Option<u32> = Some(42);
        evict_if_fatal::<u32, ()>(&mut slot, &Err(LspError::Exited));
        assert!(
            slot.is_none(),
            "fatal Exited should evict the option contents"
        );
    }

    #[test]
    fn evict_if_fatal_clears_option_on_protocol_exit_status() {
        let mut slot: Option<&'static str> = Some("dead client");
        evict_if_fatal::<&'static str, ()>(
            &mut slot,
            &Err(LspError::Protocol(format!(
                "{EXITED_PROTOCOL_PREFIX} exit status: 0"
            ))),
        );
        assert!(slot.is_none(), "exit-status Protocol error should evict");
    }

    #[test]
    fn evict_if_fatal_clears_option_on_io_broken_pipe() {
        let mut slot: Option<u32> = Some(7);
        evict_if_fatal::<u32, ()>(
            &mut slot,
            &Err(LspError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "pipe closed",
            ))),
        );
        assert!(slot.is_none(), "broken-pipe Io should evict");
    }

    #[test]
    fn evict_if_fatal_keeps_option_on_transient_errors() {
        // Timeouts and server-side errors keep the client because
        // the subprocess may still be healthy. Generic Protocol
        // errors (frame parse glitches not tied to a subprocess
        // exit) also keep the client -- if they recur the next call
        // can decide what to do.
        let cases: Vec<LspError> = vec![
            LspError::Timeout(Duration::from_secs(60)),
            LspError::Server("method not found".into()),
            LspError::Protocol("decode: bad json".into()),
        ];
        for err in cases {
            let mut slot: Option<u32> = Some(1);
            evict_if_fatal::<u32, ()>(&mut slot, &Err(err));
            assert!(
                slot.is_some(),
                "expected the option to be preserved across non-fatal errors"
            );
        }
    }

    #[test]
    fn evict_if_fatal_is_a_noop_on_success() {
        let mut slot: Option<u32> = Some(1);
        evict_if_fatal::<u32, u32>(&mut slot, &Ok(99));
        assert_eq!(slot, Some(1));
    }

    #[test]
    fn is_fatal_keys_on_exit_prefix_exactly() {
        // The prefix is the documented signal -- a Protocol error
        // that merely mentions "exited" elsewhere in its body must
        // not be treated as fatal. Defends against accidental
        // false-positive eviction if some other code path ever
        // produces a Protocol error whose body contains "exited".
        assert!(
            !LspError::Protocol("rust-analyzer protocol error: server has exited cleanly".into())
                .is_fatal()
        );
    }

    #[test]
    fn shutdown_client_is_idempotent_with_no_active_client() {
        // The static CLIENT is empty in the unit-test context
        // (no test spawned a real rust-analyzer subprocess). Two
        // back-to-back shutdown_client() calls should both
        // succeed and produce no observable change.
        shutdown_client();
        shutdown_client();
        // No assertion needed; merely reaching this line proves
        // the function returns without panic.
    }

    // ---- read_frame error paths beyond EOF ----

    #[test]
    fn read_frame_bad_content_length_value_errors() {
        let bad = b"Content-Length: not-a-number\r\n\r\n{}".to_vec();
        let mut buf = BufReader::new(Cursor::new(bad));
        let err = read_frame(&mut buf).unwrap_err();
        match err {
            LspError::Protocol(msg) => {
                assert!(msg.contains("bad Content-Length"), "got: {msg}");
            }
            other => panic!("expected Protocol error; got {other:?}"),
        }
    }

    #[test]
    fn read_frame_malformed_json_body_errors() {
        let body = b"{ this is not valid JSON";
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut bytes = header.into_bytes();
        bytes.extend_from_slice(body);
        let mut buf = BufReader::new(Cursor::new(bytes));
        let err = read_frame(&mut buf).unwrap_err();
        match err {
            LspError::Protocol(msg) => assert!(msg.contains("decode"), "got: {msg}"),
            other => panic!("expected Protocol error; got {other:?}"),
        }
    }

    #[test]
    fn read_frame_truncated_body_returns_io_error() {
        // Header says 100 bytes; body is empty. read_exact fails.
        let bytes = b"Content-Length: 100\r\n\r\n".to_vec();
        let mut buf = BufReader::new(Cursor::new(bytes));
        let err = read_frame(&mut buf).unwrap_err();
        assert!(matches!(err, LspError::Io(_)));
    }
}
