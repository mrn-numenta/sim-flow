//! Markdown to [`SpecMd`] parser.
//!
//! Entry point: [`parse`]. Walks the markdown event stream produced
//! by `pulldown_cmark`, segments the document into H2-delimited
//! sections, and dispatches each section to a per-section parser.
//! Empty input parses to `Ok(SpecMd::default())`; missing OPTIONAL
//! sections also yield defaults. Hard errors (malformed tables,
//! bad anchors, missing REQUIRED sub-structure) surface as
//! [`SpecMdParseError`].

use super::types::SpecMd;

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

    // Peel out the H1 title if present so dispatch only sees H2-
    // rooted sections.
    let (title, body) = split_title(input);
    if !title.is_empty() {
        spec.title = title;
    }

    let sections = split_sections(body);
    for section in sections {
        dispatch_section(&section, &mut spec)?;
    }

    Ok(spec)
}

/// One H2-rooted section: the heading text plus its body lines
/// (everything from the heading up to but not including the next H2
/// or end of input).
#[derive(Debug, Clone)]
pub(crate) struct Section {
    pub heading: String,
    pub body: String,
}

fn split_title(input: &str) -> (String, &str) {
    let mut lines = input.lines();
    let mut consumed = 0usize;
    let mut title = String::new();
    for line in lines.by_ref() {
        consumed += line.len() + 1; // +1 for the newline
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            title = rest.trim().to_string();
        } else {
            // No title -- rewind: return the entire input as the body.
            return (String::new(), input);
        }
        break;
    }
    let body = if consumed >= input.len() {
        ""
    } else {
        &input[consumed..]
    };
    (title, body)
}

fn split_sections(body: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    let mut current: Option<Section> = None;
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            if let Some(sec) = current.take() {
                out.push(sec);
            }
            current = Some(Section {
                heading: rest.trim().to_string(),
                body: String::new(),
            });
        } else if let Some(sec) = current.as_mut() {
            sec.body.push_str(line);
            sec.body.push('\n');
        }
    }
    if let Some(sec) = current.take() {
        out.push(sec);
    }
    out
}

fn dispatch_section(section: &Section, spec: &mut SpecMd) -> Result<(), SpecMdParseError> {
    // Per-section parsers will be wired in milestones 1.4 through
    // 1.12. For now every branch is a no-op stub so the dispatch
    // table is the single source of truth for section recognition.
    let _ = spec;
    match section.heading.as_str() {
        "Metadata" => Ok(()),
        "Purpose" => Ok(()),
        "Scope" => Ok(()),
        "Non-goals" => Ok(()),
        "Assumptions and Constraints" => Ok(()),
        "External Interfaces" => Ok(()),
        "Blocks" => Ok(()),
        "Parameters" => Ok(()),
        "State Machines" => Ok(()),
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
}
