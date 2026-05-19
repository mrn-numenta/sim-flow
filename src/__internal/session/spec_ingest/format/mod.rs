//! `format.json` semantic descriptor (Architecture Chapter 7).
//!
//! `format.json` is the per-spec content-addressed descriptor that
//! drives downstream chunking, table classification, chrome stripping,
//! and DM0 auto-populate. The full schema is specified in
//! `tools/sim-flow/docs/architecture/07-spec-format-discovery.md`
//! §7.3 and this module's [`descriptor`] submodule mirrors that
//! schema 1:1 so JSON values round-trip without an intermediate
//! shape.
//!
//! Milestone 9.2 ships only the typed descriptor + (de)serialisers +
//! a content-key helper. Skeleton building, first-cut classification,
//! LLM critique, and the deterministic validation post-pass are split
//! across later milestones.

pub mod descriptor;
pub mod skeleton;

pub use descriptor::{
    ChromeEntry, ChromeKind, ColumnMapping, ContentKey, FigureEntry, FigureKind, FigureTarget,
    FontWeight, FormatJson, GlossaryEntry, GlossarySource, Layer, SectionRoleEntry, SpecMdRole,
    TableEntry, TableKind, TableTarget, ValidationBlock, ValidationWarning, WrapStrategy,
};
