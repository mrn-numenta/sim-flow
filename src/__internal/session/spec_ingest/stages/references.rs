//! Stage 5: cross-spec reference parsing.

use super::super::pipeline::IngestWarning;
use super::parse::SectionTree;

#[derive(Debug, Clone)]
pub struct CrossSpecReference {
    pub breadcrumb: Vec<String>,
    pub source_page: u32,
    /// Empty until the orchestrator resolves it against the registered
    /// peer set.
    pub peer_id: String,
    pub reference_text: String,
    pub referenced_breadcrumbs: Vec<String>,
}

pub fn parse_references(
    tree: &SectionTree,
    _warnings: &mut Vec<IngestWarning>,
) -> Vec<CrossSpecReference> {
    let mut out = Vec::new();
    let see_re = regex::Regex::new(
        r#"(?i)see\s+([A-Z][^,:\n]{1,100}?)[,:]\s+sections?\s+["']?([^"'\n]+?)["']?(?:[\.\n]|$)"#,
    )
    .unwrap();
    let see_simple_re = regex::Regex::new(
        r#"(?i)see\s+([A-Z][^,:\n]{1,100}?)[,:]\s+["']?([^"'\n]+?)["']?(?:[\.\n]|$)"#,
    )
    .unwrap();
    let link_re = regex::Regex::new(r#"\[([^\]]+)\]\(([^\)\s]+\.(?:md|pdf))\)"#).unwrap();
    for section in tree.iter() {
        let is_refs_section = section
            .heading
            .to_ascii_lowercase()
            .starts_with("references")
            || section.heading.to_ascii_lowercase().starts_with("inherits");
        for cap in see_re.captures_iter(&section.body) {
            let title = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
            let crumb = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            out.push(CrossSpecReference {
                breadcrumb: section.breadcrumb.clone(),
                source_page: section.page_range.0,
                peer_id: String::new(),
                reference_text: format!("see {title}, section {crumb}"),
                referenced_breadcrumbs: vec![crumb.to_string()],
            });
        }
        for cap in see_simple_re.captures_iter(&section.body) {
            // Skip duplicates of "see X, section Y".
            let kw = cap.get(0).map(|m| m.as_str()).unwrap_or("");
            if kw.to_ascii_lowercase().contains("section") {
                continue;
            }
            let title = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
            let crumb = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            if title.is_empty() || crumb.is_empty() {
                continue;
            }
            out.push(CrossSpecReference {
                breadcrumb: section.breadcrumb.clone(),
                source_page: section.page_range.0,
                peer_id: String::new(),
                reference_text: format!("see {title}: {crumb}"),
                referenced_breadcrumbs: vec![crumb.to_string()],
            });
        }
        for cap in link_re.captures_iter(&section.body) {
            let label = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
            let target = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            out.push(CrossSpecReference {
                breadcrumb: section.breadcrumb.clone(),
                source_page: section.page_range.0,
                peer_id: String::new(),
                reference_text: format!("[{label}]({target})"),
                referenced_breadcrumbs: Vec::new(),
            });
        }
        if is_refs_section {
            for line in section.body.lines() {
                let t = line
                    .trim()
                    .trim_start_matches('-')
                    .trim_start_matches('*')
                    .trim();
                if t.is_empty() {
                    continue;
                }
                // Already captured by see_re / link_re; only add the
                // bare bullet text if it didn't match either.
                if see_re.is_match(t) || see_simple_re.is_match(t) || link_re.is_match(t) {
                    continue;
                }
                out.push(CrossSpecReference {
                    breadcrumb: section.breadcrumb.clone(),
                    source_page: section.page_range.0,
                    peer_id: String::new(),
                    reference_text: t.to_string(),
                    referenced_breadcrumbs: Vec::new(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::spec_ingest::stages::parse::parse_markdown;

    #[test]
    fn detects_see_section_pattern() {
        let body = "# Top\nFor details see The Temporal Memory, section \"Hardware Elements\".\n";
        let mut w = Vec::new();
        let tree = parse_markdown(body, &mut w).unwrap();
        let refs = parse_references(&tree, &mut w);
        assert!(!refs.is_empty());
        assert!(refs[0].reference_text.contains("The Temporal Memory"));
    }

    #[test]
    fn detects_markdown_link_to_peer_pdf() {
        let body = "# Top\nRefer to [TM spec](../tm-spec.pdf) for details.\n";
        let mut w = Vec::new();
        let tree = parse_markdown(body, &mut w).unwrap();
        let refs = parse_references(&tree, &mut w);
        assert!(
            refs.iter()
                .any(|r| r.reference_text.contains("tm-spec.pdf"))
        );
    }

    #[test]
    fn detects_references_section_bullets() {
        let body = "# References\n- Some External Doc\n- Another Document v2\n\n## Other\nbody\n";
        let mut w = Vec::new();
        let tree = parse_markdown(body, &mut w).unwrap();
        let refs = parse_references(&tree, &mut w);
        assert!(
            refs.iter()
                .any(|r| r.reference_text.contains("Some External Doc"))
        );
    }
}
