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
//! interleaved notifications. The client is intentionally
//! single-threaded -- all requests serialize on the static
//! `Mutex`. Notifications received while waiting for a response
//! are dropped except for `experimental/serverStatus`, which
//! `wait_for_quiescent` consumes during startup to know when
//! initial indexing has finished.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

const READY_TIMEOUT: Duration = Duration::from_secs(120);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

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

pub type LspResult<T> = std::result::Result<T, LspError>;

pub struct RustAnalyzerClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
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
        let mut client = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
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

    fn read_message_with_deadline(&mut self, _deadline: Instant) -> LspResult<Value> {
        // BufReader doesn't expose a deadline; for Phase 1 we
        // accept a blocking read and rely on rust-analyzer's
        // liveness. If indexing wedges we hit the outer
        // `wait_for_quiescent` timeout via the elapsed check.
        // TODO(phase 2+): swap for a non-blocking read or a
        // reader thread + channel if we observe real wedges.
        if let Some(status) = self.child.try_wait()? {
            return Err(LspError::Protocol(format!(
                "rust-analyzer exited: {status}"
            )));
        }
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line)?;
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
            // Other headers (Content-Type, etc.) are accepted and ignored.
        }
        let len = content_length
            .ok_or_else(|| LspError::Protocol("missing Content-Length header".into()))?;
        let mut buf = vec![0u8; len];
        self.stdout.read_exact(&mut buf)?;
        serde_json::from_slice(&buf).map_err(|e| LspError::Protocol(format!("decode: {e}")))
    }
}

impl Drop for RustAnalyzerClient {
    fn drop(&mut self) {
        // Best-effort graceful shutdown so rust-analyzer doesn't
        // log "client exited without proper shutdown sequence" to
        // stderr. Spec sequence: shutdown request -> exit
        // notification. We don't strictly need the shutdown
        // response, so fire-and-forget on a tight 2s budget.
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
    }
}

fn path_to_uri(p: &Path) -> LspResult<String> {
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

/// Run `f` against the shared `rust-analyzer` client, spawning it
/// against `workspace_root` on first use. The mutex serializes
/// every `api_*` tool call; rust-analyzer handles one request at
/// a time anyway, and the contention is negligible at LLM-turn
/// granularity. If the spawned client was rooted at a different
/// workspace, returns an error -- one client per process for now.
pub fn with_client<F, T>(workspace_root: &Path, f: F) -> LspResult<T>
where
    F: FnOnce(&mut RustAnalyzerClient) -> LspResult<T>,
{
    let mut guard = CLIENT
        .lock()
        .map_err(|_| LspError::Protocol("rust-analyzer client mutex poisoned".into()))?;
    if let Some(existing) = guard.as_ref() {
        if existing.workspace_root() != workspace_root {
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
    f(client)
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
        let msg = read_one(&mut buf).unwrap();
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
        let m1 = read_one(&mut buf).unwrap();
        let m2 = read_one(&mut buf).unwrap();
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
        let msg = read_one(&mut buf).unwrap();
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
        let err = read_one(&mut buf).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("missing Content-Length"), "got: {msg}");
    }

    /// Test-only helper that mirrors `read_message_with_deadline`
    /// against an arbitrary reader. Kept inline so we don't have to
    /// expose the parsing internals on the type itself just for
    /// tests.
    fn read_one<R: BufRead + Read>(r: &mut R) -> LspResult<Value> {
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            let n = r.read_line(&mut line)?;
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
        let len = content_length
            .ok_or_else(|| LspError::Protocol("missing Content-Length header".into()))?;
        let mut buf = vec![0u8; len];
        r.read_exact(&mut buf)?;
        serde_json::from_slice(&buf).map_err(|e| LspError::Protocol(format!("decode: {e}")))
    }
}
