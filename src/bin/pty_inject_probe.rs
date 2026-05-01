//! Iterate through PTY-inject strategies against a real `claude`
//! interactive session and report which one(s) actually cause claude
//! to submit the prompt instead of leaving it in the input field
//! waiting for the user to hit Enter.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p sim-flow --bin pty_inject_probe -- --prompt "hello"
//! ```
//!
//! For each strategy: spawn a fresh `claude` PTY, wait for the TUI to
//! settle, take a baseline snapshot of bytes already on the wire,
//! apply the strategy to inject the prompt, capture stdout for
//! `--wait-secs` seconds, kill claude, and write the captured bytes
//! to `/tmp/pty-inject-probe-<id>.log`. Strategies are ranked by:
//!
//!   1. Whether the captured tail contains a likely-response marker
//!      (`Let me`, `Hello`, `Hi`, `I'll`, etc.).
//!   2. Total bytes captured after the inject (responses produce
//!      visibly more output than idle TUI rendering).
//!
//! The user can then either trust the auto-pick or visually grep the
//! per-strategy logs for confirmation.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use sim_flow::session::agent::interactive_pty::{InteractivePtySession, PtyWriter};

/// One inject strategy to try. `apply` writes the prompt + whatever
/// suffix this strategy considers "Enter" to the PTY's stdin.
struct Strategy {
    id: &'static str,
    description: &'static str,
    apply: fn(&PtyWriter, &str) -> std::io::Result<()>,
}

fn strategies() -> Vec<Strategy> {
    vec![
        Strategy {
            id: "lf",
            description: "body + LF (\\n)",
            apply: |w, body| {
                w.write_bytes(body.as_bytes()).map_err(io_err)?;
                w.write_bytes(b"\n").map_err(io_err)
            },
        },
        Strategy {
            id: "cr",
            description: "body + CR (\\r)",
            apply: |w, body| {
                w.write_bytes(body.as_bytes()).map_err(io_err)?;
                w.write_bytes(b"\r").map_err(io_err)
            },
        },
        Strategy {
            id: "crlf",
            description: "body + CRLF (\\r\\n)",
            apply: |w, body| {
                w.write_bytes(body.as_bytes()).map_err(io_err)?;
                w.write_bytes(b"\r\n").map_err(io_err)
            },
        },
        Strategy {
            id: "body-pause-cr",
            description: "body, sleep 250ms, CR",
            apply: |w, body| {
                w.write_bytes(body.as_bytes()).map_err(io_err)?;
                thread::sleep(Duration::from_millis(250));
                w.write_bytes(b"\r").map_err(io_err)
            },
        },
        Strategy {
            id: "bracketed-paste-cr",
            description: "ESC[200~ body ESC[201~, sleep 250ms, CR",
            apply: |w, body| {
                w.write_bytes(b"\x1b[200~").map_err(io_err)?;
                w.write_bytes(body.as_bytes()).map_err(io_err)?;
                w.write_bytes(b"\x1b[201~").map_err(io_err)?;
                thread::sleep(Duration::from_millis(250));
                w.write_bytes(b"\r").map_err(io_err)
            },
        },
        Strategy {
            id: "bracketed-paste-cr-cr",
            description: "ESC[200~ body ESC[201~, sleep 250ms, CR, sleep 100ms, CR",
            apply: |w, body| {
                w.write_bytes(b"\x1b[200~").map_err(io_err)?;
                w.write_bytes(body.as_bytes()).map_err(io_err)?;
                w.write_bytes(b"\x1b[201~").map_err(io_err)?;
                thread::sleep(Duration::from_millis(250));
                w.write_bytes(b"\r").map_err(io_err)?;
                thread::sleep(Duration::from_millis(100));
                w.write_bytes(b"\r").map_err(io_err)
            },
        },
        Strategy {
            id: "char-by-char-cr",
            description: "type body byte-by-byte (5ms between), sleep 250ms, CR",
            apply: |w, body| {
                for ch in body.bytes() {
                    w.write_bytes(&[ch]).map_err(io_err)?;
                    thread::sleep(Duration::from_millis(5));
                }
                thread::sleep(Duration::from_millis(250));
                w.write_bytes(b"\r").map_err(io_err)
            },
        },
        Strategy {
            id: "ctrl-enter",
            description: "body, sleep 250ms, xterm Ctrl+Enter (\\x1b[27;5;13~)",
            apply: |w, body| {
                w.write_bytes(body.as_bytes()).map_err(io_err)?;
                thread::sleep(Duration::from_millis(250));
                w.write_bytes(b"\x1b[27;5;13~").map_err(io_err)
            },
        },
        Strategy {
            id: "shift-enter",
            description: "body, sleep 250ms, xterm Shift+Enter (\\x1b[27;2;13~)",
            apply: |w, body| {
                w.write_bytes(body.as_bytes()).map_err(io_err)?;
                thread::sleep(Duration::from_millis(250));
                w.write_bytes(b"\x1b[27;2;13~").map_err(io_err)
            },
        },
        Strategy {
            id: "alt-enter",
            description: "body, sleep 250ms, ESC + CR (Alt+Enter)",
            apply: |w, body| {
                w.write_bytes(body.as_bytes()).map_err(io_err)?;
                thread::sleep(Duration::from_millis(250));
                w.write_bytes(b"\x1b\r").map_err(io_err)
            },
        },
    ]
}

/// Markers we look for in the captured stdout that suggest claude
/// generated a response to "hello". These are cheap-and-cheerful
/// heuristics, not a guarantee.
const RESPONSE_MARKERS: &[&str] = &[
    "Hello",
    "Hi!",
    "Hi,",
    "I'll",
    "I'm",
    "Let me",
    "Sure",
    "Of course",
    "How can I",
    "How may I",
];

#[derive(Debug)]
#[allow(dead_code)] // `apply_failed` is for future use; keep silent now.
struct StrategyResult {
    id: &'static str,
    description: &'static str,
    bytes_after_inject: usize,
    found_markers: Vec<&'static str>,
    log_path: PathBuf,
    apply_failed: bool,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let prompt = parse_arg(&args, "--prompt").unwrap_or_else(|| "hello".to_string());
    let wait_secs: u64 = parse_arg(&args, "--wait-secs")
        .and_then(|s| s.parse().ok())
        .unwrap_or(15);
    let settle_secs: u64 = parse_arg(&args, "--settle-secs")
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    let model: Option<String> = parse_arg(&args, "--model");

    let strategies = strategies();
    let only_id: Option<String> = parse_arg(&args, "--only");

    eprintln!(
        "pty_inject_probe: prompt={:?} wait={}s settle={}s model={:?}",
        prompt,
        wait_secs,
        settle_secs,
        model.as_deref().unwrap_or("(default)")
    );
    eprintln!("Running {} strategies...", strategies.len());
    eprintln!();

    let mut results: Vec<StrategyResult> = Vec::new();
    for strategy in &strategies {
        if let Some(only) = &only_id
            && only != strategy.id
        {
            continue;
        }
        eprintln!("--- {} ({}) ---", strategy.id, strategy.description);
        match run_one(strategy, &prompt, wait_secs, settle_secs, model.as_deref()) {
            Ok(r) => {
                eprintln!(
                    "    bytes after inject: {}, markers: {:?}, log: {}",
                    r.bytes_after_inject,
                    r.found_markers,
                    r.log_path.display()
                );
                results.push(r);
            }
            Err(err) => {
                eprintln!("    error: {err}");
            }
        }
        eprintln!();
    }

    // Rank: prefer strategies that hit response markers; tie-break by
    // bytes_after_inject (more output -> more likely a real response).
    results.sort_by(|a, b| {
        let a_score = a.found_markers.len();
        let b_score = b.found_markers.len();
        b_score
            .cmp(&a_score)
            .then(b.bytes_after_inject.cmp(&a.bytes_after_inject))
    });

    eprintln!("=========================================================");
    eprintln!("RANKING (likely-best first):");
    for (i, r) in results.iter().enumerate() {
        let badge = if !r.found_markers.is_empty() {
            "✅"
        } else if r.bytes_after_inject > 4096 {
            "?"
        } else {
            "✗"
        };
        eprintln!(
            "  {} {:>2}. [{}] {} -- {} bytes; markers: {:?}",
            badge,
            i + 1,
            r.id,
            r.description,
            r.bytes_after_inject,
            r.found_markers,
        );
    }
    eprintln!();
    eprintln!(
        "Inspect /tmp/pty-inject-probe-<id>.log for the captured byte stream of any strategy."
    );
}

fn run_one(
    strategy: &Strategy,
    prompt: &str,
    wait_secs: u64,
    settle_secs: u64,
    model: Option<&str>,
) -> std::io::Result<StrategyResult> {
    let mut argv: Vec<String> = vec!["claude".into()];
    if let Some(m) = model {
        argv.push("--model".into());
        argv.push(m.into());
    }
    let mut session = InteractivePtySession::new(argv, None, Vec::<String>::new());
    session.spawn().map_err(|e| io_err(e.to_string()))?;
    let writer = session.writer().map_err(|e| io_err(e.to_string()))?;
    let reader = session.take_reader().map_err(|e| io_err(e.to_string()))?;

    // Drain reader into a shared buffer in a background thread; tag
    // the byte index where we apply the inject so we can slice
    // "before" / "after" cleanly.
    let buffer: Arc<std::sync::Mutex<Vec<u8>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let buf_for_thread = buffer.clone();
    let stop_for_thread = stop_flag.clone();
    let reader_thread = thread::spawn(move || {
        let mut reader = reader;
        let mut chunk = [0u8; 4096];
        loop {
            if stop_for_thread.load(Ordering::Relaxed) {
                break;
            }
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut buf) = buf_for_thread.lock() {
                        buf.extend_from_slice(&chunk[..n]);
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Settle window so claude renders its initial UI before we touch
    // stdin. Without this the inject can race the TUI startup and
    // arrive in the wrong state.
    thread::sleep(Duration::from_secs(settle_secs));
    let inject_offset = buffer.lock().map(|b| b.len()).unwrap_or(0);

    // Apply the strategy.
    let apply_failed = (strategy.apply)(&writer, prompt).is_err();

    // Wait for response.
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(wait_secs) {
        thread::sleep(Duration::from_millis(200));
    }

    // Tear down.
    session.kill();
    stop_flag.store(true, Ordering::Relaxed);
    let _ = reader_thread.join();

    let captured = buffer.lock().map(|b| b.clone()).unwrap_or_default();
    let after = if captured.len() > inject_offset {
        &captured[inject_offset..]
    } else {
        &[]
    };
    let after_text = String::from_utf8_lossy(after);

    // Strip ANSI escapes so the marker grep doesn't false-positive on
    // claude's own UI strings inside escape sequences.
    let stripped = strip_ansi(&after_text);
    let mut found_markers: Vec<&'static str> = Vec::new();
    for marker in RESPONSE_MARKERS {
        if stripped.contains(marker) {
            found_markers.push(marker);
        }
    }

    let log_path = PathBuf::from(format!("/tmp/pty-inject-probe-{}.log", strategy.id));
    let mut f = std::fs::File::create(&log_path)?;
    f.write_all(&captured)?;

    Ok(StrategyResult {
        id: strategy.id,
        description: strategy.description,
        bytes_after_inject: after.len(),
        found_markers,
        log_path,
        apply_failed,
    })
}

fn strip_ansi(text: &str) -> String {
    // Cheap ANSI stripper: drop ESC + `[...<final>` sequences and bare
    // ESC + single char. Good enough to defang claude's TUI rendering
    // for marker matching.
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == 0x1b as char {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() || c == '~' {
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn parse_arg(argv: &[String], flag: &str) -> Option<String> {
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter.next().cloned();
        }
        if let Some(rest) = arg.strip_prefix(&format!("{flag}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

fn io_err<E: std::fmt::Display>(err: E) -> std::io::Error {
    std::io::Error::other(err.to_string())
}
