//! DM0 gate-check.
//!
//! Replaces the regex-based gate dispatch with a parser-driven
//! check: parse `docs/spec.md` via Phase 1, run
//! `SpecMd::validate()`, verify quantitative-section regexes,
//! resolve every source-anchor against the ingest manifest, and
//! (in automated mode) verify Auto-decisions were populated.
//!
//! Owned by Phase 6 Stream C.

use std::collections::HashSet;
use std::path::Path;

use regex::Regex;

use crate::__internal::session::ask_user::mode_flip::read_current_step_mode;
use crate::__internal::session::protocol::StepMode;
use crate::__internal::session::spec_md::{
    self, IssueSeverity, SourceSpecAnchor, SpecMd, ValidationIssue,
};
use crate::Result;

/// Aggregate outcome of a DM0 gate-check. The `failures` vector
/// mirrors `gate::GateFailure` but is computed against the new
/// structured parser instead of regex matchers. The orchestrator
/// converts this into the existing `gate::GateReport` for protocol
/// emission.
#[derive(Debug, Clone, Default)]
pub struct Dm0GateOutcome {
    pub failures: Vec<Dm0GateFailure>,
}

impl Dm0GateOutcome {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct Dm0GateFailure {
    /// Short human-readable label (e.g. `"missing-clock-frequency"`).
    pub code: String,
    /// One-sentence description of what failed and where.
    pub message: String,
}

/// Evaluate the DM0 gate against the project's current `spec.md`
/// and ingest manifest. The manifest is optional — for no-source
/// projects the manifest path may not exist; in that case
/// anchor-resolution checks are skipped but the structural / regex
/// checks still run.
///
/// `project_dir` is the project root and is used to resolve
/// `read_current_step_mode` (so the Auto-decisions check fires only
/// in automated mode). When `None`, the mode check is skipped — the
/// caller is presumed to be a synthetic test invocation.
pub fn check_dm0_gate(
    spec_md_path: &Path,
    manifest_path: Option<&Path>,
    project_dir: Option<&Path>,
) -> Result<Dm0GateOutcome> {
    let mut outcome = Dm0GateOutcome::default();

    // 1. Read + parse spec.md. Hard fail on missing file or parse
    //    error -- nothing else can run if the document is broken.
    let body = match std::fs::read_to_string(spec_md_path) {
        Ok(b) => b,
        Err(err) => {
            outcome.failures.push(Dm0GateFailure {
                code: "spec-md-missing".to_string(),
                message: format!("cannot read spec.md at {}: {err}", spec_md_path.display()),
            });
            return Ok(outcome);
        }
    };
    let spec = match spec_md::parse(&body) {
        Ok(s) => s,
        Err(err) => {
            outcome.failures.push(Dm0GateFailure {
                code: "parse-error".to_string(),
                message: format!("spec.md parse error: {err}"),
            });
            return Ok(outcome);
        }
    };

    // 2. Cross-reference / required-row validation from Phase 1.
    //    Errors become Dm0GateFailures; warnings are silently
    //    dropped here (they surface via the parser warning channel
    //    elsewhere).
    for issue in spec.validate() {
        if matches!(issue.severity, IssueSeverity::Error) {
            outcome.failures.push(failure_from_issue(&issue));
        }
    }

    // 3. Explicit quantitative-row regex checks. `validate()` covers
    //    the "row exists" half; we restate the regex checks
    //    explicitly here so the failure codes match the gate-engine
    //    contract (`missing-clock-frequency` /
    //    `missing-gate-budget`).
    let clock_re = Regex::new(r"\d+\s*(MHz|GHz)").unwrap();
    let gate_re = Regex::new(r"\d+").unwrap();
    let mut have_clock = false;
    let mut have_gate = false;
    for row in &spec.assumptions.quantitative {
        let key = row.constraint.to_ascii_lowercase();
        if key.contains("clock frequency") && clock_re.is_match(&row.value) {
            have_clock = true;
        }
        if (key.contains("gate budget per cycle") || key.contains("gate budget"))
            && gate_re.is_match(&row.value)
        {
            have_gate = true;
        }
    }
    if !have_clock
        && !outcome
            .failures
            .iter()
            .any(|f| f.code == "missing-clock-frequency")
    {
        outcome.failures.push(Dm0GateFailure {
            code: "missing-clock-frequency".to_string(),
            message: "Assumptions > Quantitative is missing a `Clock frequency` \
                      row whose value matches `\\d+\\s*(MHz|GHz)`"
                .to_string(),
        });
    }
    if !have_gate
        && !outcome
            .failures
            .iter()
            .any(|f| f.code == "missing-gate-budget")
    {
        outcome.failures.push(Dm0GateFailure {
            code: "missing-gate-budget".to_string(),
            message: "Assumptions > Quantitative is missing a `Gate budget per cycle` \
                      row whose value contains a number"
                .to_string(),
        });
    }

    // 4. Anchor resolution against the ingest manifest. The manifest
    //    gives us the set of valid `<source>` identifiers (primary +
    //    each peer.id). When the manifest is absent, we emit a
    //    soft warning failure and skip the per-anchor pass -- the
    //    no-source-spec case is legitimate.
    match manifest_path {
        Some(mp) if mp.exists() => match collect_known_sources(mp) {
            Ok(sources) => {
                for (loc, anchor_str) in iter_anchors(&spec) {
                    if anchor_str.trim().is_empty() {
                        continue;
                    }
                    let parsed = match SourceSpecAnchor::parse(&anchor_str) {
                        Ok(p) => p,
                        // Already covered by `validate()` -> BadAnchor;
                        // skip to avoid duplicate failures.
                        Err(_) => continue,
                    };
                    let source = match &parsed {
                        SourceSpecAnchor::Page { source, .. }
                        | SourceSpecAnchor::PageRange { source, .. }
                        | SourceSpecAnchor::Chunk { source, .. } => source,
                    };
                    if !sources.contains(source.as_str()) {
                        outcome.failures.push(Dm0GateFailure {
                            code: "unresolved-anchor".to_string(),
                            message: format!(
                                "{loc}: source-anchor `{anchor_str}` references \
                                 source `{source}` which is not registered in the \
                                 ingest manifest"
                            ),
                        });
                    }
                }
            }
            Err(err) => {
                outcome.failures.push(Dm0GateFailure {
                    code: "manifest-unreadable".to_string(),
                    message: format!(
                        "cannot resolve source-anchors: failed to parse ingest \
                         manifest at {}: {err}",
                        mp.display()
                    ),
                });
            }
        },
        Some(mp) => {
            outcome.failures.push(Dm0GateFailure {
                code: "manifest-missing".to_string(),
                message: format!(
                    "ingest manifest at {} not found; source-anchor resolution \
                     skipped (re-run `sim-flow ingest` or pass --manifest=none)",
                    mp.display()
                ),
            });
        }
        None => {
            // No manifest expected (no-source-spec project). Anchor
            // resolution is skipped; the structural / regex checks
            // above still ran.
        }
    }

    // 5. Auto-decisions populated in automated mode. The mode flag
    //    lives in `<project>/.sim-flow/state.toml`; absence defaults
    //    to manual, so the check fires only when state.toml says
    //    `current_step_mode = "auto"`.
    if let Some(pd) = project_dir
        && matches!(read_current_step_mode(pd), StepMode::Auto)
        && spec.auto_decisions.is_empty()
    {
        outcome.failures.push(Dm0GateFailure {
            code: "missing-auto-decisions".to_string(),
            message: "running in automated mode but `## Auto-decisions` is empty; \
                      every non-trivial inference DM0 made should land as an \
                      Auto-decision row"
                .to_string(),
        });
    }

    Ok(outcome)
}

/// Convert a Phase 1 `ValidationIssue` (Error severity) into a
/// `Dm0GateFailure`. The `code` is a short stable label the
/// orchestrator's emission layer can branch on; the message is the
/// issue's location + message verbatim.
fn failure_from_issue(issue: &ValidationIssue) -> Dm0GateFailure {
    let code = if issue.message.contains("Clock frequency") {
        "missing-clock-frequency"
    } else if issue.message.contains("Gate budget per cycle")
        || issue.message.contains("Gate budget")
    {
        "missing-gate-budget"
    } else if issue.message.contains("malformed source anchor") {
        "malformed-anchor"
    } else if issue.message.contains("parent") {
        "bad-block-parent"
    } else {
        "validation-error"
    };
    Dm0GateFailure {
        code: code.to_string(),
        message: format!("{}: {}", issue.location, issue.message),
    }
}

/// Walk every source-anchor string declared in the spec, yielding
/// `(location, anchor_text)` pairs. Mirrors the locations
/// `validate.rs::validate_anchors` walks.
fn iter_anchors(spec: &SpecMd) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for row in &spec.assumptions.quantitative {
        if !row.source_anchor.trim().is_empty() {
            out.push((
                format!("Assumptions > Quantitative > {}", row.constraint),
                row.source_anchor.clone(),
            ));
        }
    }
    for iface in &spec.external_interfaces {
        for a in &iface.source_anchors {
            if !a.trim().is_empty() {
                out.push((format!("External Interfaces > {}", iface.name), a.clone()));
            }
        }
    }
    for block in &spec.blocks {
        for a in &block.source_anchors {
            if !a.trim().is_empty() {
                out.push((format!("Blocks > {}", block.name), a.clone()));
            }
        }
    }
    for p in &spec.parameters {
        if !p.source_anchor.trim().is_empty() {
            out.push((format!("Parameters > {}", p.name), p.source_anchor.clone()));
        }
    }
    for fsm in &spec.state_machines {
        if !fsm.source_anchor.trim().is_empty() {
            out.push((
                format!("State Machines > {}", fsm.name),
                fsm.source_anchor.clone(),
            ));
        }
    }
    for e in &spec.encodings {
        if !e.source_anchor.trim().is_empty() {
            out.push((format!("Encodings > {}", e.field), e.source_anchor.clone()));
        }
    }
    for m in &spec.memory_map {
        if !m.source_anchor.trim().is_empty() {
            out.push((format!("Memory Map > {}", m.name), m.source_anchor.clone()));
        }
    }
    if let Some(c) = &spec.connectivity {
        for e in &c.edges {
            if !e.source_anchor.trim().is_empty() {
                out.push((
                    format!("Connectivity > Edges > {} -> {}", e.from, e.to),
                    e.source_anchor.clone(),
                ));
            }
        }
    }
    for e in &spec.error_handling {
        if !e.source_anchor.trim().is_empty() {
            out.push((
                format!("Error Handling > {}", e.error_type),
                e.source_anchor.clone(),
            ));
        }
    }
    for op in &spec.functional_behavior.operations {
        if !op.source_anchor.trim().is_empty() {
            out.push((
                format!("Functional Behavior > Operation > {}", op.id),
                op.source_anchor.clone(),
            ));
        }
    }
    for sc in &spec.cycle_accurate {
        if !sc.source_anchor.trim().is_empty() {
            out.push((
                format!("Cycle-Accurate Behavior > {}", sc.name),
                sc.source_anchor.clone(),
            ));
        }
    }
    // The `## Source-Spec Anchors` index uses
    // `<source>:chunk-<id>` shorthand; treat each row as an anchor
    // pair so the source half resolves.
    for entry in &spec.source_spec_anchors {
        if entry.source.trim().is_empty() {
            continue;
        }
        let synthesized = format!("{}:chunk-{}", entry.source, entry.chunk_id);
        out.push((
            format!("Source-Spec Anchors > {}", entry.section_path),
            synthesized,
        ));
    }
    out
}

/// Read the ingest manifest and collect the set of valid
/// `<source>` identifiers (the literal `primary` plus every
/// `[[peers]].id`). The manifest is line-oriented TOML; we use
/// `toml::Value` so partial / unknown fields are tolerated.
fn collect_known_sources(manifest_path: &Path) -> std::io::Result<HashSet<String>> {
    let body = std::fs::read_to_string(manifest_path)?;
    let mut sources: HashSet<String> = HashSet::new();
    sources.insert("primary".to_string());
    let value: toml::Value =
        toml::from_str(&body).map_err(|e| std::io::Error::other(format!("parse manifest: {e}")))?;
    if let Some(peers) = value.get("peers").and_then(|v| v.as_array()) {
        for peer in peers {
            if let Some(id) = peer.get("id").and_then(|v| v.as_str()) {
                sources.insert(id.to_string());
            }
        }
    }
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::__internal::session::ask_user::mode_flip::write_current_step_mode;
    use crate::__internal::session::spec_md::types::{
        AssumptionsAndConstraints, AutoDecision, Block, QuantitativeRow,
    };
    use tempfile::tempdir;

    fn write_spec(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("spec.md");
        std::fs::write(&path, body).unwrap();
        path
    }

    fn write_manifest(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("manifest.toml");
        std::fs::write(
            &path,
            "schema_version = 1\nsource_kind = \"markdown\"\n\n\
             [[peers]]\nid = \"tm-spec\"\nsource_path = \"x\"\nsource_sha256 = \"y\"\n",
        )
        .unwrap();
        path
    }

    fn minimal_valid_body() -> String {
        // Build the minimal valid SpecMd from the Rust types and
        // serialize it via the writer so the parser round-trips
        // cleanly. This avoids hand-authoring markdown that the
        // parser's heading dispatch might disagree with.
        let spec = SpecMd {
            assumptions: AssumptionsAndConstraints {
                quantitative: vec![
                    QuantitativeRow {
                        constraint: "Clock frequency".into(),
                        value: "1 GHz".into(),
                        source_anchor: "primary:p3".into(),
                    },
                    QuantitativeRow {
                        constraint: "Gate budget per cycle".into(),
                        value: "50".into(),
                        source_anchor: "primary:p3".into(),
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        spec.to_markdown()
    }

    #[test]
    fn synthetic_valid_spec_passes() {
        let tmp = tempdir().unwrap();
        let spec_path = write_spec(tmp.path(), &minimal_valid_body());
        let manifest = write_manifest(tmp.path());
        let outcome = check_dm0_gate(&spec_path, Some(&manifest), None).expect("gate runs");
        assert!(
            outcome.is_clean(),
            "expected clean outcome, got: {:?}",
            outcome.failures
        );
    }

    #[test]
    fn missing_clock_frequency_fails() {
        let spec = SpecMd {
            assumptions: AssumptionsAndConstraints {
                quantitative: vec![QuantitativeRow {
                    constraint: "Gate budget per cycle".into(),
                    value: "50".into(),
                    source_anchor: "primary:p3".into(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let tmp = tempdir().unwrap();
        let spec_path = write_spec(tmp.path(), &spec.to_markdown());
        let manifest = write_manifest(tmp.path());
        let outcome = check_dm0_gate(&spec_path, Some(&manifest), None).unwrap();
        assert!(
            outcome
                .failures
                .iter()
                .any(|f| f.code == "missing-clock-frequency"),
            "expected missing-clock-frequency failure, got: {:?}",
            outcome.failures
        );
    }

    #[test]
    fn unresolved_anchor_fails() {
        // Anchor cites a source ID (`ghost-spec`) that the manifest
        // does not list. The anchor itself parses cleanly (so
        // validate() doesn't flag it as malformed); the resolution
        // step catches it.
        let spec = SpecMd {
            assumptions: AssumptionsAndConstraints {
                quantitative: vec![
                    QuantitativeRow {
                        constraint: "Clock frequency".into(),
                        value: "1 GHz".into(),
                        source_anchor: "ghost-spec:p5".into(),
                    },
                    QuantitativeRow {
                        constraint: "Gate budget per cycle".into(),
                        value: "50".into(),
                        source_anchor: "primary:p3".into(),
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        let tmp = tempdir().unwrap();
        let spec_path = write_spec(tmp.path(), &spec.to_markdown());
        let manifest = write_manifest(tmp.path());
        let outcome = check_dm0_gate(&spec_path, Some(&manifest), None).unwrap();
        assert!(
            outcome
                .failures
                .iter()
                .any(|f| f.code == "unresolved-anchor"),
            "expected unresolved-anchor failure, got: {:?}",
            outcome.failures
        );
    }

    #[test]
    fn unresolved_anchor_skipped_without_manifest() {
        // Same `ghost-spec:p5` anchor; manifest path is None, so the
        // anchor-resolution step is skipped and the gate stays
        // clean. The malformed-anchor path is NOT triggered because
        // the anchor parses syntactically.
        let spec = SpecMd {
            assumptions: AssumptionsAndConstraints {
                quantitative: vec![
                    QuantitativeRow {
                        constraint: "Clock frequency".into(),
                        value: "1 GHz".into(),
                        source_anchor: "ghost-spec:p5".into(),
                    },
                    QuantitativeRow {
                        constraint: "Gate budget per cycle".into(),
                        value: "50".into(),
                        source_anchor: "ghost-spec:p3".into(),
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        let tmp = tempdir().unwrap();
        let spec_path = write_spec(tmp.path(), &spec.to_markdown());
        let outcome = check_dm0_gate(&spec_path, None, None).unwrap();
        assert!(
            outcome.is_clean(),
            "expected clean outcome with no manifest, got: {:?}",
            outcome.failures
        );
    }

    #[test]
    fn missing_auto_decisions_in_automated_mode_fails() {
        let tmp = tempdir().unwrap();
        // Write the auto-mode flag for this project.
        write_current_step_mode(tmp.path(), StepMode::Auto).unwrap();
        let spec_path = write_spec(tmp.path(), &minimal_valid_body());
        let manifest = write_manifest(tmp.path());
        let outcome = check_dm0_gate(&spec_path, Some(&manifest), Some(tmp.path())).unwrap();
        assert!(
            outcome
                .failures
                .iter()
                .any(|f| f.code == "missing-auto-decisions"),
            "expected missing-auto-decisions failure, got: {:?}",
            outcome.failures
        );
    }

    #[test]
    fn auto_mode_with_populated_auto_decisions_passes() {
        let tmp = tempdir().unwrap();
        write_current_step_mode(tmp.path(), StepMode::Auto).unwrap();
        let mut spec = SpecMd {
            assumptions: AssumptionsAndConstraints {
                quantitative: vec![
                    QuantitativeRow {
                        constraint: "Clock frequency".into(),
                        value: "1 GHz".into(),
                        source_anchor: "primary:p3".into(),
                    },
                    QuantitativeRow {
                        constraint: "Gate budget per cycle".into(),
                        value: "50".into(),
                        source_anchor: "primary:p3".into(),
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        spec.auto_decisions.push(AutoDecision {
            decision: "XLEN default = 32".into(),
            rationale: "embedded-market default".into(),
        });
        let spec_path = write_spec(tmp.path(), &spec.to_markdown());
        let manifest = write_manifest(tmp.path());
        let outcome = check_dm0_gate(&spec_path, Some(&manifest), Some(tmp.path())).unwrap();
        assert!(
            outcome.is_clean(),
            "expected clean outcome, got: {:?}",
            outcome.failures
        );
    }

    #[test]
    fn parse_error_is_hard_failure() {
        // `## Quantitative` table with the wrong column count is a
        // MalformedTable parse error that the per-section parser
        // raises before validate() runs.
        let body = "# Test\n\n\
                    ## Assumptions and Constraints\n\n\
                    ### Quantitative\n\n\
                    | Constraint | Value |\n\
                    | --- | --- |\n\
                    | Clock frequency | 1 GHz |\n";
        let tmp = tempdir().unwrap();
        let spec_path = write_spec(tmp.path(), body);
        let outcome = check_dm0_gate(&spec_path, None, None).unwrap();
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].code, "parse-error");
    }

    #[test]
    fn missing_spec_file_fails_cleanly() {
        let tmp = tempdir().unwrap();
        let spec_path = tmp.path().join("does-not-exist.md");
        let outcome = check_dm0_gate(&spec_path, None, None).unwrap();
        assert!(outcome.failures.iter().any(|f| f.code == "spec-md-missing"));
    }

    #[test]
    fn dropped_warning_anchor_does_not_double_report() {
        // A malformed anchor surfaces as `malformed-anchor` from the
        // validate() step. The anchor-resolution step parses the
        // anchor and bails (it cannot resolve unparseable input), so
        // we should see exactly one failure, not two.
        let spec = SpecMd {
            assumptions: AssumptionsAndConstraints {
                quantitative: vec![
                    QuantitativeRow {
                        constraint: "Clock frequency".into(),
                        value: "1 GHz".into(),
                        source_anchor: "not-a-real-anchor".into(),
                    },
                    QuantitativeRow {
                        constraint: "Gate budget per cycle".into(),
                        value: "50".into(),
                        source_anchor: "primary:p3".into(),
                    },
                ],
                ..Default::default()
            },
            blocks: vec![Block {
                name: "X".into(),
                parent: "(none -- top-level)".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let tmp = tempdir().unwrap();
        let spec_path = write_spec(tmp.path(), &spec.to_markdown());
        let manifest = write_manifest(tmp.path());
        let outcome = check_dm0_gate(&spec_path, Some(&manifest), None).unwrap();
        let malformed: Vec<_> = outcome
            .failures
            .iter()
            .filter(|f| f.code == "malformed-anchor")
            .collect();
        assert_eq!(malformed.len(), 1, "got: {:?}", outcome.failures);
    }
}
