//! `sim-flow` CLI binary.

mod cli;
mod commands;

use std::path::PathBuf;

use clap::Parser;
use cli::Cli;

fn main() {
    // Tracing goes to STDERR. Stdout is reserved for the JSONL
    // session protocol when `sim-flow auto` is driven by an IDE
    // host (the dashboard, e2e_manual, etc.); a tracing line
    // bleeding into stdout would fail to parse as a protocol
    // event and confuse every host. The default
    // `tracing_subscriber::fmt()` writer is stdout, hence the
    // explicit `.with_writer(std::io::stderr)`.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let project_for_log = resolve_project_dir(&cli);
    install_panic_hook(project_for_log.clone());
    let result = commands::run(&cli);
    // Tear down the lazily-spawned rust-analyzer subprocess (if
    // any) before the process exits. Rust doesn't run Drop on
    // statics, so without this call the rust-analyzer client
    // held in the lsp module's static would never receive its
    // shutdown / exit handshake. Idempotent; safe to call from
    // the error path below too.
    sim_flow::__internal::session::lsp::shutdown_client();
    if let Err(err) = result {
        let reason = format!("error: {err}");
        eprintln!("{reason}");
        if let Some(dir) = project_for_log.as_deref() {
            sim_flow::__internal::session::debug_log::append_session_exit_marker(dir, &reason);
        }
        std::process::exit(1);
    }
}

/// Best-effort project directory resolution for the debug-log exit
/// marker. Mirrors `run()`'s logic but never errors. The marker is a
/// diagnostic aid, not part of the contract.
fn resolve_project_dir(cli: &Cli) -> Option<PathBuf> {
    cli.project.clone().or_else(|| std::env::current_dir().ok())
}

/// Install a panic hook that prints the panic to stderr (default
/// behavior, kept for the parent process's stderr capture) AND appends
/// a marker to the session debug log so it doesn't end mid-stream
/// without explanation. The default hook is wrapped so backtraces
/// continue to print when `RUST_BACKTRACE` is set.
fn install_panic_hook(project_dir: Option<PathBuf>) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Best-effort: tear down rust-analyzer before the panic
        // bubbles up. Idempotent so a later `shutdown_client()`
        // in main is a no-op. Done before `default_hook` so the
        // backtrace prints AFTER rust-analyzer is gone (its
        // stderr would otherwise interleave with ours).
        sim_flow::__internal::session::lsp::shutdown_client();
        default_hook(info);
        if let Some(dir) = project_dir.as_deref() {
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "(unknown)".into());
            let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "(non-string panic payload)".into()
            };
            let reason = format!("PANIC at {location}: {payload}");
            sim_flow::__internal::session::debug_log::append_session_exit_marker(dir, &reason);
        }
    }));
}
