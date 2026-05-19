//! Parser for the `## Reset Domains` section (Chapter 7 §7.7).
//!
//! Single typed table with columns: Name / Polarity / Sync /
//! Source / Description.

use super::SpecMdParseError;
use super::table::MarkdownTable;
use crate::session::spec_md::types::ResetDomain;

pub(crate) fn parse_reset_domains(body: &str) -> Result<Vec<ResetDomain>, SpecMdParseError> {
    let tables = MarkdownTable::parse_all(body)?;
    let Some(t) = tables.first() else {
        return Ok(Vec::new());
    };
    let name_idx = t
        .headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case("name"))
        .ok_or_else(|| missing("Name", &t.headers))?;
    let polarity_idx = t
        .headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case("polarity"));
    let sync_idx = t.headers.iter().position(|h| {
        let lc = h.to_ascii_lowercase();
        lc == "sync" || lc == "synchronous"
    });
    let source_idx = t
        .headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case("source"));
    let desc_idx = t.headers.iter().position(|h| {
        let lc = h.to_ascii_lowercase();
        lc == "description" || lc == "notes"
    });
    let mut out = Vec::with_capacity(t.rows.len());
    for row in &t.rows {
        out.push(ResetDomain {
            name: t.cell(row, name_idx).to_string(),
            polarity: polarity_idx
                .map(|i| t.cell(row, i).to_string())
                .unwrap_or_default(),
            sync: sync_idx
                .map(|i| parse_bool(t.cell(row, i)))
                .unwrap_or(false),
            source: source_idx
                .map(|i| t.cell(row, i).to_string())
                .unwrap_or_default(),
            description: desc_idx
                .map(|i| t.cell(row, i).to_string())
                .unwrap_or_default(),
        });
    }
    Ok(out)
}

fn parse_bool(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "yes" | "true" | "y" | "1" | "sync" | "synchronous"
    )
}

fn missing(label: &str, headers: &[String]) -> SpecMdParseError {
    SpecMdParseError::MalformedTable {
        message: format!(
            "missing required column `{label}`; headers were [{}]",
            headers.join(", ")
        ),
        line: 0,
        column: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reset_domains_table() {
        let body = "\
## Reset Domains

| Name | Polarity | Sync | Source | Description |
| --- | --- | --- | --- | --- |
| nReset | active_low | yes | power-on | Main reset |
| wdog_rst | active_high | no | watchdog | Watchdog reset |
";
        let rd = parse_reset_domains(body).expect("parses");
        assert_eq!(rd.len(), 2);
        assert_eq!(rd[0].name, "nReset");
        assert_eq!(rd[0].polarity, "active_low");
        assert!(rd[0].sync);
        assert_eq!(rd[0].source, "power-on");
        assert_eq!(rd[1].name, "wdog_rst");
        assert_eq!(rd[1].polarity, "active_high");
        assert!(!rd[1].sync);
    }

    #[test]
    fn empty_body_yields_empty() {
        assert!(
            parse_reset_domains("## Reset Domains\n")
                .expect("parses")
                .is_empty()
        );
    }
}
