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

/// Enforce-on-write contract for `docs/spec.md`.
///
/// Both `write_file` and `edit_file` call this with the PROPOSED
/// content before flushing it to disk. If the content parses
/// cleanly AND every cross-ref / quantitative-row / anchor check
/// in [`SpecMd::validate`] passes, the function returns `Ok(())`
/// and the caller commits the write. If parse or validation
/// fails, the function returns a structured `Err` listing every
/// violation; the caller refuses the write and surfaces the
/// errors to the agent.
///
/// The intent is to make `docs/spec.md`'s structured schema
/// *enforced*, not advisory: an agent cannot land a spec.md that
/// trips the DM0 gate's structural / anchor / quantitative-row
/// validators. Mirrors the validation the gate would run, but
/// fires per-write so the agent learns about violations
/// immediately rather than at the next critique cycle.
///
/// Callers should compose the returned `Err(Vec<ValidationIssue>)`
/// into a single error message; see the `write_file` /
/// `edit_file` tools for the rendering convention.
pub fn validate_proposed_spec_md(content: &str) -> std::result::Result<(), SpecMdWriteError> {
    let spec = parser::parse(content).map_err(SpecMdWriteError::Parse)?;

    // Required-H2-section check. The parser silently ignores
    // unknown H2s for forward-compat, so `## Purpose And Scope`
    // (a popular agent-invented merge of three required sections)
    // parses cleanly with empty Purpose / Scope / Non-goals
    // fields and `spec.validate()` doesn't object. Catch the
    // missing-required-section case here before the file lands
    // on disk.
    let present_h2s: std::collections::HashSet<String> = parser::segment_document(content)
        .into_iter()
        .filter_map(|seg| match seg {
            parser::Segment::Section(s) => Some(s.heading),
            _ => None,
        })
        .collect();
    let missing: Vec<&'static str> = REQUIRED_H2_SECTIONS
        .iter()
        .filter(|name| !present_h2s.contains(**name))
        .copied()
        .collect();
    if !missing.is_empty() {
        return Err(SpecMdWriteError::MissingRequiredSections(missing));
    }

    let issues = spec.validate();
    if issues.iter().any(|i| i.severity == IssueSeverity::Error) {
        return Err(SpecMdWriteError::Validate(issues));
    }
    Ok(())
}

/// Canonical list of H2 section headings every `docs/spec.md` MUST
/// carry. Order is the schema's documented order (see the DM0
/// work prompt's "Required sections" list), but the guard checks
/// presence not order — re-ordering would be a separate prompt
/// rule, not a structural validator concern. Strings match the
/// exact text the parser's `dispatch_section` keys on.
pub const REQUIRED_H2_SECTIONS: &[&str] = &[
    "Metadata",
    "Purpose",
    "Scope",
    "Non-goals",
    "Assumptions and Constraints",
    "Blocks",
    "Functional Behavior",
    "Timing, Latency, and Throughput",
    "Pipeline and Hierarchy",
    "Reset, Initialization, Flush, Drain",
    "Worked Examples",
    "Source-Spec Anchors",
    "Open Questions",
    "Auto-decisions",
];

/// Reasons the write-time validator can reject a proposed
/// `docs/spec.md` body.
#[derive(Debug)]
pub enum SpecMdWriteError {
    /// The proposed content didn't parse against the structured
    /// schema. The agent must fix headings / table shapes /
    /// section order before the write can succeed.
    Parse(SpecMdParseError),
    /// One or more required H2 sections were missing from the
    /// proposed content. Carries the missing canonical names.
    /// Distinct from `Validate` so the agent's error message
    /// directly names what to add rather than burying the issue
    /// inside a cross-ref check.
    MissingRequiredSections(Vec<&'static str>),
    /// The content parsed but cross-reference, anchor, or
    /// quantitative-row checks failed. Carries every issue so the
    /// agent can fix them in one round.
    Validate(Vec<ValidationIssue>),
}

impl SpecMdWriteError {
    /// Flatten the error into a sequence of `(section, title, body)`
    /// triples suitable for emitting as Phase 1 critique findings.
    /// Each entry is a BLOCKER-class issue (the LLM critique would
    /// have to address it before advancement either way; making it
    /// a BLOCKER lets the deterministic phase short-circuit the
    /// auto-loop's "retry until clean" path with a clear signal).
    ///
    /// The fields are deliberately chosen to round-trip cleanly
    /// into `crate::critique::CritiqueFinding`:
    ///
    /// - `section` is a human-readable label so the rendered
    ///   markdown view groups related findings together.
    /// - `title` is the one-line summary the markdown row leads
    ///   with.
    /// - `body` carries the remediation hint + the original
    ///   location detail so the agent has everything it needs in
    ///   one place.
    pub fn to_phase1_findings(&self) -> Vec<(String, String, String)> {
        match self {
            SpecMdWriteError::Parse(err) => vec![(
                "Structural parse".into(),
                "docs/spec.md fails the structured parser".into(),
                format!(
                    "Parser error: {err}.\n\nThe structured schema in DM0's prompt is mandatory: every required H2 must be present in order, tables must have the documented columns, and headings must match the canonical strings exactly. Fix the parse error and re-write the file."
                ),
            )],
            SpecMdWriteError::MissingRequiredSections(missing) => missing
                .iter()
                .map(|name| {
                    (
                        "Required H2 missing".into(),
                        format!("`## {name}` is missing"),
                        format!(
                            "Add a `## {name}` heading as its OWN H2 (no merging like `## Purpose And Scope`, no case changes). The schema requires every section in `REQUIRED_H2_SECTIONS` to be present."
                        ),
                    )
                })
                .collect(),
            SpecMdWriteError::Validate(issues) => issues
                .iter()
                .map(|issue| {
                    (
                        "Validation".into(),
                        issue.location.clone(),
                        issue.message.clone(),
                    )
                })
                .collect(),
        }
    }

    /// Render the error into a user-facing message suitable for
    /// returning to the agent as a `ToolResult::err` body. Format
    /// is "spec.md validation failed (N issue(s)):" + a bulleted
    /// list of every failure, with locations.
    pub fn to_agent_message(&self) -> String {
        match self {
            SpecMdWriteError::Parse(err) => format!(
                "docs/spec.md rejected: parse failed — {err}. The structured schema \
                 in DM0's prompt is mandatory: every required H2 must be present in \
                 order, tables must have the documented columns, and headings must \
                 match the canonical strings exactly. Fix and retry."
            ),
            SpecMdWriteError::MissingRequiredSections(missing) => {
                let mut out = format!(
                    "docs/spec.md rejected: {} required H2 section(s) missing:\n",
                    missing.len()
                );
                for name in missing {
                    out.push_str(&format!("  - `## {name}`\n"));
                }
                out.push_str(
                    "\nEvery required H2 must be present as its OWN section with the \
                     EXACT heading string above (no merging like `## Purpose And \
                     Scope`, no renaming, no case changes). Add the missing sections \
                     and retry. The write was not persisted; the file on disk is \
                     unchanged.",
                );
                out
            }
            SpecMdWriteError::Validate(issues) => {
                let mut out = format!(
                    "docs/spec.md rejected: {} validation error(s):\n",
                    issues.len()
                );
                for issue in issues {
                    out.push_str(&format!("  - {} :: {}\n", issue.location, issue.message));
                }
                out.push_str(
                    "\nFix every issue above and retry. The write was not persisted; \
                     the file on disk is unchanged.",
                );
                out
            }
        }
    }
}
