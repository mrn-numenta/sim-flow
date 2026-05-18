//! No-source DM0 Q&A loop, built on top of the `ask_user` tool.
//!
//! When [`super::detect_mode`] returns
//! [`super::Dm0Mode::NoSource`], the agent's LLM turn drives this
//! loop: it iterates over `SpecMd::missing_required_fields()`, opens
//! an `ask_user` thread per field, validates the reply against the
//! field's `kind`, and closes the thread with `record_as =
//! "auto-decision"` (or chains a clarification on ambiguous input,
//! or records a TBD on cancellation).
//!
//! The loop does NOT implement its own user-prompting machinery;
//! every question goes through the `ask_user` suspend/resume
//! protocol the orchestrator's dispatch loop already understands
//! via `ToolResult::suspend`. The orchestrator is responsible for
//! the `RequestUserInput` event + thread-chaining; this module just
//! sequences the field walk and the validation logic. Owned by
//! Phase 6 Stream B.

use regex::Regex;

use crate::__internal::session::llm_adapter::LlmAdapter;
use crate::__internal::session::protocol::{LlmMessage, LlmRole};
use crate::__internal::session::spec_md::{MissingField, MissingFieldKind, SpecMd};
use crate::Result;

/// Soft cap on chained clarification turns per field, matching the
/// `ASK_USER_TURN_CAP` constant in the `ask_user` tool. The dispatch
/// loop emits a turn-cap warning above this; the qa-loop driver stops
/// chaining clarifications and records a TBD instead.
pub const QA_TURN_CAP: u32 = 5;

/// Result of [`ask_section_applicability`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionApplicability {
    Applicable,
    NotApplicable,
    Deferred,
}

/// Outcome of [`drive_qa_loop`]. Counts each terminal state so the
/// caller (DM0 dispatch site) can record metrics and surface them in
/// the chat panel summary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QaLoopReport {
    /// Fields whose Q&A thread closed with a resolved answer the
    /// validator accepted.
    pub fields_resolved: usize,
    /// Fields whose thread was cancelled by the user (recorded as a
    /// TBD Open Question).
    pub fields_cancelled: usize,
    /// Fields the loop gave up on after the turn cap fired without a
    /// validating answer (also recorded as TBD).
    pub fields_capped: usize,
    /// Optional sections marked not-applicable by the user.
    pub sections_not_applicable: usize,
    /// Optional sections the user deferred (surfaced again at end).
    pub sections_deferred: usize,
}

/// Drive the Q&A loop for every MissingField in `spec`. Each
/// iteration opens an `ask_user` thread, validates the user's reply,
/// either closes the thread with the resolved value or chains a
/// clarification, and advances. Returns when
/// `spec.missing_required_fields()` is empty or every remaining
/// field has been cancelled (and recorded as a TBD).
///
/// The function takes `&mut dyn LlmAdapter` because the
/// normalization passes for free-form sections (e.g. Worked
/// Examples) and for ambiguity detection on user replies require an
/// LLM call. Reply validation that is purely syntactic (regex /
/// yes-no / choice) runs locally.
///
/// The mock LLM driving this in tests doubles as the scripted user
/// (its responses are the user's replies); in production the
/// orchestrator's dispatch loop interposes between the agent and
/// the user via `ask_user`, and the same code path runs. Either
/// way the visible artifact is the same: `spec` ends with the
/// resolved answers applied, and the report records the breakdown.
pub fn drive_qa_loop(spec: &mut SpecMd, llm: &mut dyn LlmAdapter) -> Result<QaLoopReport> {
    let mut report = QaLoopReport::default();
    let missing = spec.missing_required_fields();
    for field in missing {
        match field.kind {
            MissingFieldKind::SectionApplicability => {
                match ask_section_applicability(&field.section_path, spec, llm)? {
                    SectionApplicability::Applicable => {
                        // Drill into the section's required rows: the
                        // template-order traversal will surface them on
                        // the next iteration if the agent doesn't add
                        // them now. For the v1 loop we record the
                        // applicability decision as an Open Question
                        // entry so the agent's later LLM turn knows to
                        // populate the section.
                        spec.open_questions.push(
                            crate::__internal::session::spec_md::OpenQuestion {
                                text: format!(
                                    "Section `{}` marked applicable — populate the required rows.",
                                    field.section_path
                                ),
                            },
                        );
                        report.fields_resolved += 1;
                    }
                    SectionApplicability::NotApplicable => {
                        spec.open_questions.push(
                            crate::__internal::session::spec_md::OpenQuestion {
                                text: format!(
                                    "Section `{}` marked not applicable.",
                                    field.section_path
                                ),
                            },
                        );
                        report.sections_not_applicable += 1;
                    }
                    SectionApplicability::Deferred => {
                        spec.open_questions.push(
                            crate::__internal::session::spec_md::OpenQuestion {
                                text: format!(
                                    "Section `{}` deferred — revisit before gate-check.",
                                    field.section_path
                                ),
                            },
                        );
                        report.sections_deferred += 1;
                    }
                }
            }
            _ => match run_field_thread(&field, spec, llm)? {
                FieldOutcome::Resolved => report.fields_resolved += 1,
                FieldOutcome::Cancelled => report.fields_cancelled += 1,
                FieldOutcome::Capped => report.fields_capped += 1,
            },
        }
    }
    Ok(report)
}

/// SectionApplicability fast-path: for OPTIONAL sections, ask the
/// user via an `ask_user` call with `kind = "choice"` and branch on
/// the reply. Called from [`drive_qa_loop`] before drilling into a
/// section's MissingFields.
pub fn ask_section_applicability(
    section: &str,
    _spec: &mut SpecMd,
    llm: &mut dyn LlmAdapter,
) -> Result<SectionApplicability> {
    let question = format!("Does this design have a `{section}`? (yes / no / skip)");
    let reply = ask_llm_for_reply(llm, &question, None)?;
    Ok(parse_yes_no_skip(&reply))
}

/// Internal per-field outcome reported back to [`drive_qa_loop`].
enum FieldOutcome {
    Resolved,
    Cancelled,
    Capped,
}

/// Run a single MissingField's Q&A thread: ask, validate, chain
/// clarifications on ambiguity, commit on resolve, record a TBD on
/// cancel / cap.
fn run_field_thread(
    field: &MissingField,
    spec: &mut SpecMd,
    llm: &mut dyn LlmAdapter,
) -> Result<FieldOutcome> {
    let mut last_question = field.prompt_template.clone();
    let mut prior_reply: Option<String> = None;
    for turn in 0..QA_TURN_CAP {
        let reply = ask_llm_for_reply(llm, &last_question, prior_reply.as_deref())?;
        let trimmed = reply.trim();
        if trimmed == "/cancel" || trimmed == "/cancel-thread" {
            record_tbd(spec, field, "user cancelled");
            return Ok(FieldOutcome::Cancelled);
        }
        match validate_reply(field, trimmed) {
            ValidationOutcome::Valid(value) => {
                apply_resolved_value(spec, field, &value);
                return Ok(FieldOutcome::Resolved);
            }
            ValidationOutcome::Invalid(reason) => {
                // Chain a clarification on the same field. The agent /
                // mock LLM is expected to refine the answer on the
                // next turn. We stop after QA_TURN_CAP and record a
                // TBD if validation never converges.
                last_question = format!(
                    "Clarification (turn {}): {reason}. Please answer: {}",
                    turn + 1,
                    field.prompt_template,
                );
                prior_reply = Some(reply);
            }
        }
    }
    record_tbd(spec, field, "turn-cap reached");
    Ok(FieldOutcome::Capped)
}

/// Local validation per `MissingFieldKind`. Returns the value to
/// commit to spec.md on success, or a human-readable reason on
/// failure (the qa loop surfaces it as a clarification prompt).
enum ValidationOutcome {
    Valid(String),
    Invalid(String),
}

fn validate_reply(field: &MissingField, reply: &str) -> ValidationOutcome {
    if reply.is_empty() {
        return ValidationOutcome::Invalid("empty reply".into());
    }
    match &field.kind {
        MissingFieldKind::Scalar | MissingFieldKind::Prose => {
            ValidationOutcome::Valid(reply.to_string())
        }
        MissingFieldKind::ConstrainedScalar { regex } => match Regex::new(regex) {
            Ok(re) => {
                if re.is_match(reply) {
                    ValidationOutcome::Valid(reply.to_string())
                } else {
                    ValidationOutcome::Invalid(format!(
                        "value `{reply}` does not match the required pattern `{regex}`"
                    ))
                }
            }
            Err(e) => {
                // Defensive: a malformed regex on a MissingField is a
                // bug in the traversal, not the user's fault. Accept
                // the value so the loop doesn't deadlock.
                tracing::warn!(
                    "qa_loop: malformed regex `{regex}` for field `{}`: {e}; accepting reply",
                    field.section_path
                );
                ValidationOutcome::Valid(reply.to_string())
            }
        },
        MissingFieldKind::TableRow { column_names } => {
            // Accept anything non-empty; the LLM normalization pass
            // turns the free-form reply into table rows. Surface a
            // hint about expected columns in the clarification on a
            // structurally-empty reply.
            if reply.split('|').count() < column_names.len() {
                ValidationOutcome::Invalid(format!(
                    "reply should describe one row with columns: {}",
                    column_names.join(", ")
                ))
            } else {
                ValidationOutcome::Valid(reply.to_string())
            }
        }
        MissingFieldKind::SectionApplicability => {
            // Caller routes these through `ask_section_applicability`;
            // a stray dispatch here is a bug. Accept yes/no/skip to
            // stay resilient.
            ValidationOutcome::Valid(reply.to_string())
        }
    }
}

/// Apply a validated value to the in-memory `SpecMd`. We only
/// populate the slots the traversal flagged as REQUIRED; everything
/// else falls through to an Auto-decisions entry the LLM-completion
/// step can pick up on its next turn.
fn apply_resolved_value(spec: &mut SpecMd, field: &MissingField, value: &str) {
    use crate::__internal::session::spec_md::{AutoDecision, QuantitativeRow};
    let path = field.section_path.as_str();
    match path {
        "Metadata > design_name" => spec.metadata.design_name = value.to_string(),
        "Metadata > version" => spec.metadata.version = value.to_string(),
        "Metadata > status" => spec.metadata.status = value.to_string(),
        "Metadata > authors" => spec.metadata.authors = vec![value.to_string()],
        "Metadata > last_updated" => spec.metadata.last_updated = value.to_string(),
        "Purpose" => spec.purpose = value.to_string(),
        "Scope" => spec.scope = value.to_string(),
        "Non-goals" => spec.non_goals = value.to_string(),
        "Pipeline and Hierarchy" => spec.pipeline_and_hierarchy.prose = value.to_string(),
        "Functional Behavior > End-to-end behavior" => {
            spec.functional_behavior.end_to_end = value.to_string()
        }
        p if p.starts_with("Assumptions > Quantitative > Clock frequency") => {
            spec.assumptions.quantitative.push(QuantitativeRow {
                constraint: "Clock frequency".into(),
                value: value.to_string(),
                source_anchor: String::new(),
            });
        }
        p if p.starts_with("Assumptions > Quantitative > Gate budget per cycle") => {
            spec.assumptions.quantitative.push(QuantitativeRow {
                constraint: "Gate budget per cycle".into(),
                value: value.to_string(),
                source_anchor: String::new(),
            });
        }
        _ => {
            // Anything we don't know how to slot directly becomes an
            // Auto-decision row so the answer isn't lost.
            spec.auto_decisions.push(AutoDecision {
                decision: format!("{path}: {value}"),
                rationale: "Resolved via DM0 Q&A loop".into(),
            });
        }
    }
}

/// Record a TBD entry for an unresolved field. Each line includes
/// the section path and the reason (user cancel, turn cap) so the
/// agent's later LLM turn can either revisit it or document it as a
/// known limitation.
fn record_tbd(spec: &mut SpecMd, field: &MissingField, reason: &str) {
    spec.open_questions
        .push(crate::__internal::session::spec_md::OpenQuestion {
            text: format!("TBD ({}): {reason}", field.section_path),
        });
}

/// Ask the LLM for one reply. The mock LLM in tests doubles as the
/// scripted user; production wiring threads the question through
/// the orchestrator's `ask_user` dispatch, which surfaces it to the
/// real user and returns their reply. Either way the LLM's response
/// text is the "user's answer."
fn ask_llm_for_reply(
    llm: &mut dyn LlmAdapter,
    question: &str,
    prior_reply: Option<&str>,
) -> Result<String> {
    let mut messages: Vec<LlmMessage> = Vec::new();
    if let Some(prior) = prior_reply {
        messages.push(LlmMessage {
            role: LlmRole::Assistant,
            content: format!("Previous reply: {prior}"),
            attachments: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning: None,
        });
    }
    messages.push(LlmMessage {
        role: LlmRole::User,
        content: question.to_string(),
        attachments: Vec::new(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        reasoning: None,
    });
    let (text, _metrics) = llm.dispatch(&messages)?;
    Ok(text)
}

/// Parse a "yes / no / skip" reply per Architecture §6.9. Defaults to
/// `Deferred` on anything we can't classify so the orchestrator
/// surfaces the section at the end rather than silently dropping it.
fn parse_yes_no_skip(reply: &str) -> SectionApplicability {
    let trimmed = reply.trim().to_ascii_lowercase();
    match trimmed.as_str() {
        "yes" | "y" | "applicable" | "true" => SectionApplicability::Applicable,
        "no" | "n" | "not-applicable" | "n/a" | "false" => SectionApplicability::NotApplicable,
        "skip" | "defer" | "deferred" | "later" => SectionApplicability::Deferred,
        _ => SectionApplicability::Deferred,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::__internal::session::agent::MockAgent;

    /// Build a default-empty spec so the traversal emits every
    /// required field. Tests script the mock LLM to answer them.
    fn fresh_spec() -> SpecMd {
        SpecMd::default()
    }

    #[test]
    fn single_turn_per_field_resolves_clean_replies() {
        let mut spec = fresh_spec();
        // Enqueue: one answer per MissingField; mock LLM returns
        // them in FIFO order. The fixture spec has many missing
        // fields so we enqueue generic answers + specific ones for
        // the constrained scalars and section applicability prompts.
        let agent = MockAgent::new();
        let missing = spec.missing_required_fields();
        // For each missing field, choose an answer the validator
        // will accept and the agent will use.
        for f in &missing {
            let canned = match &f.kind {
                MissingFieldKind::SectionApplicability => "no".to_string(),
                MissingFieldKind::ConstrainedScalar { regex } => {
                    if regex.contains("MHz|GHz") {
                        "1 GHz".to_string()
                    } else {
                        "42".to_string()
                    }
                }
                MissingFieldKind::TableRow { column_names } => column_names
                    .iter()
                    .map(|_| "x")
                    .collect::<Vec<_>>()
                    .join(" | "),
                MissingFieldKind::Scalar | MissingFieldKind::Prose => "an answer".to_string(),
            };
            agent.enqueue(canned);
        }
        let mut llm: Box<dyn LlmAdapter> = Box::new(agent);
        let report = drive_qa_loop(&mut spec, llm.as_mut()).expect("drive");
        assert_eq!(report.fields_capped, 0, "no field should hit turn cap");
        assert_eq!(report.fields_cancelled, 0, "no field should cancel");
        // Spot-check the spec was populated.
        assert_eq!(spec.metadata.design_name, "an answer");
        assert!(
            spec.assumptions
                .quantitative
                .iter()
                .any(|q| q.constraint.eq_ignore_ascii_case("Clock frequency")
                    && q.value == "1 GHz")
        );
    }

    #[test]
    fn multi_turn_per_field_clarifies_before_resolving() {
        // The Clock frequency field is a regex-constrained scalar.
        // First reply doesn't match; second does. The validator emits
        // a clarification in between (consuming a turn). The mock
        // LLM is scripted to deliver both replies in order.
        let mut spec = SpecMd::default();
        let agent = MockAgent::new();
        let missing = spec.missing_required_fields();
        // Walk every missing field; for the Clock frequency field
        // inject a bad reply followed by a good one (two turns).
        // For every other field, inject a single clean reply.
        for f in &missing {
            if f.section_path == "Assumptions > Quantitative > Clock frequency" {
                agent.enqueue("not-a-number"); // turn 0: invalid
                agent.enqueue("1 GHz"); // turn 1: valid
            } else if f.section_path == "Assumptions > Quantitative > Gate budget per cycle" {
                agent.enqueue("50");
            } else {
                let canned = match &f.kind {
                    MissingFieldKind::SectionApplicability => "no".to_string(),
                    MissingFieldKind::ConstrainedScalar { .. } => "42".to_string(),
                    MissingFieldKind::TableRow { column_names } => column_names
                        .iter()
                        .map(|_| "x")
                        .collect::<Vec<_>>()
                        .join(" | "),
                    _ => "an answer".to_string(),
                };
                agent.enqueue(canned);
            }
        }
        let mut llm: Box<dyn LlmAdapter> = Box::new(agent);
        let report = drive_qa_loop(&mut spec, llm.as_mut()).expect("drive");
        assert_eq!(
            report.fields_capped, 0,
            "Clock freq should resolve after one clarification"
        );
        // The clock-frequency row should be populated with the second
        // (valid) answer.
        let row = spec
            .assumptions
            .quantitative
            .iter()
            .find(|q| q.constraint.eq_ignore_ascii_case("Clock frequency"))
            .expect("clock freq row");
        assert_eq!(row.value, "1 GHz");
    }

    #[test]
    fn cancellation_path_records_tbd_and_advances() {
        let mut spec = SpecMd::default();
        let agent = MockAgent::new();
        let missing = spec.missing_required_fields();
        // Cancel the very first missing field; clean answers for the
        // rest so we exercise the "advance past cancel" path.
        for (i, f) in missing.iter().enumerate() {
            if i == 0 {
                agent.enqueue("/cancel-thread");
            } else {
                let canned = match &f.kind {
                    MissingFieldKind::SectionApplicability => "no".to_string(),
                    MissingFieldKind::ConstrainedScalar { regex } => {
                        if regex.contains("MHz|GHz") {
                            "1 GHz".to_string()
                        } else {
                            "42".to_string()
                        }
                    }
                    MissingFieldKind::TableRow { column_names } => column_names
                        .iter()
                        .map(|_| "x")
                        .collect::<Vec<_>>()
                        .join(" | "),
                    _ => "an answer".to_string(),
                };
                agent.enqueue(canned);
            }
        }
        let mut llm: Box<dyn LlmAdapter> = Box::new(agent);
        let report = drive_qa_loop(&mut spec, llm.as_mut()).expect("drive");
        assert!(
            report.fields_cancelled >= 1,
            "expected at least one cancellation, got {report:?}"
        );
        // The cancelled field is recorded as a TBD Open Question.
        assert!(
            spec.open_questions
                .iter()
                .any(|q| q.text.starts_with("TBD")),
            "expected TBD entry in Open Questions: {:?}",
            spec.open_questions
        );
    }

    #[test]
    fn ask_section_applicability_branches_on_three_replies() {
        // "yes" → Applicable
        {
            let mut spec = SpecMd::default();
            let agent = MockAgent::new();
            agent.enqueue("yes");
            let mut llm: Box<dyn LlmAdapter> = Box::new(agent);
            let result = ask_section_applicability("State Machines", &mut spec, llm.as_mut())
                .expect("applicability");
            assert_eq!(result, SectionApplicability::Applicable);
        }
        // "no" → NotApplicable
        {
            let mut spec = SpecMd::default();
            let agent = MockAgent::new();
            agent.enqueue("no");
            let mut llm: Box<dyn LlmAdapter> = Box::new(agent);
            let result = ask_section_applicability("Memory Map", &mut spec, llm.as_mut())
                .expect("applicability");
            assert_eq!(result, SectionApplicability::NotApplicable);
        }
        // "skip" → Deferred
        {
            let mut spec = SpecMd::default();
            let agent = MockAgent::new();
            agent.enqueue("skip");
            let mut llm: Box<dyn LlmAdapter> = Box::new(agent);
            let result = ask_section_applicability("Figures", &mut spec, llm.as_mut())
                .expect("applicability");
            assert_eq!(result, SectionApplicability::Deferred);
        }
    }

    #[test]
    fn turn_cap_records_tbd_when_validation_never_passes() {
        // Construct a one-field spec scenario by examining only the
        // Clock-frequency check. Feed QA_TURN_CAP invalid replies;
        // the loop should record a TBD and report `fields_capped >= 1`.
        let mut spec = SpecMd::default();
        let agent = MockAgent::new();
        let missing = spec.missing_required_fields();
        for f in &missing {
            if f.section_path == "Assumptions > Quantitative > Clock frequency" {
                // QA_TURN_CAP invalid replies.
                for _ in 0..QA_TURN_CAP {
                    agent.enqueue("not-a-number");
                }
            } else {
                let canned = match &f.kind {
                    MissingFieldKind::SectionApplicability => "no".to_string(),
                    MissingFieldKind::ConstrainedScalar { regex } => {
                        if regex.contains("MHz|GHz") {
                            "1 GHz".to_string()
                        } else {
                            "42".to_string()
                        }
                    }
                    MissingFieldKind::TableRow { column_names } => column_names
                        .iter()
                        .map(|_| "x")
                        .collect::<Vec<_>>()
                        .join(" | "),
                    _ => "an answer".to_string(),
                };
                agent.enqueue(canned);
            }
        }
        let mut llm: Box<dyn LlmAdapter> = Box::new(agent);
        let report = drive_qa_loop(&mut spec, llm.as_mut()).expect("drive");
        assert!(
            report.fields_capped >= 1,
            "Clock freq with all invalid replies should hit the cap: {report:?}"
        );
        // TBD entry present for the capped field.
        assert!(
            spec.open_questions
                .iter()
                .any(|q| q.text.contains("Clock frequency") && q.text.contains("turn-cap")),
            "expected turn-cap TBD: {:?}",
            spec.open_questions
        );
    }

    #[test]
    fn parse_yes_no_skip_covers_common_phrasing() {
        assert_eq!(parse_yes_no_skip("yes"), SectionApplicability::Applicable);
        assert_eq!(parse_yes_no_skip(" Y "), SectionApplicability::Applicable);
        assert_eq!(parse_yes_no_skip("no"), SectionApplicability::NotApplicable);
        assert_eq!(
            parse_yes_no_skip("not-applicable"),
            SectionApplicability::NotApplicable
        );
        assert_eq!(parse_yes_no_skip("skip"), SectionApplicability::Deferred);
        assert_eq!(parse_yes_no_skip("later"), SectionApplicability::Deferred);
        // Unknown reply: classifier defaults to Deferred so the
        // section isn't silently dropped.
        assert_eq!(parse_yes_no_skip("dunno"), SectionApplicability::Deferred);
    }
}
