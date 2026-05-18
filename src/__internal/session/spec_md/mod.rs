//! Structured `spec.md` schema, parser, and writer.
//!
//! `spec.md` is the design-specification artifact DM0 produces and
//! DM1+ / critique / gate engine all consume. This module owns the
//! Rust representation:
//!
//! - [`types`] -- the typed struct hierarchy mirroring §2.2 of the
//!   architecture chapter.
//! - [`parser`] -- markdown to [`SpecMd`].
//! - `writer` -- [`SpecMd`] to markdown. (Phase 1.15.)
//! - `traversal` -- required-field walking for the manual-mode Q&A
//!   loop. (Phase 1.18.)
//! - `validate` -- cross-reference and required-row checks. (Phase
//!   1.14.)
//!
//! Phases 2, 6, and 8 consume this module; nothing here is wired
//! into DM0, the lance index, or any prompt yet.

pub mod anchor;
pub mod parser;
pub mod types;
pub mod validate;

pub use anchor::{AnchorParseError, AnchorParseReason};
pub use parser::{SpecMdParseError, parse};
pub use types::*;
pub use validate::{IssueSeverity, ValidationIssue};
