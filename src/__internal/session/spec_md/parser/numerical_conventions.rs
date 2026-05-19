//! Parser for the `## Numerical Conventions` section
//! (Chapter 7 §7.7).
//!
//! Each convention is `### Convention: <name>` with a bold-property
//! block (Q-format default / Saturation policy / Signed default /
//! Rounding mode) plus optional `#### Description` prose.

use super::SpecMdParseError;
use super::section_util::{collect_prose, parse_bold_properties, split_h3, split_h4};
use crate::session::spec_md::types::NumericalConvention;

pub(crate) fn parse_numerical_conventions(
    body: &str,
) -> Result<Vec<NumericalConvention>, SpecMdParseError> {
    let mut out: Vec<NumericalConvention> = Vec::new();
    let (_pre, subs) = split_h3(body);
    for sub in subs {
        let Some(name) = sub.heading.strip_prefix("Convention:") else {
            continue;
        };
        let mut conv = NumericalConvention {
            name: name.trim().to_string(),
            ..NumericalConvention::default()
        };
        let (preamble, h4s) = split_h4(&sub.body);
        for (k, v) in parse_bold_properties(&preamble) {
            match k.to_ascii_lowercase().as_str() {
                "q-format default" | "q format default" | "q-format" => conv.q_format_default = v,
                "saturation policy" | "saturation" => conv.saturation_policy = v,
                "signed default" | "signedness" => conv.signed_default = v,
                "rounding mode" | "rounding" => conv.rounding_mode = v,
                _ => {}
            }
        }
        for h4 in h4s {
            if h4.heading.eq_ignore_ascii_case("description") {
                conv.description = collect_prose(&h4.body);
            }
        }
        out.push(conv);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numerical_conventions() {
        let body = "\
## Numerical Conventions

### Convention: default

**Q-format default:** Q16.16
**Saturation policy:** saturate
**Signed default:** signed
**Rounding mode:** round_half_even

#### Description

Default numerical handling for all signals.

### Convention: synapse_permanence

**Q-format default:** Q0.16
**Saturation policy:** saturate
**Signed default:** unsigned
**Rounding mode:** truncate
";
        let conv = parse_numerical_conventions(body).expect("parses");
        assert_eq!(conv.len(), 2);
        assert_eq!(conv[0].name, "default");
        assert_eq!(conv[0].q_format_default, "Q16.16");
        assert_eq!(conv[0].saturation_policy, "saturate");
        assert_eq!(conv[0].signed_default, "signed");
        assert_eq!(conv[0].rounding_mode, "round_half_even");
        assert!(conv[0].description.contains("Default numerical"));
        assert_eq!(conv[1].name, "synapse_permanence");
        assert_eq!(conv[1].q_format_default, "Q0.16");
    }
}
