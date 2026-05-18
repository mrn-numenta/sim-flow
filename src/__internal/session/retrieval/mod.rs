//! Retrieval service for the Phase 5 agent tools (Chapter 4 §4.6).
//!
//! The orchestrator's tool dispatch is synchronous; LanceDB and the
//! embedder are async. This module bridges that gap. A
//! `RetrievalService` owns a single-threaded tokio runtime, the
//! embedder client, and the framework / spec lance connections (each
//! optional — missing connections degrade gracefully to a structured
//! "index missing" error from the tools rather than a hard failure
//! at construction time).
//!
//! Each retrieval tool holds an `Arc<RetrievalService>` and forwards
//! its queries through the synchronous `*_sync` wrappers; the wrappers
//! `block_on` the underlying async query functions from
//! `lance_index::query`.

pub mod service;

pub use service::{RetrievalError, RetrievalService};
