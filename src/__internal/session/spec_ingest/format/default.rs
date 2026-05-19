//! Built-in default `format.json` descriptor (Chapter 7 §7.5).
//!
//! Returned by [`default_descriptor`] when `--no-format-discovery`
//! is set and no cached `format.json` exists for the input spec.
//! The default carries no per-spec knowledge — it is the schema
//! scaffolding only. With this descriptor in place:
//!
//! - Sections fall through to the parser's default heading
//!   classification (`prose` / `unknown`).
//! - Tables fall through to the heuristic extractors in
//!   `classify.rs` (the no-format path).
//! - Chrome falls through to the positional Y-band detection in
//!   `chrome.rs` (existing repeated-line behaviour).
//!
//! In short, `--no-format-discovery` with this descriptor is
//! equivalent to "run pdf_oxide detection + heuristic-only
//! classification" — the existing behaviour after the Phase 9
//! foundation merge.
//!
//! # Future extension hooks
//!
//! TODO(phase-9+): the default descriptor will be extended in
//! future phases to ship built-in patterns for common spec
//! conventions — e.g. the `### Signal Direction To/From
//! Description` markdown table style that the existing
//! heuristic extractors already recognise. v1 ships empty so the
//! `--no-format-discovery` path is provably no-op relative to
//! today's behaviour; per-spec heuristics layer on after the
//! first-cut classifier (milestone 9.4) is wired through.

use chrono::{DateTime, TimeZone, Utc};

use super::descriptor::{FormatJson, ValidationBlock};

/// Sentinel `model` value identifying a descriptor that the LLM
/// pipeline never touched. Used by callers (and humans grepping
/// the JSON) to tell built-in defaults apart from real
/// discoveries.
pub const DEFAULT_MODEL: &str = "default-builtin";

/// Prompt version paired with [`DEFAULT_MODEL`]. Bumped when the
/// shape of the built-in descriptor changes in a way that
/// invalidates cached output keyed on
/// `(source_sha256, model, prompt_version)`.
pub const DEFAULT_PROMPT_VERSION: &str = "default-builtin-v1";

/// Built-in markdown-friendly default descriptor used when
/// `--no-format-discovery` is set and no cached `format.json`
/// exists for the input spec. Produces today's chunk/table
/// behavior on Markdown source inputs without requiring an LLM
/// endpoint.
///
/// The returned value is intentionally empty: it carries no
/// per-spec section roles, tables, figures, glossary entries, or
/// chrome regexes. Downstream stages fall through to their
/// heuristic / positional defaults. `source_sha256` is left
/// blank — the CLI fills it in when caching the descriptor next
/// to the input.
pub fn default_descriptor() -> FormatJson {
    FormatJson {
        schema_version: FormatJson::current_schema_version(),
        model: DEFAULT_MODEL.to_string(),
        prompt_version: DEFAULT_PROMPT_VERSION.to_string(),
        source_sha256: String::new(),
        // Epoch zero serialises as `1970-01-01T00:00:00Z`. The
        // built-in default is content-agnostic, so a fixed
        // timestamp keeps the descriptor reproducible across
        // invocations (no wall-clock noise in cached output).
        discovered_at: epoch_zero(),
        section_roles: Vec::new(),
        tables: Vec::new(),
        figures: Vec::new(),
        glossary: Vec::new(),
        chrome: Vec::new(),
        validation: ValidationBlock::default(),
    }
}

/// `1970-01-01T00:00:00Z`. Hard-coded so the default descriptor
/// is bit-identical across runs.
fn epoch_zero() -> DateTime<Utc> {
    Utc.timestamp_opt(0, 0)
        .single()
        .expect("epoch 0 is a valid UTC instant")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::__internal::session::spec_ingest::format::descriptor::ContentKey;

    /// `default_descriptor()` returns a `FormatJson` with the
    /// pinned schema version, sentinel `model` /
    /// `prompt_version`, and every collection empty.
    #[test]
    fn default_descriptor_is_empty_scaffolding() {
        let d = default_descriptor();
        assert_eq!(d.schema_version, 1);
        assert_eq!(d.schema_version, FormatJson::current_schema_version());
        assert_eq!(d.model, "default-builtin");
        assert_eq!(d.prompt_version, "default-builtin-v1");
        assert_eq!(d.source_sha256, "");
        assert!(d.section_roles.is_empty());
        assert!(d.tables.is_empty());
        assert!(d.figures.is_empty());
        assert!(d.glossary.is_empty());
        assert!(d.chrome.is_empty());
        // ValidationBlock::default() is all-zero / empty.
        assert_eq!(d.validation, ValidationBlock::default());
    }

    /// The default descriptor round-trips through `write` +
    /// `load` to a tempfile.
    #[test]
    fn default_descriptor_round_trips_through_tempfile() {
        let value = default_descriptor();
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("format.json");
        value.write(&path).expect("write");
        let loaded = FormatJson::load(&path).expect("load");
        assert_eq!(value, loaded);
    }

    /// `content_key()` on the default returns the sentinel
    /// `(source_sha256="", model="default-builtin",
    /// prompt_version="default-builtin-v1")` tuple.
    #[test]
    fn default_descriptor_content_key_is_sentinel() {
        let key = default_descriptor().content_key();
        assert_eq!(
            key,
            ContentKey {
                source_sha256: String::new(),
                model: "default-builtin".to_string(),
                prompt_version: "default-builtin-v1".to_string(),
            }
        );
    }
}
