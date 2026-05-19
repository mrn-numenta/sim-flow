//! Parser for the `## Power Domains` section (Chapter 7 §7.7).
//!
//! Single typed table with columns: Name / Voltage / Always-on /
//! Description.

use super::SpecMdParseError;
use super::table::MarkdownTable;
use crate::session::spec_md::types::PowerDomain;

pub(crate) fn parse_power_domains(body: &str) -> Result<Vec<PowerDomain>, SpecMdParseError> {
    let tables = MarkdownTable::parse_all(body)?;
    let Some(t) = tables.first() else {
        return Ok(Vec::new());
    };
    let name_idx = t
        .headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case("name"))
        .ok_or_else(|| missing("Name", &t.headers))?;
    let voltage_idx = t
        .headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case("voltage"));
    let always_on_idx = t.headers.iter().position(|h| {
        let lc = h.to_ascii_lowercase();
        lc == "always-on" || lc == "always on" || lc == "always_on"
    });
    let desc_idx = t.headers.iter().position(|h| {
        let lc = h.to_ascii_lowercase();
        lc == "description" || lc == "notes"
    });
    let mut out = Vec::with_capacity(t.rows.len());
    for row in &t.rows {
        out.push(PowerDomain {
            name: t.cell(row, name_idx).to_string(),
            voltage: voltage_idx
                .map(|i| t.cell(row, i).to_string())
                .unwrap_or_default(),
            always_on: always_on_idx
                .map(|i| parse_bool(t.cell(row, i)))
                .unwrap_or(false),
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
        "yes" | "true" | "y" | "1"
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
    fn parses_power_domains_table() {
        let body = "\
## Power Domains

| Name | Voltage | Always-on | Description |
| --- | --- | --- | --- |
| core_pd | 0.85V | no | Power-gated core |
| aon_pd | 0.85V | yes | Always-on island |
";
        let pd = parse_power_domains(body).expect("parses");
        assert_eq!(pd.len(), 2);
        assert_eq!(pd[0].name, "core_pd");
        assert_eq!(pd[0].voltage, "0.85V");
        assert!(!pd[0].always_on);
        assert_eq!(pd[1].name, "aon_pd");
        assert!(pd[1].always_on);
    }

    #[test]
    fn empty_body_yields_empty() {
        assert!(
            parse_power_domains("## Power Domains\n")
                .expect("parses")
                .is_empty()
        );
    }
}
