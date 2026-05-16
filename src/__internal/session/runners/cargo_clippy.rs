//! `cargo clippy` diagnostic-summary extraction.
//!
//! Walks stderr, pulls out each `error:` / `warning:` block + its
//! `--> file:line:col` location, then groups identical (kind, header
//! message) pairs so a lint repeated at N sites surfaces as one
//! group with N locations rather than N separate entries.

/// Coalesced view of a failing `cargo clippy` run. `diagnostic_count`
/// is the total number of `error:` / `warning:` blocks parsed out
/// of stderr; `display` is the formatted summary the orchestrator
/// threads back into the next User turn.
#[derive(Debug, Clone)]
pub struct ClippyDiagSummary {
    pub diagnostic_count: usize,
    pub display: String,
}

/// Parse `cargo clippy` stderr into a coalesced summary. Each
/// diagnostic emits an `error:` or `warning:` header followed by a
/// `   --> file:line:col` location line and several lines of code-
/// snippet + help. Many diagnostics are the SAME lint repeated at
/// different sites; coalescing groups by header text and lists the
/// locations together so the agent sees `clippy::single_match: 12
/// occurrences across 4 files (sample: src/foo.rs:42)` rather than
/// 12 verbatim blocks.
///
/// Returns `None` when no diagnostic headers are found (caller
/// falls back to the raw stderr tail).
pub fn summarize_clippy_diagnostics(stdout: &str, stderr: &str) -> Option<ClippyDiagSummary> {
    // Clippy emits diagnostics on stderr. stdout typically just
    // shows the `Compiling ...` / `Checking ...` progress lines
    // (cut by `--quiet`) plus a final cargo summary; we keep it
    // trimmed for context but parse the stderr.
    let diagnostics = extract_clippy_diagnostics(stderr);
    if diagnostics.is_empty() {
        return None;
    }
    let diagnostic_count = diagnostics.len();

    // Group by (kind, message header). Locations differ per
    // occurrence; collect them per group.
    let mut groups: Vec<ClippyGroup> = Vec::new();
    for d in &diagnostics {
        let key = (d.kind, d.message.clone());
        if let Some(g) = groups.iter_mut().find(|g| g.key == key) {
            g.locations.push(d.location.clone());
        } else {
            groups.push(ClippyGroup {
                key,
                locations: vec![d.location.clone()],
            });
        }
    }

    let unique = groups.len();
    let mut out = String::new();
    out.push_str(&format!(
        "clippy diagnostics: {diagnostic_count} total, {unique} unique.\n\n",
    ));
    for g in &groups {
        let (kind, msg) = &g.key;
        let count = g.locations.len();
        let kind_str = match kind {
            ClippyKind::Error => "error",
            ClippyKind::Warning => "warning",
        };
        if count == 1 {
            out.push_str(&format!("- {kind_str}: {msg}\n  at {}\n", g.locations[0]));
        } else {
            // Show a sample location + count of additional sites.
            let sample = &g.locations[0];
            out.push_str(&format!(
                "- {kind_str}: {msg}\n  ({count} occurrences; sample: {sample})\n",
            ));
            // For small group sizes (<=6) list every location so
            // the agent can fix them in one pass.
            if count <= 6 {
                for loc in &g.locations[1..] {
                    out.push_str(&format!("    also: {loc}\n"));
                }
            }
        }
        out.push('\n');
    }
    // Preserve the final cargo summary line(s) verbatim if present
    // (e.g. "error: could not compile ..." or
    // "error: aborting due to N previous errors").
    for line in stderr.lines().rev().take(20) {
        if line.starts_with("error: aborting due to") || line.starts_with("error: could not") {
            out.push_str(line.trim());
            out.push('\n');
        }
    }
    let trimmed_stdout = stdout.trim();
    if !trimmed_stdout.is_empty() {
        out.push_str("\nstdout (tail):\n");
        out.push_str(trimmed_stdout);
        out.push('\n');
    }
    Some(ClippyDiagSummary {
        diagnostic_count,
        display: out,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClippyKind {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
struct ExtractedClippyDiag {
    kind: ClippyKind,
    /// First-line text of the diagnostic, with the `error: ` /
    /// `warning: ` prefix stripped and a trailing lint-name
    /// suffix (`#[deny(clippy::single_match)]`) preserved when
    /// present so identical lints group correctly.
    message: String,
    /// `file:line:col` form from the `--> ...` location line.
    location: String,
}

#[derive(Debug)]
struct ClippyGroup {
    key: (ClippyKind, String),
    locations: Vec<String>,
}

/// Walk clippy stderr extracting each diagnostic block. A block
/// opens with `error: ...` or `warning: ...` and contains a
/// `   --> file:line:col` location line. Blocks without a location
/// (terminal "could not compile" wrappers) are dropped here -- the
/// caller surfaces those separately as the trailing summary line.
fn extract_clippy_diagnostics(stderr: &str) -> Vec<ExtractedClippyDiag> {
    let mut out: Vec<ExtractedClippyDiag> = Vec::new();
    let lines: Vec<&str> = stderr.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let kind = if line.starts_with("error: ") {
            Some(ClippyKind::Error)
        } else if line.starts_with("warning: ") {
            Some(ClippyKind::Warning)
        } else {
            None
        };
        let Some(kind) = kind else {
            i += 1;
            continue;
        };
        // The "summary" diagnostic at the tail of clippy output --
        // `error: aborting due to N previous errors; M warnings
        // emitted` or `error: could not compile FOO due to N
        // previous errors` -- doesn't have a location line and is
        // accounted for separately. Detect by header shape so it
        // doesn't inflate the diagnostic count.
        let msg_first_line = line
            .trim_start_matches("error: ")
            .trim_start_matches("warning: ");
        if msg_first_line.starts_with("aborting due to")
            || msg_first_line.starts_with("could not compile")
        {
            i += 1;
            continue;
        }
        // Look for the `--> file:line:col` location line in the
        // next few non-blank lines. Real diagnostics have it
        // within 1-2 lines of the header; if missing entirely,
        // skip the block (treat as non-coalescable).
        let mut location: Option<String> = None;
        let mut j = i + 1;
        let scan_limit = (i + 5).min(lines.len());
        while j < scan_limit {
            let l = lines[j].trim_start();
            if let Some(rest) = l.strip_prefix("--> ") {
                location = Some(rest.trim().to_string());
                break;
            }
            j += 1;
        }
        let Some(location) = location else {
            i += 1;
            continue;
        };
        out.push(ExtractedClippyDiag {
            kind,
            message: msg_first_line.to_string(),
            location,
        });
        // Advance to the next blank line OR next header so we
        // don't double-count when the block has its own embedded
        // `note:` / `help:` lines.
        i = j + 1;
        while i < lines.len() {
            let l = lines[i];
            if l.is_empty() || l.starts_with("error: ") || l.starts_with("warning: ") {
                break;
            }
            i += 1;
        }
    }
    out
}
