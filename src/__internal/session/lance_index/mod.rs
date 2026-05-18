//! LanceDB index for sim-flow (Chapter 3 of the spec/lance
//! architecture).
//!
//! This module owns sim-flow's on-disk vector + scalar retrieval
//! store. It holds four tables across two trees:
//!
//! - `~/.sim-flow/lance-index/api/` (shared, framework-level):
//!   `framework_chunks.lance/`.
//! - `<project>/.sim-flow/lance-index/` (per-project, spec-level):
//!   `spec_chunks.lance/`, `signal_table_rows.lance/`,
//!   `cross_spec_refs.lance/`.
//!
//! Each tree carries a `manifest.toml` plus an `embedder.toml`
//! recording the embedder identity used at build time. Queries refuse
//! to run against a tree built with a different embedder.
//!
//! The Lance Rust API is async-only; this module owns its own
//! current-thread tokio runtime for CLI-side build operations and
//! exposes synchronous facades. Read-side query helpers (Chapter 3
//! §3.11) are async; the retrieval-service bridge built in a later
//! phase (Chapter 5 §5.7) calls them through its own runtime.

pub mod build;
pub mod connection;
pub mod lock;
pub mod manifests;
pub mod query;
pub mod schemas;
pub mod staleness;
