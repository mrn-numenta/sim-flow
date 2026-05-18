//! Prose-only sections: `Purpose`, `Scope`, `Non-goals`, and the
//! prose-only Functional Behavior / Pipeline and Hierarchy / Reset
//! sub-sections. Each one is one to three paragraphs of plain
//! markdown; the parser collapses to a normalized string.

use super::section_util::collect_prose;

/// Collect the prose body of a section (after stripping the leading
/// `## <Heading>`). Returns a normalized string suitable for storing
/// in any of the prose fields on [`crate::session::spec_md::types::SpecMd`].
pub(crate) fn parse_prose_section(body: &str) -> String {
    collect_prose(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_purpose_paragraphs() {
        let body = "\
## Purpose

The model implements a 6-stage pipeline.

It carries XLEN-wide datapaths.
";
        let prose = parse_prose_section(body);
        assert_eq!(
            prose,
            "The model implements a 6-stage pipeline.\n\nIt carries XLEN-wide datapaths."
        );
    }

    #[test]
    fn empty_body_yields_empty_string() {
        let body = "## Purpose\n";
        assert_eq!(parse_prose_section(body), "");
    }
}
