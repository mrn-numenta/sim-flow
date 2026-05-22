//! Template helpers shared by `sim-flow new`, the tracking writer, and
//! anywhere else that needs a snake_case crate identifier or an
//! ISO-8601 timestamp.
//!
//! The actual template expansion is delegated to `cargo generate`
//! (see [`crate::new_project`]); the helpers here just compute the
//! placeholder values cargo-generate is then fed via `--define`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Templates directory, relative to the sim-flow crate root. Lives
/// next to `prompts/` and `extensions/` at the repo root; all
/// sim-flow-specific assets sit in one tree.
pub const TEMPLATES_DIR: &str = "templates";
pub const SIM_FOUNDATION_GIT_URL: &str = "ssh://git@github.com/NumentaCorp/sim-foundation.git";
pub const SIM_FLOW_GIT_URL: &str = "ssh://git@github.com/NumentaCorp/sim-flow.git";

/// Convert a human-facing project name (e.g. "my-model") to a valid cargo
/// crate identifier (snake_case, ASCII, leading digit stripped).
///
/// Mirrors cargo-generate's built-in `crate_name` derivation so the
/// placeholder map we hand to `--define` agrees with what
/// cargo-generate computes internally. Used by [`default_placeholders`]
/// and by [`crate::new_project`] to surface the derived name back to
/// the CLI caller.
pub fn crate_name(project_name: &str) -> String {
    let mut out = String::with_capacity(project_name.len());
    for ch in project_name.chars() {
        let mapped = match ch {
            'a'..='z' | '0'..='9' | '_' => ch,
            'A'..='Z' => ch.to_ascii_lowercase(),
            '-' | ' ' | '.' | '/' => '_',
            _ => continue,
        };
        out.push(mapped);
    }
    // Cargo identifiers cannot start with a digit.
    if out
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(true)
    {
        out.insert(0, '_');
    }
    out
}

pub fn foundation_rev() -> &'static str {
    env!("SIM_FLOW_FOUNDATION_REV")
}

pub fn sim_flow_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn sim_flow_rev() -> &'static str {
    env!("SIM_FLOW_GIT_REV")
}

/// Produce an ISO-8601 UTC timestamp suitable for the `{{timestamp}}`
/// placeholder. No external dep — implemented against `SystemTime`.
pub fn utc_timestamp_now() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs() as i64;
    format_iso8601(secs)
}

fn format_iso8601(mut secs: i64) -> String {
    // Gregorian calendar conversion valid for 1970-01-01 and later.
    let time_of_day = secs.rem_euclid(86_400) as u32;
    secs = secs.div_euclid(86_400);
    let hour = time_of_day / 3600;
    let minute = (time_of_day / 60) % 60;
    let second = time_of_day % 60;
    let (year, month, day) = days_to_date(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn days_to_date(mut days: i64) -> (i64, u32, u32) {
    // Days since 1970-01-01.
    let mut year: i64 = 1970;
    loop {
        let year_len = if is_leap(year) { 366 } else { 365 };
        if days < year_len {
            break;
        }
        days -= year_len;
        year += 1;
    }
    let months = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 0usize;
    while month < 12 && days >= months[month] {
        days -= months[month];
        month += 1;
    }
    (year, (month + 1) as u32, (days + 1) as u32)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Build the standard placeholder map for a generated project. The
/// `project-name` / `crate_name` entries are informational -- they
/// match cargo-generate's built-in derivations from `--name` and the
/// caller can read them back without re-running the same logic. The
/// custom keys carry the per-project provenance sim-flow stamps into
/// the generated manifest.
pub fn default_placeholders(project_name: &str, library_path: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("project-name".into(), project_name.to_string());
    m.insert("crate_name".into(), crate_name(project_name));
    m.insert("foundation_repo".into(), SIM_FOUNDATION_GIT_URL.to_string());
    m.insert("foundation_rev".into(), foundation_rev().to_string());
    m.insert("library_path".into(), library_path.to_string());
    m.insert("sim_flow_repo".into(), SIM_FLOW_GIT_URL.to_string());
    m.insert("sim_flow_rev".into(), sim_flow_rev().to_string());
    m.insert("sim_flow_version".into(), sim_flow_version().to_string());
    m.insert("timestamp".into(), utc_timestamp_now());
    m
}

/// Resolve the path to a named template inside the sim-flow root.
pub fn template_path(sim_flow_root: &Path, template_name: &str) -> PathBuf {
    sim_flow_root.join(TEMPLATES_DIR).join(template_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_snake_case() {
        assert_eq!(crate_name("my-model"), "my_model");
        assert_eq!(crate_name("My Model 42"), "my_model_42");
        assert_eq!(crate_name("42-bad"), "_42_bad");
    }

    #[test]
    fn iso_timestamp_is_well_formed() {
        let ts = utc_timestamp_now();
        // YYYY-MM-DDTHH:MM:SSZ
        assert_eq!(ts.len(), 20);
        assert!(ts.ends_with('Z'));
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], "T");
    }

    #[test]
    fn template_path_joins_under_sim_flow_templates_dir() {
        let p = template_path(std::path::Path::new("/abs/sim-flow"), "model-project");
        assert!(p.ends_with("templates/model-project"));
    }

    #[test]
    fn default_placeholders_populates_canonical_keys() {
        let m = default_placeholders("demo-model", "../sim-models/library");
        assert_eq!(
            m.get("project-name").map(String::as_str),
            Some("demo-model")
        );
        assert_eq!(m.get("crate_name").map(String::as_str), Some("demo_model"));
        assert_eq!(
            m.get("foundation_repo").map(String::as_str),
            Some(SIM_FOUNDATION_GIT_URL),
        );
        assert_eq!(
            m.get("foundation_rev").map(String::as_str),
            Some(foundation_rev()),
        );
        assert_eq!(
            m.get("library_path").map(String::as_str),
            Some("../sim-models/library"),
        );
        assert_eq!(
            m.get("sim_flow_repo").map(String::as_str),
            Some(SIM_FLOW_GIT_URL),
        );
        assert_eq!(
            m.get("sim_flow_rev").map(String::as_str),
            Some(sim_flow_rev()),
        );
        assert_eq!(
            m.get("sim_flow_version").map(String::as_str),
            Some(sim_flow_version()),
        );
        assert!(m.get("timestamp").is_some_and(|t| !t.is_empty()));
    }

    #[test]
    fn is_leap_handles_century_and_400_year_edges() {
        assert!(is_leap(2000));
        assert!(!is_leap(1900));
        assert!(is_leap(2024));
        assert!(!is_leap(2025));
    }
}
