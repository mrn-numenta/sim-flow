//! Source-spec anchor format parser (Chapter 2 §2.4).
//!
//! Three textual forms:
//!
//! - Page: `<source>:p<N>`        (e.g. `primary:p13`).
//! - Page-range: `<source>:p<N>-<M>` (e.g. `primary:p12-13`).
//! - Chunk: `<source>:chunk-<NNN>` (e.g. `primary:chunk-0042`).
//!
//! `<source>` is `primary` or a peer ID; the parser does not validate
//! against `manifest.toml.peers[].id` (that lives in the lance build
//! / validation paths).

use super::types::SourceSpecAnchor;

/// Errors produced by [`parse_anchor`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorParseError {
    pub input: String,
    pub reason: AnchorParseReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorParseReason {
    /// No `:` separator found between source and form.
    MissingColon,
    /// Empty `<source>` half.
    EmptySource,
    /// Form prefix not recognised (not `p` or `chunk-`).
    UnknownForm,
    /// Numeric value did not parse.
    BadNumber,
    /// Page-range had `<N>-<M>` with M < N.
    InvalidRange,
}

impl std::fmt::Display for AnchorParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let why = match &self.reason {
            AnchorParseReason::MissingColon => "missing `:`",
            AnchorParseReason::EmptySource => "empty source",
            AnchorParseReason::UnknownForm => "unknown form",
            AnchorParseReason::BadNumber => "bad number",
            AnchorParseReason::InvalidRange => "invalid range",
        };
        write!(f, "bad anchor `{}`: {why}", self.input)
    }
}

impl std::error::Error for AnchorParseError {}

impl SourceSpecAnchor {
    /// Parse a string into a [`SourceSpecAnchor`]. The exact textual
    /// inverse of [`Self::to_anchor_string`].
    pub fn parse(s: &str) -> Result<SourceSpecAnchor, AnchorParseError> {
        let input = s.trim();
        let Some((source, rest)) = input.split_once(':') else {
            return Err(AnchorParseError {
                input: input.to_string(),
                reason: AnchorParseReason::MissingColon,
            });
        };
        let source = source.trim();
        if source.is_empty() {
            return Err(AnchorParseError {
                input: input.to_string(),
                reason: AnchorParseReason::EmptySource,
            });
        }
        if let Some(rest) = rest.strip_prefix("chunk-") {
            return Ok(SourceSpecAnchor::Chunk {
                source: source.to_string(),
                chunk: rest.to_string(),
            });
        }
        if let Some(rest) = rest.strip_prefix('p') {
            if let Some((a, b)) = rest.split_once('-') {
                let start: u32 = a.parse().map_err(|_| AnchorParseError {
                    input: input.to_string(),
                    reason: AnchorParseReason::BadNumber,
                })?;
                let end: u32 = b.parse().map_err(|_| AnchorParseError {
                    input: input.to_string(),
                    reason: AnchorParseReason::BadNumber,
                })?;
                if end < start {
                    return Err(AnchorParseError {
                        input: input.to_string(),
                        reason: AnchorParseReason::InvalidRange,
                    });
                }
                return Ok(SourceSpecAnchor::PageRange {
                    source: source.to_string(),
                    start,
                    end,
                });
            }
            let page: u32 = rest.parse().map_err(|_| AnchorParseError {
                input: input.to_string(),
                reason: AnchorParseReason::BadNumber,
            })?;
            return Ok(SourceSpecAnchor::Page {
                source: source.to_string(),
                page,
            });
        }
        Err(AnchorParseError {
            input: input.to_string(),
            reason: AnchorParseReason::UnknownForm,
        })
    }

    /// Canonical string form (inverse of [`Self::parse`]).
    pub fn to_anchor_string(&self) -> String {
        match self {
            SourceSpecAnchor::Page { source, page } => format!("{source}:p{page}"),
            SourceSpecAnchor::PageRange { source, start, end } => {
                format!("{source}:p{start}-{end}")
            }
            SourceSpecAnchor::Chunk { source, chunk } => format!("{source}:chunk-{chunk}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_page_form() {
        let a = SourceSpecAnchor::parse("primary:p13").expect("parses");
        assert_eq!(
            a,
            SourceSpecAnchor::Page {
                source: "primary".to_string(),
                page: 13
            }
        );
        assert_eq!(a.to_anchor_string(), "primary:p13");
    }

    #[test]
    fn parses_page_range() {
        let a = SourceSpecAnchor::parse("tm-spec:p7-9").expect("parses");
        assert_eq!(
            a,
            SourceSpecAnchor::PageRange {
                source: "tm-spec".to_string(),
                start: 7,
                end: 9
            }
        );
        assert_eq!(a.to_anchor_string(), "tm-spec:p7-9");
    }

    #[test]
    fn parses_chunk_form() {
        let a = SourceSpecAnchor::parse("primary:chunk-0042").expect("parses");
        assert_eq!(
            a,
            SourceSpecAnchor::Chunk {
                source: "primary".to_string(),
                chunk: "0042".to_string()
            }
        );
        assert_eq!(a.to_anchor_string(), "primary:chunk-0042");
    }

    #[test]
    fn rejects_missing_colon() {
        assert!(matches!(
            SourceSpecAnchor::parse("primaryp13"),
            Err(AnchorParseError {
                reason: AnchorParseReason::MissingColon,
                ..
            })
        ));
    }

    #[test]
    fn rejects_unknown_form() {
        assert!(matches!(
            SourceSpecAnchor::parse("primary:line42"),
            Err(AnchorParseError {
                reason: AnchorParseReason::UnknownForm,
                ..
            })
        ));
    }

    #[test]
    fn rejects_bad_number() {
        assert!(matches!(
            SourceSpecAnchor::parse("primary:pXX"),
            Err(AnchorParseError {
                reason: AnchorParseReason::BadNumber,
                ..
            })
        ));
    }

    #[test]
    fn rejects_inverted_range() {
        assert!(matches!(
            SourceSpecAnchor::parse("primary:p10-3"),
            Err(AnchorParseError {
                reason: AnchorParseReason::InvalidRange,
                ..
            })
        ));
    }
}
