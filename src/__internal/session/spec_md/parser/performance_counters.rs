//! Parser for the `## Performance Counters` section
//! (Chapter 7 §7.7).
//!
//! Single typed table with columns: Id / Name / CSR address /
//! Description.

use super::SpecMdParseError;
use super::table::MarkdownTable;
use crate::session::spec_md::types::PmuEvent;

pub(crate) fn parse_performance_counters(body: &str) -> Result<Vec<PmuEvent>, SpecMdParseError> {
    let tables = MarkdownTable::parse_all(body)?;
    let Some(t) = tables.first() else {
        return Ok(Vec::new());
    };
    let id_idx = t
        .headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case("id"))
        .ok_or_else(|| missing("Id", &t.headers))?;
    let name_idx = t
        .headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case("name"))
        .ok_or_else(|| missing("Name", &t.headers))?;
    let csr_idx = t.headers.iter().position(|h| {
        let lc = h.to_ascii_lowercase();
        lc == "csr address" || lc == "csr" || lc == "address"
    });
    let desc_idx = t.headers.iter().position(|h| {
        let lc = h.to_ascii_lowercase();
        lc == "description" || lc == "notes"
    });
    let mut out = Vec::with_capacity(t.rows.len());
    for row in &t.rows {
        out.push(PmuEvent {
            id: t.cell(row, id_idx).trim_matches('`').to_string(),
            name: t.cell(row, name_idx).to_string(),
            csr_address: csr_idx
                .map(|i| t.cell(row, i).trim_matches('`').to_string())
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
    fn parses_pmu_events_table() {
        let body = "\
## Performance Counters

| Id | Name | CSR address | Description |
| --- | --- | --- | --- |
| cycles | Cycle counter | 0xC00 | Total elapsed cycles |
| icache_miss | Instruction cache miss | 0xC03 | I-cache miss count |
";
        let pmu = parse_performance_counters(body).expect("parses");
        assert_eq!(pmu.len(), 2);
        assert_eq!(pmu[0].id, "cycles");
        assert_eq!(pmu[0].name, "Cycle counter");
        assert_eq!(pmu[0].csr_address, "0xC00");
        assert!(pmu[0].description.contains("Total"));
        assert_eq!(pmu[1].id, "icache_miss");
    }

    #[test]
    fn empty_body_yields_empty() {
        assert!(
            parse_performance_counters("## Performance Counters\n")
                .expect("parses")
                .is_empty()
        );
    }
}
