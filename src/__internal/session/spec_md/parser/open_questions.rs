//! Parser for the `## Open Questions` section (Chapter 2 §2.3.20).
//! Bullet list of free-form questions.

use super::section_util::collect_top_level_bullets;
use crate::session::spec_md::types::OpenQuestion;

pub(crate) fn parse_open_questions(body: &str) -> Vec<OpenQuestion> {
    collect_top_level_bullets(body)
        .into_iter()
        .map(|text| OpenQuestion { text })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_questions() {
        let body = "\
## Open Questions

- BPU table size at default `BPU_LOCAL_BITS=8` not specified (primary:p9)
- Reset value for `if_exception` not stated (primary:p13)
";
        let qs = parse_open_questions(body);
        assert_eq!(qs.len(), 2);
        assert!(qs[0].text.contains("BPU table size"));
    }
}
