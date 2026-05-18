//! Markdown to [`SpecMd`] parser.
//!
//! Entry point: [`parse`]. Walks the markdown event stream produced
//! by `pulldown_cmark`, segments the document into H2-delimited
//! sections, and dispatches each section to a per-section parser.
//! Empty input parses to `Ok(SpecMd::default())`; missing OPTIONAL
//! sections also yield defaults. Hard errors (malformed tables,
//! bad anchors, missing REQUIRED sub-structure) surface as
//! [`SpecMdParseError`].

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::types::SpecMd;

pub(crate) mod assumptions;
pub(crate) mod blocks;
pub(crate) mod external_interfaces;
pub(crate) mod metadata;
pub(crate) mod parameters;
pub(crate) mod prose;
pub(crate) mod section_util;
pub(crate) mod state_machines;
pub(crate) mod table;

/// Errors produced by [`parse`]. Every variant carries the offending
/// line / column so the caller can surface a precise diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecMdParseError {
    /// A REQUIRED section heading was not found.
    MissingSection {
        section: &'static str,
        line: usize,
        column: usize,
    },
    /// A markdown table had the wrong shape (missing required columns,
    /// mismatched row width, etc.).
    MalformedTable {
        message: String,
        line: usize,
        column: usize,
    },
    /// A source-spec anchor string failed to parse.
    BadAnchor {
        anchor: String,
        line: usize,
        column: usize,
    },
    /// Any other structural error not covered by a more specific
    /// variant.
    Other {
        message: String,
        line: usize,
        column: usize,
    },
}

impl std::fmt::Display for SpecMdParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecMdParseError::MissingSection {
                section,
                line,
                column,
            } => {
                write!(f, "missing required section `{section}` at {line}:{column}")
            }
            SpecMdParseError::MalformedTable {
                message,
                line,
                column,
            } => {
                write!(f, "malformed table at {line}:{column}: {message}")
            }
            SpecMdParseError::BadAnchor {
                anchor,
                line,
                column,
            } => {
                write!(f, "bad source-spec anchor `{anchor}` at {line}:{column}")
            }
            SpecMdParseError::Other {
                message,
                line,
                column,
            } => {
                write!(f, "spec.md parse error at {line}:{column}: {message}")
            }
        }
    }
}

impl std::error::Error for SpecMdParseError {}

/// Build the `pulldown_cmark::Options` used by the parser. We need
/// table support; everything else stays default.
pub(crate) fn cmark_options() -> Options {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts
}

/// Parse a structured `spec.md` document into typed form.
///
/// On empty input, returns `Ok(SpecMd::default())` with every
/// section empty. Missing OPTIONAL sections also yield defaults;
/// missing REQUIRED sections do NOT raise an error here -- this
/// parser produces whatever it can find. Required-section
/// validation lives in `validate.rs`.
pub fn parse(input: &str) -> Result<SpecMd, SpecMdParseError> {
    let mut spec = SpecMd::default();
    if input.trim().is_empty() {
        return Ok(spec);
    }

    // Segment the document by H1 (title) and H2 (sections).
    let segments = segment_document(input);
    for seg in segments {
        match seg {
            Segment::Title(text) => {
                spec.title = text;
            }
            Segment::Section(section) => {
                dispatch_section(&section, &mut spec)?;
            }
        }
    }

    Ok(spec)
}

/// One slice of the document: either the H1 title or an H2-rooted
/// section body.
#[derive(Debug, Clone)]
pub(crate) enum Segment {
    Title(String),
    Section(Section),
}

/// One H2-rooted section: the heading text plus the raw markdown
/// body (everything from the heading up to but not including the
/// next H2 or end of input). Per-section parsers reparse the body
/// with `pulldown_cmark` to extract their own sub-structure.
#[derive(Debug, Clone)]
#[allow(dead_code)] // body / line are consumed by per-section parsers (M1.4+).
pub(crate) struct Section {
    pub heading: String,
    pub body: String,
    /// 1-based line number of the H2 heading. Used in error
    /// diagnostics for per-section parsers.
    pub line: usize,
}

/// Walk the event stream and compute (a) the H1 title if present,
/// (b) the H2 headings + their byte offsets so we can slice the
/// source into section bodies for the per-section parsers.
///
/// We rely on byte offsets from `pulldown_cmark`'s
/// `into_offset_iter` so the per-section parsers receive the exact
/// original markdown for their section -- this keeps round-trip
/// stability achievable.
pub(crate) fn segment_document(input: &str) -> Vec<Segment> {
    let parser = Parser::new_ext(input, cmark_options()).into_offset_iter();

    // (heading_level, start_byte, end_byte_of_heading, heading_text)
    let mut h1_title: Option<String> = None;
    let mut h2_marks: Vec<(usize, String)> = Vec::new();

    let mut current_h: Option<(HeadingLevel, usize)> = None;
    let mut current_text = String::new();
    for (event, range) in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current_h = Some((level, range.start));
                current_text.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, start)) = current_h.take() {
                    let text = std::mem::take(&mut current_text).trim().to_string();
                    match level {
                        HeadingLevel::H1 if h1_title.is_none() => {
                            h1_title = Some(text);
                        }
                        HeadingLevel::H2 => {
                            h2_marks.push((start, text));
                        }
                        _ => {}
                    }
                }
            }
            Event::Text(t) | Event::Code(t) if current_h.is_some() => {
                current_text.push_str(&t);
            }
            _ => {}
        }
    }

    let mut out: Vec<Segment> = Vec::new();
    if let Some(title) = h1_title
        && !title.is_empty()
    {
        out.push(Segment::Title(title));
    }
    for (i, (start, heading)) in h2_marks.iter().enumerate() {
        let end = h2_marks.get(i + 1).map(|(s, _)| *s).unwrap_or(input.len());
        let body = input[*start..end].to_string();
        let line = byte_offset_to_line(input, *start);
        out.push(Segment::Section(Section {
            heading: heading.clone(),
            body,
            line,
        }));
    }
    out
}

/// 1-based line number for a byte offset.
pub(crate) fn byte_offset_to_line(input: &str, offset: usize) -> usize {
    let upto = offset.min(input.len());
    input[..upto].bytes().filter(|b| *b == b'\n').count() + 1
}

fn dispatch_section(section: &Section, spec: &mut SpecMd) -> Result<(), SpecMdParseError> {
    match section.heading.as_str() {
        "Metadata" => {
            spec.metadata = metadata::parse_metadata(&section.body)?;
            Ok(())
        }
        "Purpose" => {
            spec.purpose = prose::parse_prose_section(&section.body);
            Ok(())
        }
        "Scope" => {
            spec.scope = prose::parse_prose_section(&section.body);
            Ok(())
        }
        "Non-goals" => {
            spec.non_goals = prose::parse_prose_section(&section.body);
            Ok(())
        }
        "Assumptions and Constraints" => {
            spec.assumptions = assumptions::parse_assumptions(&section.body)?;
            Ok(())
        }
        "External Interfaces" => {
            spec.external_interfaces =
                external_interfaces::parse_external_interfaces(&section.body)?;
            Ok(())
        }
        "Blocks" => {
            spec.blocks = blocks::parse_blocks(&section.body)?;
            Ok(())
        }
        "Parameters" => {
            spec.parameters = parameters::parse_parameters(&section.body)?;
            Ok(())
        }
        "State Machines" => {
            spec.state_machines = state_machines::parse_state_machines(&section.body)?;
            Ok(())
        }
        "Encodings" => Ok(()),
        "Memory Map" => Ok(()),
        "Connectivity" => Ok(()),
        "Error Handling" => Ok(()),
        "Functional Behavior" => Ok(()),
        "Timing, Latency, and Throughput" => Ok(()),
        "Pipeline and Hierarchy" => Ok(()),
        "Reset, Initialization, Flush, Drain" => Ok(()),
        "Cycle-Accurate Behavior" => Ok(()),
        "Figures" => Ok(()),
        "Worked Examples" => Ok(()),
        "Source-Spec Anchors" => Ok(()),
        "Open Questions" => Ok(()),
        "Auto-decisions" => Ok(()),
        // Unknown sections are tolerated (forward compatibility) but
        // produce no parsed output.
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_default() {
        let spec = parse("").expect("empty input parses");
        assert_eq!(spec, SpecMd::default());
    }

    #[test]
    fn whitespace_input_yields_default() {
        let spec = parse("   \n\n  \n").expect("whitespace input parses");
        assert_eq!(spec, SpecMd::default());
    }

    #[test]
    fn title_only_input_captures_title() {
        let spec = parse("# RV12 RISC-V CPU Core\n").expect("title parses");
        assert_eq!(spec.title, "RV12 RISC-V CPU Core");
    }

    #[test]
    fn h2_sections_are_segmented_and_routed() {
        // The dispatcher is still a stub but unknown sections are
        // tolerated -- this exercises the segmenter end-to-end.
        let input = "# Doc\n\n## Metadata\n\nbody\n\n## Purpose\n\nmore\n";
        let segments = segment_document(input);
        let headings: Vec<_> = segments
            .iter()
            .filter_map(|s| match s {
                Segment::Section(sec) => Some(sec.heading.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            headings,
            vec!["Metadata".to_string(), "Purpose".to_string()]
        );
        // Title is captured separately.
        let title = segments.iter().find_map(|s| match s {
            Segment::Title(t) => Some(t.clone()),
            _ => None,
        });
        assert_eq!(title.as_deref(), Some("Doc"));
    }
}
