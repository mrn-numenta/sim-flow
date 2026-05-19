//! LLM critique pass (Phase 9 milestone 9.5).
//!
//! Takes a deterministic first-cut [`FormatJson`] descriptor and the
//! structural [`Skeleton`] it was derived from, sends both to an
//! [`LlmAdapter`], parses the model's reply as an **adjustments
//! patch** (an array of per-entry overrides), applies the patch on
//! top of the first cut, and returns the refined descriptor.
//!
//! Architecture Chapter 7 §7.4 + §7.6 specify the two-pass design:
//! the first cut is what the LLM corrects, not what the LLM
//! produces from scratch. The model's task is **critique**, not
//! classification — for each first-cut entry, either accept it (no
//! patch entry) or override the tag with a rationale. Cases the
//! first cut nailed pass through with the LLM's confirmation; cases
//! it got wrong (or marked `unknown`) get LLM-revised tags.
//!
//! The `--no-format-discovery` CLI path skips this module entirely
//! and ships the first-cut descriptor as-is.

use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;

use super::descriptor::{
    ChromeKind, ColumnMapping, FigureKind, FigureTarget, FormatJson, GlossarySource, Layer,
    SpecMdRole, TableKind, TableTarget, WrapStrategy,
};
use super::skeleton::Skeleton;
use crate::Result;
use crate::session::llm_adapter::LlmAdapter;
use crate::session::protocol::{LlmMessage, LlmRole};
use crate::session::spec_ingest::pipeline::IngestWarning;

/// Stable identifier the descriptor carries in its `prompt_version`
/// field after a successful critique pass. Bump alongside any
/// material change to [`CRITIQUE_PROMPT`] so the
/// `(source_sha256, model, prompt_version)` cache key invalidates.
pub const CRITIQUE_PROMPT_VERSION: &str = "critique-v1";

/// Stage tag used in [`IngestWarning::stage`] for warnings this
/// module surfaces. The format-discovery LLM pass is the third
/// stage in the §7.4 decision policy (pdf_oxide → first-cut →
/// LLM critique → user). See `IngestWarning::stage` for the
/// 1-indexed numbering used by adjacent stages.
const DISCOVER_STAGE: u8 = 3;

/// Inline system prompt (v1). Future phases can promote this to
/// `prompts/format-discovery.md` per Chapter 6 loader conventions;
/// out of scope here. The prompt has three parts: a system
/// framing paragraph, a schema description for the adjustments
/// patch, and per-call inputs (the first-cut descriptor JSON and
/// the rendered skeleton) appended at call time.
const CRITIQUE_PROMPT: &str = r#"You are reviewing a deterministic first-cut classification of a hardware-spec PDF's structure. Your job is to confirm or correct the first cut. You do NOT re-classify from scratch. You emit an adjustments patch — a JSON array of per-entry overrides — that the caller applies on top of the first cut to produce the final descriptor.

# Adjustments-patch schema

Emit a JSON array of adjustment objects. Each adjustment has this shape:

  {
    "target": { "kind": "<target-kind>", "id": "<entry-id>" },
    "field": "<dotted.path.to.field>",
    "old_value": <current value, json>,
    "new_value": <new value, json>,
    "rationale": "<short reason>"
  }

`target.kind` is one of: `section_role`, `table`, `figure`, `glossary`, `chrome`.

`target.id` identifies which entry to patch:
  - For `section_role`: `"<page>:<line>"` (matches the entry's
    `page` and `line` fields in the first-cut descriptor).
  - For `table`: the entry's `id` field (e.g. `"T01"`).
  - For `figure`: the entry's `id` field (e.g. `"F01"`).
  - For `glossary`: the entry's `acronym` field (e.g. `"IF"`).
  - For `chrome`: the entry's `regex` field.

`field` is the dotted path of the field to update. Allowed fields per target kind:
  - `section_role`: `spec_md_role`, `layer`, `rationale`.
  - `table`: `kind`, `spec_md_target`, `column_map`, `wrap_strategy`, `rationale`.
  - `figure`: `kind`, `spec_md_target`, `referenced_acronyms`, `rationale`.
  - `glossary`: `expansion`, `scope`, `used_in_blocks`, `source`.
  - `chrome`: `regex`, `kind`, `y_band_pt`.

You MAY NOT change pdf_oxide-derived facts: table `page` / `first_line` / `row_count` / `col_count` / `bbox`, figure `page`, section_role `page` / `line` / `font_size` / `font_weight`, glossary `acronym` / `first_page`. Adjustments touching those fields will be rejected.

`old_value` must match the current field value verbatim (it's a sanity check; mismatches are skipped with a drift warning).

Wrap the JSON array in `<patch>` and `</patch>` tags so the parser can locate it unambiguously. If you have no corrections, emit `<patch>[]</patch>`. Do not emit anything outside the patch tags except optional brief commentary.

# Inputs

The first-cut descriptor (JSON):

"#;

/// Run the LLM critique pass over the first cut.
///
/// Builds a prompt around the first-cut descriptor + the rendered
/// skeleton, dispatches it through `llm`, parses any adjustments
/// patch the model emits, applies the patch, and returns the
/// refined descriptor. Warnings (no patch parsed, retry attempts,
/// rejected immutable fields, `old_value` drift, etc.) accumulate
/// into `warnings` rather than aborting the pipeline — a no-op
/// critique is a legitimate outcome (it means the LLM has no
/// corrections), and a parse failure falls back to the first cut
/// unchanged.
///
/// On success the returned descriptor's `model`, `prompt_version`,
/// and `discovered_at` reflect the LLM call that produced it; all
/// other fields are the first cut with adjustments applied.
pub fn discover(
    skeleton: &Skeleton,
    first_cut: &FormatJson,
    llm: &mut dyn LlmAdapter,
    warnings: &mut Vec<IngestWarning>,
) -> Result<FormatJson> {
    let descriptor_json = serde_json::to_string_pretty(first_cut)
        .map_err(|e| crate::Error::State(format!("format discover: serialise first cut: {e}")))?;
    let skeleton_text = skeleton.render();

    let initial_prompt = build_prompt(&descriptor_json, &skeleton_text);
    let (raw_first, _metrics) = llm.dispatch(&[LlmMessage {
        role: LlmRole::User,
        content: initial_prompt.clone(),
        ..LlmMessage::default()
    }])?;

    let patch_text = match extract_patch_text(&raw_first) {
        Some(text) => Some(text),
        None => {
            warnings.push(IngestWarning::new(
                "discover_retry",
                format!(
                    "first response did not contain a parseable <patch> block; \
                     raw response: {}",
                    truncate_for_message(&raw_first),
                ),
                DISCOVER_STAGE,
            ));
            let retry_prompt = build_retry_prompt(&initial_prompt, &raw_first);
            let (raw_second, _metrics) = llm.dispatch(&[LlmMessage {
                role: LlmRole::User,
                content: retry_prompt,
                ..LlmMessage::default()
            }])?;
            extract_patch_text(&raw_second)
        }
    };

    let patch_text = match patch_text {
        Some(t) => t,
        None => {
            warnings.push(IngestWarning::new(
                "discover_failed",
                "retry response still did not contain a parseable <patch> block; \
                 falling back to first cut unchanged"
                    .to_string(),
                DISCOVER_STAGE,
            ));
            return Ok(stamp_metadata(first_cut, llm.name()));
        }
    };

    let adjustments: Vec<RawAdjustment> = match serde_json::from_str(&patch_text) {
        Ok(list) => list,
        Err(e) => {
            warnings.push(IngestWarning::new(
                "discover_no_patch_parsed",
                format!("patch block did not decode as JSON array of adjustments: {e}",),
                DISCOVER_STAGE,
            ));
            return Ok(stamp_metadata(first_cut, llm.name()));
        }
    };

    let mut refined = first_cut.clone();
    for adj in adjustments {
        apply_adjustment(&mut refined, &adj, warnings);
    }

    Ok(stamp_metadata(&refined, llm.name()))
}

/// Stamp `model` / `prompt_version` / `discovered_at` on a
/// (possibly already-adjusted) descriptor. `model` overrides the
/// first-cut sentinel with the LLM adapter's identifier.
fn stamp_metadata(descriptor: &FormatJson, model: &str) -> FormatJson {
    let mut out = descriptor.clone();
    out.model = model.to_string();
    out.prompt_version = CRITIQUE_PROMPT_VERSION.to_string();
    out.discovered_at = Utc::now();
    out
}

/// Assemble the first user-prompt: system framing + schema
/// description + the first-cut descriptor JSON + the rendered
/// skeleton. The trailing two sections are appended programmatically
/// so we don't have to embed multi-MB skeletons in a const string.
fn build_prompt(descriptor_json: &str, skeleton_text: &str) -> String {
    let mut buf = String::with_capacity(
        CRITIQUE_PROMPT.len() + descriptor_json.len() + skeleton_text.len() + 256,
    );
    buf.push_str(CRITIQUE_PROMPT);
    buf.push_str(descriptor_json);
    buf.push_str("\n\nThe structural skeleton (rendered text):\n\n");
    buf.push_str(skeleton_text);
    buf.push_str("\n\nEmit the adjustments patch now, wrapped in <patch>...</patch> tags.\n");
    buf
}

/// Assemble the retry prompt: the original prompt + the model's
/// first response + an explicit reminder about the `<patch>` tags.
fn build_retry_prompt(original_prompt: &str, first_response: &str) -> String {
    let mut buf = String::with_capacity(original_prompt.len() + first_response.len() + 512);
    buf.push_str(original_prompt);
    buf.push_str("\n\nYour previous response was:\n\n");
    buf.push_str(first_response);
    buf.push_str(
        "\n\nYour previous response did not contain a parseable <patch>...</patch> \
         block. Please emit only the JSON adjustments array between <patch> and \
         </patch> tags. If you have no corrections, emit <patch>[]</patch>.\n",
    );
    buf
}

/// Extract the JSON-array text from the model's reply. Tries, in
/// order: `<patch>...</patch>` tags, then a ```json fenced block,
/// then a bare `[...]` balanced JSON array. Returns `None` if none
/// of those patterns appears in the response.
fn extract_patch_text(response: &str) -> Option<String> {
    if let (Some(start), Some(end)) = (response.find("<patch>"), response.find("</patch>")) {
        let body_start = start + "<patch>".len();
        if end > body_start {
            return Some(response[body_start..end].trim().to_string());
        }
    }
    if let Some(fenced) = extract_fenced_json(response) {
        return Some(fenced);
    }
    extract_balanced_array(response)
}

/// Look for a ```json ... ``` fenced block (case-insensitive on
/// `json`) and return the body. Falls back to a bare ``` fence if
/// the language tag is missing.
fn extract_fenced_json(response: &str) -> Option<String> {
    let needles: &[&str] = &["```json\n", "```JSON\n", "```Json\n", "```\n"];
    for needle in needles {
        if let Some(start) = response.find(needle) {
            let body_start = start + needle.len();
            if let Some(rel_end) = response[body_start..].find("```") {
                let body = &response[body_start..body_start + rel_end];
                let trimmed = body.trim();
                if trimmed.starts_with('[') {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

/// Walk `response` for the first `[` and return the substring
/// through its matching balanced `]`. Respects string literals so
/// brackets inside quoted strings don't fool the matcher.
fn extract_balanced_array(response: &str) -> Option<String> {
    let bytes = response.as_bytes();
    let mut start: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'[' {
            start = Some(i);
            break;
        }
    }
    let start = start?;

    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut escape = false;
    let mut end: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    end.map(|e| response[start..=e].to_string())
}

/// Truncate raw model output for inclusion in a warning message.
/// Long bodies blow up the manifest's warnings table; 240 bytes is
/// enough to make the failure shape recognisable in logs.
fn truncate_for_message(raw: &str) -> String {
    const LIMIT: usize = 240;
    if raw.len() <= LIMIT {
        raw.to_string()
    } else {
        let mut cut = LIMIT;
        while !raw.is_char_boundary(cut) && cut > 0 {
            cut -= 1;
        }
        format!("{}…", &raw[..cut])
    }
}

/// Wire shape of one adjustment entry the LLM emits. The targets
/// and fields are validated by [`apply_adjustment`]; `old_value` /
/// `new_value` stay as opaque `serde_json::Value`s until then.
#[derive(Debug, Clone, Deserialize)]
struct RawAdjustment {
    target: RawTarget,
    field: String,
    #[serde(default)]
    old_value: Value,
    new_value: Value,
    #[serde(default)]
    rationale: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RawTarget {
    kind: String,
    id: String,
}

/// Apply one adjustment. Looks the target entry up by id, checks
/// the field name against the allow-list for that target kind,
/// verifies `old_value` matches the current field value, and then
/// installs `new_value`. Failures emit a warning + skip; one bad
/// adjustment never aborts the rest of the patch.
fn apply_adjustment(
    descriptor: &mut FormatJson,
    adj: &RawAdjustment,
    warnings: &mut Vec<IngestWarning>,
) {
    match adj.target.kind.as_str() {
        "section_role" => apply_section_role(descriptor, adj, warnings),
        "table" => apply_table(descriptor, adj, warnings),
        "figure" => apply_figure(descriptor, adj, warnings),
        "glossary" => apply_glossary(descriptor, adj, warnings),
        "chrome" => apply_chrome(descriptor, adj, warnings),
        other => {
            warnings.push(IngestWarning::new(
                "discover_unknown_target",
                format!("adjustment target kind '{other}' is not recognised"),
                DISCOVER_STAGE,
            ));
        }
    }
}

fn apply_section_role(
    descriptor: &mut FormatJson,
    adj: &RawAdjustment,
    warnings: &mut Vec<IngestWarning>,
) {
    let (page, line) = match parse_page_line(&adj.target.id) {
        Some(pl) => pl,
        None => {
            warnings.push(IngestWarning::new(
                "discover_unknown_target",
                format!(
                    "section_role target id '{}' is not '<page>:<line>'",
                    adj.target.id
                ),
                DISCOVER_STAGE,
            ));
            return;
        }
    };
    let Some(entry) = descriptor
        .section_roles
        .iter_mut()
        .find(|s| s.page == page && s.line == line)
    else {
        warnings.push(IngestWarning::new(
            "discover_unknown_target",
            format!("no section_role found at {page}:{line}"),
            DISCOVER_STAGE,
        ));
        return;
    };
    match adj.field.as_str() {
        "spec_md_role" => {
            let current = match serde_json::to_value(&entry.spec_md_role) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "spec_md_role differs from old_value");
            }
            let parsed: SpecMdRole = match serde_json::from_value(adj.new_value.clone()) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            entry.spec_md_role = parsed;
            note_rationale(&mut entry.rationale, &adj.rationale);
        }
        "layer" => {
            let current = match serde_json::to_value(entry.layer) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "layer differs from old_value");
            }
            let parsed: Layer = match serde_json::from_value(adj.new_value.clone()) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            entry.layer = parsed;
            note_rationale(&mut entry.rationale, &adj.rationale);
        }
        "rationale" => {
            let current = Value::String(entry.rationale.clone());
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "rationale differs from old_value");
            }
            entry.rationale = string_or_drift(&adj.new_value, warnings, &adj.field)
                .unwrap_or(entry.rationale.clone());
        }
        other => warn_immutable(warnings, "section_role", other),
    }
}

fn apply_table(
    descriptor: &mut FormatJson,
    adj: &RawAdjustment,
    warnings: &mut Vec<IngestWarning>,
) {
    let id = adj.target.id.clone();
    let Some(entry) = descriptor.tables.iter_mut().find(|t| t.id == id) else {
        warnings.push(IngestWarning::new(
            "discover_unknown_target",
            format!("no table found with id '{id}'"),
            DISCOVER_STAGE,
        ));
        return;
    };
    match adj.field.as_str() {
        "kind" => {
            let current = match serde_json::to_value(entry.kind) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "kind differs from old_value");
            }
            let parsed: TableKind = match serde_json::from_value(adj.new_value.clone()) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            entry.kind = parsed;
            note_rationale(&mut entry.rationale, &adj.rationale);
        }
        "spec_md_target" => {
            let current = match serde_json::to_value(&entry.spec_md_target) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(
                    warnings,
                    &adj.field,
                    "spec_md_target differs from old_value",
                );
            }
            let parsed: TableTarget = match serde_json::from_value(adj.new_value.clone()) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            entry.spec_md_target = parsed;
            note_rationale(&mut entry.rationale, &adj.rationale);
        }
        "column_map" => {
            let current = match serde_json::to_value(&entry.column_map) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "column_map differs from old_value");
            }
            let parsed: Vec<ColumnMapping> = match serde_json::from_value(adj.new_value.clone()) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            entry.column_map = parsed;
            note_rationale(&mut entry.rationale, &adj.rationale);
        }
        "wrap_strategy" => {
            let current = match serde_json::to_value(entry.wrap_strategy) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "wrap_strategy differs from old_value");
            }
            let parsed: WrapStrategy = match serde_json::from_value(adj.new_value.clone()) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            entry.wrap_strategy = parsed;
            note_rationale(&mut entry.rationale, &adj.rationale);
        }
        "rationale" => {
            let current = Value::String(entry.rationale.clone());
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "rationale differs from old_value");
            }
            entry.rationale = string_or_drift(&adj.new_value, warnings, &adj.field)
                .unwrap_or(entry.rationale.clone());
        }
        other => warn_immutable(warnings, "table", other),
    }
}

fn apply_figure(
    descriptor: &mut FormatJson,
    adj: &RawAdjustment,
    warnings: &mut Vec<IngestWarning>,
) {
    let id = adj.target.id.clone();
    let Some(entry) = descriptor.figures.iter_mut().find(|f| f.id == id) else {
        warnings.push(IngestWarning::new(
            "discover_unknown_target",
            format!("no figure found with id '{id}'"),
            DISCOVER_STAGE,
        ));
        return;
    };
    match adj.field.as_str() {
        "kind" => {
            let current = match serde_json::to_value(entry.kind) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "kind differs from old_value");
            }
            let parsed: FigureKind = match serde_json::from_value(adj.new_value.clone()) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            entry.kind = parsed;
            note_rationale(&mut entry.rationale, &adj.rationale);
        }
        "spec_md_target" => {
            let current = match serde_json::to_value(&entry.spec_md_target) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(
                    warnings,
                    &adj.field,
                    "spec_md_target differs from old_value",
                );
            }
            let parsed: FigureTarget = match serde_json::from_value(adj.new_value.clone()) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            entry.spec_md_target = parsed;
            note_rationale(&mut entry.rationale, &adj.rationale);
        }
        "referenced_acronyms" => {
            let current = match serde_json::to_value(&entry.referenced_acronyms) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(
                    warnings,
                    &adj.field,
                    "referenced_acronyms differs from old_value",
                );
            }
            let parsed: Vec<String> = match serde_json::from_value(adj.new_value.clone()) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            entry.referenced_acronyms = parsed;
            note_rationale(&mut entry.rationale, &adj.rationale);
        }
        "rationale" => {
            let current = Value::String(entry.rationale.clone());
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "rationale differs from old_value");
            }
            entry.rationale = string_or_drift(&adj.new_value, warnings, &adj.field)
                .unwrap_or(entry.rationale.clone());
        }
        other => warn_immutable(warnings, "figure", other),
    }
}

fn apply_glossary(
    descriptor: &mut FormatJson,
    adj: &RawAdjustment,
    warnings: &mut Vec<IngestWarning>,
) {
    let acronym = adj.target.id.clone();
    let Some(entry) = descriptor
        .glossary
        .iter_mut()
        .find(|g| g.acronym == acronym)
    else {
        warnings.push(IngestWarning::new(
            "discover_unknown_target",
            format!("no glossary entry found for acronym '{acronym}'"),
            DISCOVER_STAGE,
        ));
        return;
    };
    match adj.field.as_str() {
        "expansion" => {
            let current = Value::String(entry.expansion.clone());
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "expansion differs from old_value");
            }
            if let Some(s) = string_or_drift(&adj.new_value, warnings, &adj.field) {
                entry.expansion = s;
            }
        }
        "scope" => {
            let current = Value::String(entry.scope.clone());
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "scope differs from old_value");
            }
            if let Some(s) = string_or_drift(&adj.new_value, warnings, &adj.field) {
                entry.scope = s;
            }
        }
        "used_in_blocks" => {
            let current = match serde_json::to_value(&entry.used_in_blocks) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(
                    warnings,
                    &adj.field,
                    "used_in_blocks differs from old_value",
                );
            }
            let parsed: Vec<String> = match serde_json::from_value(adj.new_value.clone()) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            entry.used_in_blocks = parsed;
        }
        "source" => {
            let current = match serde_json::to_value(entry.source) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "source differs from old_value");
            }
            let parsed: GlossarySource = match serde_json::from_value(adj.new_value.clone()) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            entry.source = parsed;
        }
        other => warn_immutable(warnings, "glossary", other),
    }
}

fn apply_chrome(
    descriptor: &mut FormatJson,
    adj: &RawAdjustment,
    warnings: &mut Vec<IngestWarning>,
) {
    let regex = adj.target.id.clone();
    let Some(entry) = descriptor.chrome.iter_mut().find(|c| c.regex == regex) else {
        warnings.push(IngestWarning::new(
            "discover_unknown_target",
            format!("no chrome entry found for regex '{regex}'"),
            DISCOVER_STAGE,
        ));
        return;
    };
    match adj.field.as_str() {
        "regex" => {
            let current = Value::String(entry.regex.clone());
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "regex differs from old_value");
            }
            if let Some(s) = string_or_drift(&adj.new_value, warnings, &adj.field) {
                entry.regex = s;
            }
        }
        "kind" => {
            let current = match serde_json::to_value(entry.kind) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "kind differs from old_value");
            }
            let parsed: ChromeKind = match serde_json::from_value(adj.new_value.clone()) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            entry.kind = parsed;
        }
        "y_band_pt" => {
            let current = match serde_json::to_value(entry.y_band_pt) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            if !old_value_matches(&adj.old_value, &current) {
                return warn_drift(warnings, &adj.field, "y_band_pt differs from old_value");
            }
            let parsed: Option<[f32; 2]> = match serde_json::from_value(adj.new_value.clone()) {
                Ok(v) => v,
                Err(e) => return warn_drift(warnings, &adj.field, &e.to_string()),
            };
            entry.y_band_pt = parsed;
        }
        other => warn_immutable(warnings, "chrome", other),
    }
}

/// Two `Value`s match if they're equal. `Null` from the LLM means
/// "I don't know the current value; trust me" — treat it as a
/// pass so the LLM can omit old_value for fields whose current
/// shape is awkward to reproduce (e.g. nested struct values).
fn old_value_matches(old: &Value, current: &Value) -> bool {
    matches!(old, Value::Null) || old == current
}

/// Decode a string-typed `new_value`. If the value isn't a string,
/// emit a drift warning + return `None` so the caller leaves the
/// field unchanged.
fn string_or_drift(
    new_value: &Value,
    warnings: &mut Vec<IngestWarning>,
    field: &str,
) -> Option<String> {
    match new_value {
        Value::String(s) => Some(s.clone()),
        other => {
            warnings.push(IngestWarning::new(
                "discover_old_value_drift",
                format!("field '{field}': expected string new_value, got {other}"),
                DISCOVER_STAGE,
            ));
            None
        }
    }
}

/// Emit a `discover_old_value_drift` warning and skip. Used by the
/// per-field application paths when either the `old_value` doesn't
/// match the descriptor's current state or `new_value` doesn't
/// parse into the field's typed shape.
fn warn_drift(warnings: &mut Vec<IngestWarning>, field: &str, detail: &str) {
    warnings.push(IngestWarning::new(
        "discover_old_value_drift",
        format!("field '{field}': {detail}; skipping adjustment"),
        DISCOVER_STAGE,
    ));
}

/// Emit a `discover_immutable_field` warning. Used when the LLM
/// tries to change a pdf_oxide-derived fact (page, line, row_count,
/// etc.) or an unrecognised field name for the target kind.
fn warn_immutable(warnings: &mut Vec<IngestWarning>, target_kind: &str, field: &str) {
    warnings.push(IngestWarning::new(
        "discover_immutable_field",
        format!("field '{field}' is not adjustable on target '{target_kind}'; skipping"),
        DISCOVER_STAGE,
    ));
}

/// Append `extra` onto the entry's existing rationale (separated
/// by `" | "`). Skips empty inputs so we don't accumulate trailing
/// separators when the LLM omits the field.
fn note_rationale(rationale: &mut String, extra: &str) {
    let extra = extra.trim();
    if extra.is_empty() {
        return;
    }
    if rationale.is_empty() {
        *rationale = format!("LLM: {extra}");
    } else {
        rationale.push_str(" | LLM: ");
        rationale.push_str(extra);
    }
}

/// Parse a `"<page>:<line>"` target id into the two `u32`s. Returns
/// `None` on malformed ids so the caller can surface a single
/// `discover_unknown_target` warning.
fn parse_page_line(id: &str) -> Option<(u32, u32)> {
    let (p, l) = id.split_once(':')?;
    let page = p.trim().parse::<u32>().ok()?;
    let line = l.trim().parse::<u32>().ok()?;
    Some((page, line))
}
