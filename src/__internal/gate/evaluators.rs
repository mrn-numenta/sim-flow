//! Per-check evaluators for the gate.
//!
//! `evaluate_one` is the dispatch over `GateCheck` variants — each
//! arm runs one check and produces a `GateFailure` (or `Ok(None)`
//! when the check passes). Two helpers live alongside:
//!
//! - `marker` renders a `Finding` variant to its on-disk prefix
//!   token (`BLOCKER` / `UNRESOLVED` / `RESOLVED`).
//! - `expand_candidate_files` resolves a `GateCheck::AnyExists` /
//!   `AnyMatches` `paths` list (file entries pass through; directory
//!   entries become every non-index `*.md` inside).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use regex::Regex;

use crate::critique::Critique;
use crate::{Error, Result};

use super::{GateCheck, GateFailure};

pub(super) fn evaluate_one(project_dir: &Path, check: &GateCheck) -> Result<Option<GateFailure>> {
    match check {
        GateCheck::FileExists { path, description } => {
            let full = project_dir.join(path);
            if !full.exists() {
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("file missing: {}", full.display()),
                }));
            }
            let meta = std::fs::metadata(&full).map_err(|source| Error::Io {
                path: full.clone(),
                source,
            })?;
            if meta.len() == 0 {
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("file is empty: {}", full.display()),
                }));
            }
            Ok(None)
        }
        GateCheck::FileMatches {
            path,
            pattern,
            description,
        } => {
            let full = project_dir.join(path);
            let text = match std::fs::read_to_string(&full) {
                Ok(t) => t,
                Err(source) => {
                    return Ok(Some(GateFailure {
                        description: description.clone(),
                        reason: format!("cannot read {}: {}", full.display(), source),
                    }));
                }
            };
            let regex = Regex::new(pattern).map_err(|e| {
                Error::Gate(format!("invalid regex in gate check {pattern:?}: {e}"))
            })?;
            if !regex.is_match(&text) {
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("pattern {pattern:?} not found in {}", full.display()),
                }));
            }
            Ok(None)
        }
        GateCheck::AnyExists { paths, description } => {
            let candidates = expand_candidate_files(project_dir, paths);
            for full in &candidates {
                if let Ok(meta) = std::fs::metadata(full)
                    && meta.is_file()
                    && meta.len() > 0
                {
                    return Ok(None);
                }
            }
            Ok(Some(GateFailure {
                description: description.clone(),
                reason: format!(
                    "no non-empty file found in any of: {}",
                    paths
                        .iter()
                        .map(|p| project_dir.join(p).display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }))
        }
        GateCheck::AnyMatches {
            paths,
            pattern,
            description,
        } => {
            let regex = Regex::new(pattern).map_err(|e| {
                Error::Gate(format!("invalid regex in gate check {pattern:?}: {e}"))
            })?;
            let candidates = expand_candidate_files(project_dir, paths);
            if candidates.is_empty() {
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!(
                        "no candidate files exist for any of: {}",
                        paths
                            .iter()
                            .map(|p| project_dir.join(p).display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                }));
            }
            for full in &candidates {
                if let Ok(text) = std::fs::read_to_string(full)
                    && regex.is_match(&text)
                {
                    return Ok(None);
                }
            }
            Ok(Some(GateFailure {
                description: description.clone(),
                reason: format!(
                    "pattern {pattern:?} not found in any of: {}",
                    candidates
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }))
        }
        GateCheck::Shell {
            cmd,
            args,
            description,
        } => {
            // Safety contract: `cmd` is NOT validated here. It is
            // safe today because every `GateCheck::Shell` value in
            // existence is constructed from compile-time string
            // literals in `steps/{dm,ds,sv}.rs` -- the
            // StepDescriptor::gate_checks fields are populated
            // exclusively from the Rust source step registry, not
            // from any project-controlled file. Do NOT change that
            // without adding a cmd allowlist: shells out via
            // Command::new + args (not `sh -c`), so the
            // command-injection surface is the `cmd` string
            // itself, and an LLM-authored milestone or
            // `state.toml` value as `cmd` would let the agent run
            // arbitrary binaries. See orchestrator audit #19
            // (2026-05-16).
            // Record per-gate-shell wall-clock timing alongside the
            // existing pass/fail outcome. The timing row lands in the
            // project-local `tool-timings.jsonl` and (best-effort) the
            // global DB with `caller_kind = "gate"` so reports can
            // separate gate-evaluation cost from agent-tool cost.
            use crate::session::tool_timings::{
                CallerKind, ToolTimingRecord, ToolTimingsLog, now_unix_seconds,
            };
            let started_unix = now_unix_seconds();
            let started = Instant::now();
            let output = Command::new(cmd)
                .args(args)
                .current_dir(project_dir)
                .output();
            let wall_ms = started.elapsed().as_millis() as u64;
            let (status_str, exit_code) = match &output {
                Ok(out) if out.status.success() => ("ok", out.status.code()),
                Ok(out) => ("error", out.status.code()),
                Err(_) => ("error", None),
            };
            let timings = ToolTimingsLog::for_project(project_dir);
            timings.record(&ToolTimingRecord {
                started_unix,
                // Gate evaluation runs outside the per-step session
                // loop; the step id isn't in scope at this depth. Step
                // attribution lives on the orchestrator's gate-emit
                // events; here we leave it None and let the consumer
                // group by command name or timestamp.
                step: None,
                caller_kind: CallerKind::Gate,
                tool_name: cmd.to_string(),
                args_summary: args.join(" "),
                status: status_str.to_string(),
                wall_ms,
                exit_code,
                request_id: None,
                turn_index: None,
            });
            match output {
                Ok(out) if out.status.success() => Ok(None),
                Ok(out) => Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!(
                        "{} {} failed: exit {:?}: {}",
                        cmd,
                        args.join(" "),
                        out.status.code(),
                        String::from_utf8_lossy(&out.stderr).trim(),
                    ),
                })),
                Err(err) => Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("failed to spawn {cmd}: {err}"),
                })),
            }
        }
        GateCheck::ExperimentsRecorded { description } => {
            let db = project_dir.join(".sim-flow").join("experiments.db");
            if !db.exists() {
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("experiments.db missing: {}", db.display()),
                }));
            }
            match crate::tracking::index::ExperimentIndex::open_path(&db) {
                Ok(index) => match index.count_runs() {
                    Ok(0) => Ok(Some(GateFailure {
                        description: description.clone(),
                        reason: "experiments.db has no recorded runs".to_string(),
                    })),
                    Ok(_) => Ok(None),
                    Err(err) => Ok(Some(GateFailure {
                        description: description.clone(),
                        reason: format!("experiments.db query failed: {err}"),
                    })),
                },
                Err(err) => Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("cannot open experiments.db: {err}"),
                })),
            }
        }
        GateCheck::MilestonesAllResolved {
            dir,
            file_prefixes,
            placeholder_marker,
            description,
            forbid_deferred,
        } => {
            let full_dir = project_dir.join(dir);
            let entries = match std::fs::read_dir(&full_dir) {
                Ok(e) => e,
                Err(err) => {
                    return Ok(Some(GateFailure {
                        description: description.clone(),
                        reason: format!("milestone dir missing: {}: {err}", full_dir.display()),
                    }));
                }
            };
            let mut milestones: Vec<(String, PathBuf)> = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !name.ends_with(".md") {
                    continue;
                }
                let Some(prefix) = file_prefixes.iter().find(|p| name.starts_with(p.as_str()))
                else {
                    continue;
                };
                let rest = &name[prefix.len()..];
                if !rest
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false)
                {
                    continue;
                }
                milestones.push((name.to_string(), path));
            }
            if milestones.is_empty() {
                let prefixes_display = file_prefixes
                    .iter()
                    .map(|p| format!("`{p}NN-*.md`"))
                    .collect::<Vec<_>>()
                    .join(" or ");
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!(
                        "no {} files found under `{}`",
                        prefixes_display,
                        full_dir.display()
                    ),
                }));
            }
            let mut pending: Vec<String> = Vec::new();
            for (name, path) in &milestones {
                let body = match std::fs::read_to_string(path) {
                    Ok(b) => b,
                    Err(err) => {
                        return Ok(Some(GateFailure {
                            description: description.clone(),
                            reason: format!("read {}: {err}", path.display()),
                        }));
                    }
                };
                match placeholder_marker {
                    Some(marker) => {
                        if body.contains(marker) {
                            pending.push(format!(
                                "  - `{name}`: still contains placeholder marker (stub not yet detailed)"
                            ));
                        }
                    }
                    None => {
                        let pending_count = body
                            .lines()
                            .filter(|line| line.trim_start().starts_with("- [ ]"))
                            .count();
                        let deferred_count = if *forbid_deferred {
                            body.lines()
                                .filter(|line| line.trim_start().starts_with("- [-]"))
                                .count()
                        } else {
                            0
                        };
                        let unresolved = pending_count + deferred_count;
                        if unresolved > 0 {
                            let detail = if *forbid_deferred && deferred_count > 0 {
                                format!(
                                    "{unresolved} unresolved row(s) ({pending_count} pending, {deferred_count} deferred -- this step forbids deferrals)"
                                )
                            } else {
                                format!("{unresolved} unresolved row(s)")
                            };
                            pending.push(format!("  - `{name}`: {detail}"));
                        }
                    }
                }
            }
            if pending.is_empty() {
                Ok(None)
            } else {
                let label = if placeholder_marker.is_some() {
                    "milestone stubs not yet detailed:"
                } else {
                    "milestone files still have unresolved rows:"
                };
                Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("{label}\n{}", pending.join("\n")),
                }))
            }
        }
        GateCheck::SpecMdStructured {
            spec_md_path,
            manifest_path,
            description,
        } => {
            let abs_spec = project_dir.join(spec_md_path);
            let abs_manifest = manifest_path.as_ref().map(|p| project_dir.join(p));
            let outcome = crate::__internal::session::dm0::gate::check_dm0_gate(
                &abs_spec,
                abs_manifest.as_deref(),
                Some(project_dir),
            )?;
            if outcome.is_clean() {
                return Ok(None);
            }
            let summary = outcome
                .failures
                .iter()
                .map(|f| format!("  - [{}] {}", f.code, f.message))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(Some(GateFailure {
                description: description.clone(),
                reason: format!("spec.md structural gate failed:\n{summary}"),
            }))
        }
        GateCheck::CritiqueClean { path, description } => {
            let full = project_dir.join(path);
            if !full.exists() {
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("critique missing: {}", full.display()),
                }));
            }
            let critique = Critique::load(&full)?;
            if critique.has_blocking() {
                let summary = critique
                    .blocking()
                    .into_iter()
                    .map(|f| format!("  - {}: {}", marker(f), f.text()))
                    .collect::<Vec<_>>()
                    .join("\n");
                return Ok(Some(GateFailure {
                    description: description.clone(),
                    reason: format!("critique has blocking findings:\n{summary}"),
                }));
            }
            Ok(None)
        }
    }
}

pub(super) fn marker(finding: &crate::critique::Finding) -> &'static str {
    match finding {
        crate::critique::Finding::Resolved(_) => "RESOLVED",
        crate::critique::Finding::Unresolved(_) => "UNRESOLVED",
        crate::critique::Finding::Blocker(_) => "BLOCKER",
    }
}

/// Expand a list of candidate paths (used by `AnyExists` /
/// `AnyMatches`) into the concrete files to inspect. File entries
/// are kept as-is; directory entries are walked one level deep and
/// every `*.md` inside is included EXCEPT scaffolding markers
/// (`.gitkeep`) and the auto-generated index files
/// (`README.md`, `_toc.md`) -- those don't carry the actual
/// content the gate cares about. Missing paths are silently
/// dropped; the caller surfaces a "no candidate files" failure
/// when the resulting list is empty.
pub(super) fn expand_candidate_files(project_dir: &Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for rel in paths {
        let abs = project_dir.join(rel);
        let Ok(meta) = std::fs::metadata(&abs) else {
            continue;
        };
        if meta.is_file() {
            out.push(abs);
            continue;
        }
        if !meta.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&abs) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.ends_with(".md") {
                continue;
            }
            // Skip scaffolding + auto-generated index files. The
            // index summarizes section content; the actual
            // numbers / patterns the gate looks for live in the
            // section files themselves. Case-insensitive so
            // `Readme.md` / `INDEX.md` / Windows-style casings
            // are also excluded. See orchestrator audit #20
            // (2026-05-16).
            let name_lower = name.to_ascii_lowercase();
            if matches!(
                name_lower.as_str(),
                ".gitkeep" | "readme.md" | "_toc.md" | "index.md"
            ) {
                continue;
            }
            if let Ok(file_meta) = path.metadata()
                && file_meta.is_file()
            {
                out.push(path);
            }
        }
    }
    out
}
