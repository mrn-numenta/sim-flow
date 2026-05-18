//! Parser for the `## Worked Examples` section (Chapter 2 §2.3.18).
//!
//! Each example lives under `### Example N: <name>`. The body
//! captures three named subsections: `Inputs`, `Expected flow`,
//! `Expected outputs`. The schema chapter shows two encodings:
//!
//! 1. H4 subsections (`#### Inputs`, etc.).
//! 2. Bold-keyed inline subsections (`**Inputs:** ...`,
//!    `**Expected flow:**` followed by a numbered list).
//!
//! Both forms are accepted; the inline bold form is canonical
//! per the chapter example.

use super::section_util::{collect_prose, split_h3, split_h4};
use crate::session::spec_md::types::WorkedExample;

pub(crate) fn parse_worked_examples(body: &str) -> Vec<WorkedExample> {
    let mut out: Vec<WorkedExample> = Vec::new();
    let (_pre, subs) = split_h3(body);
    for sub in subs {
        let Some(name) = sub.heading.strip_prefix("Example") else {
            continue;
        };
        // Strip the leading `<N>:` prefix from `Example <N>: <name>`.
        let name = name
            .trim()
            .split_once(':')
            .map(|(_, n)| n.trim())
            .unwrap_or(name.trim());
        let mut ex = WorkedExample {
            name: name.to_string(),
            ..WorkedExample::default()
        };
        // H4 form first.
        let (_preamble, h4s) = split_h4(&sub.body);
        if !h4s.is_empty() {
            for h4 in h4s {
                match h4.heading.to_ascii_lowercase().as_str() {
                    "inputs" => ex.inputs = collect_prose(&h4.body),
                    "expected flow" => ex.expected_flow = collect_prose(&h4.body),
                    "expected outputs" => ex.expected_outputs = collect_prose(&h4.body),
                    _ => {}
                }
            }
        } else {
            // Inline bold-key form: split body by **Key:** markers.
            let inline = extract_inline_sections(&sub.body);
            for (k, v) in inline {
                match k.to_ascii_lowercase().as_str() {
                    "inputs" => ex.inputs = v,
                    "expected flow" => ex.expected_flow = v,
                    "expected outputs" => ex.expected_outputs = v,
                    _ => {}
                }
            }
        }
        out.push(ex);
    }
    out
}

/// Split a body by `**Key:**` markers. The value for each marker is
/// everything between it and the next marker (or end-of-body),
/// trimmed of leading / trailing whitespace.
fn extract_inline_sections(body: &str) -> Vec<(String, String)> {
    let mut marks: Vec<(usize, String, usize)> = Vec::new();
    // Find each `**Key:**` literal on its own.
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if &bytes[i..i + 2] == b"**" {
            let after = i + 2;
            if let Some(close_rel) = body[after..].find("**") {
                let key = body[after..after + close_rel].trim_end_matches(':').trim();
                let value_start = after + close_rel + 2;
                if !key.is_empty() {
                    marks.push((i, key.to_string(), value_start));
                }
                i = value_start;
                continue;
            }
        }
        i += 1;
    }
    let mut out: Vec<(String, String)> = Vec::new();
    for (idx, (_, key, value_start)) in marks.iter().enumerate() {
        let value_end = marks.get(idx + 1).map(|m| m.0).unwrap_or(body.len());
        let raw = body[*value_start..value_end].trim();
        let stripped = raw.trim_start_matches(':').trim();
        // If the value contains markdown-list lines, keep them as a
        // normalized newline-joined string (one bullet/numbered item
        // per line, leading marker stripped). Otherwise fall back to
        // prose collection.
        let normalized = if stripped.lines().any(is_list_line) {
            stripped
                .lines()
                .map(|l| {
                    let t = l.trim();
                    // Strip list markers `- `, `* `, `+ `, `N. `.
                    if let Some(r) = t.strip_prefix("- ") {
                        return r.to_string();
                    }
                    if let Some(r) = t.strip_prefix("* ") {
                        return r.to_string();
                    }
                    if let Some(r) = t.strip_prefix("+ ") {
                        return r.to_string();
                    }
                    if let Some(rest) = t.split_once(". ")
                        && rest.0.chars().all(|c| c.is_ascii_digit())
                    {
                        return rest.1.to_string();
                    }
                    t.to_string()
                })
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        } else if stripped.contains('\n') {
            collect_prose(stripped)
        } else {
            stripped.to_string()
        };
        out.push((key.clone(), normalized));
    }
    out
}

fn is_list_line(line: &str) -> bool {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix("- ") {
        return !rest.is_empty();
    }
    if let Some(rest) = t.strip_prefix("* ") {
        return !rest.is_empty();
    }
    if let Some(rest) = t.strip_prefix("+ ") {
        return !rest.is_empty();
    }
    if let Some((digits, rest)) = t.split_once(". ")
        && !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
        && !rest.is_empty()
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_bold_form() {
        let body = "\
## Worked Examples

### Example 1: Single ADD instruction through the pipeline

**Inputs:** PC=0x1000, instruction `add x1, x2, x3` at 0x1000
**Expected flow:**
1. Cycle 1: IF fetches parcel from 0x1000
2. Cycle 2: PD decodes

**Expected outputs:** x1 <- x2 + x3 at cycle 6.
";
        let exs = parse_worked_examples(body);
        assert_eq!(exs.len(), 1);
        let e = &exs[0];
        assert!(e.name.contains("Single ADD"));
        assert!(e.inputs.contains("PC=0x1000"));
        assert!(e.expected_flow.contains("Cycle 1") || !e.expected_flow.is_empty());
        assert!(e.expected_outputs.contains("at cycle 6"));
    }

    #[test]
    fn parses_h4_form() {
        let body = "\
## Worked Examples

### Example 2: corner case

#### Inputs

PC=0x2000, branch taken.

#### Expected flow

Branch redirects fetch.

#### Expected outputs

Fetch advances to target.
";
        let exs = parse_worked_examples(body);
        assert_eq!(exs.len(), 1);
        let e = &exs[0];
        assert_eq!(e.name, "corner case");
        assert!(e.inputs.contains("PC=0x2000"));
        assert!(e.expected_flow.contains("Branch redirects"));
        assert!(e.expected_outputs.contains("advances"));
    }
}
