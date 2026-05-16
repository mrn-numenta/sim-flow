//! Minimal template engine for `sim-flow new`.
//!
//! Copies a template directory (e.g.
//! `sim-foundation/tools/sim-flow/templates/model-project/`) into a
//! destination, performing `{{placeholder}}` substitution on every
//! file's *contents*. Filenames are also expanded so path components
//! can carry placeholders. Unknown placeholders are left untouched
//! so future generators (or hand edits) can fill them in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Templates directory, relative to the foundation (workspace) root.
/// Lives next to `prompts/` and `extensions/` under
/// `tools/sim-flow/` to keep all sim-flow-specific assets in one
/// tree.
pub const TEMPLATES_DIR: &str = "tools/sim-flow/templates";

/// Substitute `{{key}}` tokens in `text` using `values`. Unknown keys are
/// left in place verbatim.
pub fn substitute(text: &str, values: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let key = after[..end].trim();
            if let Some(value) = values.get(key) {
                out.push_str(value);
            } else {
                // Leave the token intact for downstream tooling.
                out.push_str("{{");
                out.push_str(&after[..end]);
                out.push_str("}}");
            }
            rest = &after[end + 2..];
        } else {
            // Dangling `{{` with no closing `}}` — copy and stop scanning.
            out.push_str("{{");
            out.push_str(after);
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

/// Copy `src` into `dst`, applying [`substitute`] to every file's contents
/// (and to every path segment). `dst` is created if it does not exist.
pub fn expand_into(src: &Path, dst: &Path, values: &BTreeMap<String, String>) -> Result<()> {
    if !src.is_dir() {
        return Err(Error::Io {
            path: src.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "template source directory not found",
            ),
        });
    }
    std::fs::create_dir_all(dst).map_err(|source| Error::Io {
        path: dst.to_path_buf(),
        source,
    })?;
    copy_dir(src, dst, values)
}

fn copy_dir(src: &Path, dst: &Path, values: &BTreeMap<String, String>) -> Result<()> {
    for entry in std::fs::read_dir(src).map_err(|source| Error::Io {
        path: src.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| Error::Io {
            path: src.to_path_buf(),
            source,
        })?;
        let ftype = entry.file_type().map_err(|source| Error::Io {
            path: entry.path(),
            source,
        })?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // The template manifest is meta; do not copy it into the
        // generated project.
        if name_str == "template.toml" {
            continue;
        }
        let expanded_name = substitute(name_str.as_ref(), values);
        let from = entry.path();
        let to = dst.join(&expanded_name);
        if ftype.is_dir() {
            std::fs::create_dir_all(&to).map_err(|source| Error::Io {
                path: to.clone(),
                source,
            })?;
            copy_dir(&from, &to, values)?;
        } else {
            copy_file(&from, &to, values)?;
        }
    }
    Ok(())
}

fn copy_file(src: &Path, dst: &Path, values: &BTreeMap<String, String>) -> Result<()> {
    let bytes = std::fs::read(src).map_err(|source| Error::Io {
        path: src.to_path_buf(),
        source,
    })?;
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    // Substitute on text content only; binary files pass through untouched.
    match std::str::from_utf8(&bytes) {
        Ok(text) => {
            let expanded = substitute(text, values);
            std::fs::write(dst, expanded).map_err(|source| Error::Io {
                path: dst.to_path_buf(),
                source,
            })
        }
        Err(_) => std::fs::write(dst, bytes).map_err(|source| Error::Io {
            path: dst.to_path_buf(),
            source,
        }),
    }
}

/// Convert a human-facing project name (e.g. "my-model") to a valid cargo
/// crate identifier (snake_case, ASCII, leading digit stripped).
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

/// Convenience: build the standard placeholder map for a generated
/// project.
pub fn default_placeholders(
    project_name: &str,
    foundation_root: &Path,
    library_path: &str,
) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("project-name".into(), project_name.to_string());
    m.insert("crate_name".into(), crate_name(project_name));
    m.insert(
        "foundation_path".into(),
        foundation_root.display().to_string(),
    );
    m.insert("library_path".into(), library_path.to_string());
    m.insert("timestamp".into(), utc_timestamp_now());
    m
}

/// Resolve the path to a named template inside the foundation root.
pub fn template_path(foundation_root: &Path, template_name: &str) -> PathBuf {
    foundation_root.join(TEMPLATES_DIR).join(template_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_known_tokens() {
        let mut v = BTreeMap::new();
        v.insert("name".into(), "ring".into());
        assert_eq!(substitute("hello {{name}}!", &v), "hello ring!");
    }

    #[test]
    fn leaves_unknown_tokens_intact() {
        let v = BTreeMap::new();
        assert_eq!(substitute("{{unknown}}", &v), "{{unknown}}");
    }

    #[test]
    fn handles_dangling_open_brace() {
        let v = BTreeMap::new();
        assert_eq!(substitute("partial {{stuff", &v), "partial {{stuff");
    }

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
    fn expand_into_copies_tree_and_substitutes() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("sub")).unwrap();
        std::fs::write(src.path().join("root.txt"), "hello {{who}}").unwrap();
        std::fs::write(src.path().join("sub").join("nested.txt"), "{{who}}!").unwrap();

        let mut values = BTreeMap::new();
        values.insert("who".into(), "world".into());
        expand_into(src.path(), dst.path(), &values).unwrap();

        let root = std::fs::read_to_string(dst.path().join("root.txt")).unwrap();
        assert_eq!(root, "hello world");
        let nested = std::fs::read_to_string(dst.path().join("sub").join("nested.txt")).unwrap();
        assert_eq!(nested, "world!");
    }

    #[test]
    fn expand_into_skips_template_toml() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("template.toml"), "meta").unwrap();
        std::fs::write(src.path().join("keep.txt"), "body").unwrap();
        expand_into(src.path(), dst.path(), &BTreeMap::new()).unwrap();
        assert!(!dst.path().join("template.toml").exists());
        assert!(dst.path().join("keep.txt").exists());
    }

    #[test]
    fn filename_placeholders_expand() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("{{crate_name}}.rs"), "fn main(){}").unwrap();
        let mut v = BTreeMap::new();
        v.insert("crate_name".into(), "foo".into());
        expand_into(src.path(), dst.path(), &v).unwrap();
        assert!(dst.path().join("foo.rs").exists());
    }

    #[test]
    fn expand_into_missing_source_returns_io_error() {
        let dst = tempfile::tempdir().unwrap();
        let v = BTreeMap::new();
        let nowhere = std::path::Path::new("/this/does/not/exist/at/all");
        assert!(expand_into(nowhere, dst.path(), &v).is_err());
    }

    #[test]
    fn template_path_joins_under_foundation_templates_dir() {
        let p = template_path(std::path::Path::new("/abs/foundation"), "model-project");
        assert!(p.ends_with("tools/sim-flow/templates/model-project"));
    }

    #[test]
    fn default_placeholders_populates_canonical_keys() {
        let m = default_placeholders(
            "demo-model",
            std::path::Path::new("/abs/foundation"),
            "../sim-models/library",
        );
        assert_eq!(
            m.get("project-name").map(String::as_str),
            Some("demo-model")
        );
        assert_eq!(m.get("crate_name").map(String::as_str), Some("demo_model"));
        assert_eq!(
            m.get("foundation_path").map(String::as_str),
            Some("/abs/foundation"),
        );
        assert_eq!(
            m.get("library_path").map(String::as_str),
            Some("../sim-models/library"),
        );
        // Timestamp must be present + non-empty (full shape tested elsewhere).
        assert!(m.get("timestamp").is_some_and(|t| !t.is_empty()));
    }

    #[test]
    fn is_leap_handles_century_and_400_year_edges() {
        // 2000 is a leap year (divisible by 400).
        assert!(is_leap(2000));
        // 1900 is NOT (divisible by 100 but not 400).
        assert!(!is_leap(1900));
        // 2024 is leap; 2025 is not.
        assert!(is_leap(2024));
        assert!(!is_leap(2025));
    }
}
