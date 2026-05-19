//! Parser for the `## Clock Domains` section (Chapter 7 §7.7).
//!
//! Single typed table with columns: Name / Frequency / Source /
//! Description.

use super::SpecMdParseError;
use super::table::MarkdownTable;
use crate::session::spec_md::types::ClockDomain;

pub(crate) fn parse_clock_domains(body: &str) -> Result<Vec<ClockDomain>, SpecMdParseError> {
    let tables = MarkdownTable::parse_all(body)?;
    let Some(t) = tables.first() else {
        return Ok(Vec::new());
    };
    let name_idx = t
        .headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case("name"))
        .ok_or_else(|| missing("Name", &t.headers))?;
    let freq_idx = t
        .headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case("frequency"));
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
        out.push(ClockDomain {
            name: t.cell(row, name_idx).to_string(),
            frequency: freq_idx
                .map(|i| t.cell(row, i).to_string())
                .unwrap_or_default(),
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
    fn parses_clock_domains_table() {
        let body = "\
## Clock Domains

| Name | Frequency | Source | Description |
| --- | --- | --- | --- |
| core_clk | 1 GHz | PLL0 | Primary core clock |
| bus_clk | 500 MHz | PLL1 | AHB bus |
";
        let cd = parse_clock_domains(body).expect("parses");
        assert_eq!(cd.len(), 2);
        assert_eq!(cd[0].name, "core_clk");
        assert_eq!(cd[0].frequency, "1 GHz");
        assert_eq!(cd[0].source, "PLL0");
        assert_eq!(cd[0].description, "Primary core clock");
        assert_eq!(cd[1].name, "bus_clk");
    }

    #[test]
    fn empty_body_yields_empty() {
        assert!(
            parse_clock_domains("## Clock Domains\n")
                .expect("parses")
                .is_empty()
        );
    }
}
