//! Structured `spec.md` schema, parser, writer, validator, and
//! traversal.
//!
//! `spec.md` is the design-specification artifact DM0 produces and
//! every downstream consumer (DM1+, critique pass, gate engine,
//! lance build) reads. This module owns the Rust representation,
//! the markdown <-> [`SpecMd`] round-trip, the cross-reference
//! validator, and the manual-mode required-field traversal.
//!
//! The architecture contract lives in
//! `docs/architecture/02-spec-md-schema.md` (referenced as
//! "Chapter 2" throughout). Section ordering, table column
//! conventions, alias rules, and the source-spec anchor format are
//! all specified there.
//!
//! ## Public surface
//!
//! - [`SpecMd`] -- the top-level typed document.
//! - [`parse`] -- markdown bytes to [`SpecMd`]. Returns
//!   [`SpecMdParseError`] on structural errors.
//! - [`SpecMd::to_markdown`] -- inverse of [`parse`]; produces
//!   round-trip stable markdown.
//! - [`SpecMd::validate`] -- cross-reference / required-row checks
//!   producing [`ValidationIssue`] entries.
//! - [`SpecMd::missing_required_fields`] -- template-order
//!   traversal producing [`MissingField`] entries for the
//!   manual-mode Q&A loop (Phase 6).
//! - [`SourceSpecAnchor`] -- the three-form anchor type, with
//!   [`SourceSpecAnchor::parse`] and
//!   [`SourceSpecAnchor::to_anchor_string`] for textual
//!   conversion.
//!
//! ## Round-trip contract
//!
//! `parse(write(parse(input))) == parse(input)` at the typed-struct
//! level. Byte-equal markdown output is NOT a contract; whitespace
//! and column-width policies are implementation detail.
//!
//! ## Out of scope (Phase 1)
//!
//! This module is intentionally not wired into DM0, the lance
//! index, or any prompt. Phases 2 / 6 / 4 own those integrations.

pub mod anchor;
pub mod parser;
pub mod traversal;
pub mod types;
pub mod validate;
pub mod writer;

pub use anchor::{AnchorParseError, AnchorParseReason};
pub use parser::{SpecMdParseError, parse};
pub use traversal::{MissingField, MissingFieldKind};
pub use types::*;
pub use validate::{IssueSeverity, ValidationIssue};
