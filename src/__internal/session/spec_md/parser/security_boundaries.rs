//! Parser for the `## Security Boundaries` section (Chapter 7
//! §7.7). Each privilege level is `### Privilege: <name>` with a
//! bold-property block (Id) plus optional `#### Description`
//! prose and `#### Capabilities` bullet list.

use super::SpecMdParseError;
use super::section_util::{
    collect_prose, collect_top_level_bullets, parse_bold_properties, split_h3, split_h4,
};
use crate::session::spec_md::types::PrivilegeLevel;

pub(crate) fn parse_security_boundaries(
    body: &str,
) -> Result<Vec<PrivilegeLevel>, SpecMdParseError> {
    let mut out: Vec<PrivilegeLevel> = Vec::new();
    let (_pre, subs) = split_h3(body);
    for sub in subs {
        let Some(name) = sub.heading.strip_prefix("Privilege:") else {
            continue;
        };
        let mut level = PrivilegeLevel {
            name: name.trim().to_string(),
            ..PrivilegeLevel::default()
        };
        let (preamble, h4s) = split_h4(&sub.body);
        for (k, v) in parse_bold_properties(&preamble) {
            if k.eq_ignore_ascii_case("id") {
                level.id = v;
            }
        }
        for h4 in h4s {
            match h4.heading.to_ascii_lowercase().as_str() {
                "description" => {
                    level.description = collect_prose(&h4.body);
                }
                "capabilities" => {
                    level.capabilities = collect_top_level_bullets(&h4.body);
                }
                _ => {}
            }
        }
        out.push(level);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_privilege_levels() {
        let body = "\
## Security Boundaries

### Privilege: Machine

**Id:** M

#### Description

Highest privilege level.

#### Capabilities

- access all CSRs
- configure interrupts

### Privilege: User

**Id:** U

#### Description

Unprivileged application code.
";
        let levels = parse_security_boundaries(body).expect("parses");
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].name, "Machine");
        assert_eq!(levels[0].id, "M");
        assert!(levels[0].description.contains("Highest"));
        assert_eq!(levels[0].capabilities.len(), 2);
        assert_eq!(levels[1].name, "User");
        assert_eq!(levels[1].id, "U");
    }
}
