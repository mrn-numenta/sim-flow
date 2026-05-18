//! Thread-aware spec.md persistence for `ask_user` (milestone 5.8 /
//! Architecture §6.5.4).
//!
//! At thread close the orchestrator writes EXACTLY ONE entry to
//! spec.md regardless of how many turns the thread carried.
//! Intermediate Q+A turns live in the chat panel and `metrics.jsonl`
//! for audit; only the resolved form lands in the spec.
//!
//! Two persistence sinks:
//!
//! - `docs/spec.md` exists -> append the resolved entry under the
//!   appropriate section heading (`## Open Questions` or
//!   `## Auto-decisions`).
//! - `docs/spec.md` does not exist (DM0 in progress) -> append the
//!   resolved entry to `.sim-flow/spec-ingest/qa-buffer.toml`. DM0
//!   pulls these in when it writes the initial spec.md.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::pending::RecordAs;
use super::threads::{ClosedAs, ResolvedThread};

/// Buffer-entry shape persisted to `qa-buffer.toml` when spec.md
/// doesn't exist yet.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedThreadRecord {
    pub thread_id: String,
    pub record_as: String,
    pub initial_question: String,
    pub final_answer: String,
    pub turn_count: u32,
    pub closed_as: String,
}

impl From<&ResolvedThread> for ResolvedThreadRecord {
    fn from(t: &ResolvedThread) -> Self {
        Self {
            thread_id: t.thread_id.clone(),
            record_as: persist_record_as(t).as_str().to_string(),
            initial_question: t.initial_question.clone(),
            final_answer: t.final_answer.clone(),
            turn_count: t.turn_count,
            closed_as: t.closed_as.as_str().to_string(),
        }
    }
}

/// Decide which `record_as` value applies to a resolved thread.
/// Cancelled threads always persist as `open-question` unresolved,
/// regardless of the agent's most recent `record_as` arg.
pub(crate) fn persist_record_as(thread: &ResolvedThread) -> RecordAs {
    match thread.closed_as {
        ClosedAs::AutoDecision => RecordAs::AutoDecision,
        ClosedAs::OpenQuestion
        | ClosedAs::Cancelled
        | ClosedAs::ThreadCancelled
        | ClosedAs::Abandoned
        | ClosedAs::ForceClosed => RecordAs::OpenQuestion,
    }
}

/// Anchor strings reported in the `recorded_at` field of the
/// `AskUserAnswer`.
const OPEN_QUESTIONS_ANCHOR: &str = "spec.md#open-questions";
const AUTO_DECISIONS_ANCHOR: &str = "spec.md#auto-decisions";
const QA_BUFFER_ANCHOR: &str = "qa-buffer.toml";

/// Persist a closed thread. Returns the anchor string that goes into
/// `AskUserAnswer.recorded_at`. Returns the empty string for
/// `record_as = "none"` thread closes (intermediate calls don't
/// flow into this function because the orchestrator skips the call
/// when `record_as = "none"` on the closing turn).
pub fn persist_resolved_thread(
    project_dir: &Path,
    thread: &ResolvedThread,
) -> std::io::Result<String> {
    let record_as = persist_record_as(thread);
    if matches!(record_as, RecordAs::None) {
        debug_assert!(false, "persist called with record_as = none");
        return Ok(String::new());
    }
    let spec_md = project_dir.join("docs").join("spec.md");
    if spec_md.is_file() {
        let anchor = match record_as {
            RecordAs::OpenQuestion => OPEN_QUESTIONS_ANCHOR,
            RecordAs::AutoDecision => AUTO_DECISIONS_ANCHOR,
            RecordAs::None => unreachable!(),
        };
        append_to_spec_md(&spec_md, thread, record_as)?;
        Ok(anchor.to_string())
    } else {
        append_to_qa_buffer(project_dir, thread)?;
        Ok(QA_BUFFER_ANCHOR.to_string())
    }
}

/// Append the resolved entry to a section of spec.md. Section is
/// created if absent.
fn append_to_spec_md(
    spec_md: &Path,
    thread: &ResolvedThread,
    record_as: RecordAs,
) -> std::io::Result<()> {
    let body = std::fs::read_to_string(spec_md)?;
    let section_heading = match record_as {
        RecordAs::OpenQuestion => "## Open Questions",
        RecordAs::AutoDecision => "## Auto-decisions",
        RecordAs::None => unreachable!(),
    };
    let entry = render_spec_md_entry(thread, record_as);
    let new_body = ensure_section_then_append(&body, section_heading, &entry);
    std::fs::write(spec_md, new_body)?;
    Ok(())
}

/// Render the single spec.md entry for a resolved thread. Multi-turn
/// threads include the "(arrived at through N rounds of clarification)"
/// annotation per Architecture §4.5 chaining section.
fn render_spec_md_entry(thread: &ResolvedThread, record_as: RecordAs) -> String {
    let annotation = if thread.turn_count > 1 {
        format!(
            " (arrived at through {} rounds of clarification)",
            thread.turn_count
        )
    } else {
        String::new()
    };
    match record_as {
        RecordAs::OpenQuestion => {
            if matches!(
                thread.closed_as,
                ClosedAs::ThreadCancelled | ClosedAs::Cancelled
            ) {
                format!(
                    "\n- **{}**: User cancelled clarification after {} exchange{}.\n",
                    escape_md(&thread.initial_question),
                    thread.turn_count,
                    if thread.turn_count == 1 { "" } else { "s" }
                )
            } else if matches!(thread.closed_as, ClosedAs::ForceClosed) {
                format!(
                    "\n- **{}**: Resolved through {} exchange{}; final answer: {}{}\n",
                    escape_md(&thread.initial_question),
                    thread.turn_count,
                    if thread.turn_count == 1 { "" } else { "s" },
                    escape_md(&thread.final_answer),
                    annotation,
                )
            } else if thread.final_answer.is_empty() {
                format!(
                    "\n- **{}**: unresolved{}\n",
                    escape_md(&thread.initial_question),
                    annotation,
                )
            } else {
                format!(
                    "\n- **{}**: {}{}\n",
                    escape_md(&thread.initial_question),
                    escape_md(&thread.final_answer),
                    annotation,
                )
            }
        }
        RecordAs::AutoDecision => {
            format!(
                "\n- **decision**: {}{}\n  **rationale**: {}\n",
                escape_md(&thread.final_answer),
                annotation,
                escape_md(&thread.initial_question),
            )
        }
        RecordAs::None => unreachable!(),
    }
}

fn escape_md(s: &str) -> String {
    s.replace('\n', " ").replace('\r', "").trim().to_string()
}

/// If `body` contains `heading`, append `entry` after the section's
/// existing content (i.e. before the next `## ` heading or EOF).
/// Otherwise append `\n{heading}\n{entry}\n` to the document.
fn ensure_section_then_append(body: &str, heading: &str, entry: &str) -> String {
    if let Some(start) = body.find(heading) {
        // Find the next `## ` (at line start) after the heading; if
        // absent, the section runs to EOF.
        let after_heading = start + heading.len();
        let rest = &body[after_heading..];
        // Search line-by-line so we skip the heading line and find
        // the next sibling.
        let mut offset = 0usize;
        let bytes = rest.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'\n' {
                // Start of next line: check for `## ` at this
                // position.
                let line_start = i + 1;
                if rest[line_start..].starts_with("## ") {
                    offset = line_start;
                    break;
                }
            }
            i += 1;
            offset = rest.len();
        }
        let insert_pos = after_heading + offset;
        let mut out = String::with_capacity(body.len() + entry.len());
        out.push_str(&body[..insert_pos]);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(entry);
        out.push_str(&body[insert_pos..]);
        out
    } else {
        let mut out = body.to_string();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
        out.push_str(heading);
        out.push('\n');
        out.push_str(entry);
        out
    }
}

fn append_to_qa_buffer(project_dir: &Path, thread: &ResolvedThread) -> std::io::Result<()> {
    let dir = project_dir.join(".sim-flow").join("spec-ingest");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("qa-buffer.toml");
    let mut entries: Vec<ResolvedThreadRecord> = if path.is_file() {
        let body = std::fs::read_to_string(&path)?;
        if body.trim().is_empty() {
            Vec::new()
        } else {
            let raw: QaBuffer = toml::from_str(&body)
                .map_err(|e| std::io::Error::other(format!("parse qa-buffer: {e}")))?;
            raw.entries
        }
    } else {
        Vec::new()
    };
    entries.push(thread.into());
    let serialized = toml::to_string_pretty(&QaBuffer { entries })
        .map_err(|e| std::io::Error::other(format!("serialize qa-buffer: {e}")))?;
    std::fs::write(path, serialized)?;
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct QaBuffer {
    #[serde(default)]
    entries: Vec<ResolvedThreadRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::__internal::session::ask_user::threads::{ClosedAs, ThreadTurn};

    fn make_thread(
        thread_id: &str,
        history: Vec<ThreadTurn>,
        closed_as: ClosedAs,
    ) -> ResolvedThread {
        let initial_question = history
            .first()
            .map(|t| t.question.clone())
            .unwrap_or_default();
        let final_answer = history
            .iter()
            .rev()
            .find(|t| !t.cancelled && !t.answer.is_empty())
            .map(|t| t.answer.clone())
            .unwrap_or_default();
        let turn_count = history.len() as u32;
        ResolvedThread {
            thread_id: thread_id.into(),
            step_id: "DM0".into(),
            closed_as,
            turn_count,
            initial_question,
            final_answer,
            history,
        }
    }

    #[test]
    fn single_turn_open_question_appends_to_spec_md() {
        let tmp = tempfile::tempdir().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("spec.md"), "# Spec\n\n## Open Questions\n\n").unwrap();
        let thread = make_thread(
            "t1",
            vec![ThreadTurn {
                question: "How wide?".into(),
                answer: "4".into(),
                turn_index: 0,
                record_as: RecordAs::OpenQuestion,
                cancelled: false,
            }],
            ClosedAs::OpenQuestion,
        );
        let anchor = persist_resolved_thread(tmp.path(), &thread).unwrap();
        assert_eq!(anchor, OPEN_QUESTIONS_ANCHOR);
        let body = std::fs::read_to_string(docs.join("spec.md")).unwrap();
        assert!(body.contains("How wide?"));
        assert!(body.contains(": 4"));
    }

    #[test]
    fn single_turn_auto_decision_appends_decision_row() {
        let tmp = tempfile::tempdir().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("spec.md"), "# Spec\n\n").unwrap();
        let thread = make_thread(
            "t1",
            vec![ThreadTurn {
                question: "Default endianness?".into(),
                answer: "little".into(),
                turn_index: 0,
                record_as: RecordAs::AutoDecision,
                cancelled: false,
            }],
            ClosedAs::AutoDecision,
        );
        let anchor = persist_resolved_thread(tmp.path(), &thread).unwrap();
        assert_eq!(anchor, AUTO_DECISIONS_ANCHOR);
        let body = std::fs::read_to_string(docs.join("spec.md")).unwrap();
        assert!(body.contains("## Auto-decisions"));
        assert!(body.contains("decision"));
        assert!(body.contains("little"));
        assert!(body.contains("Default endianness?"));
    }

    #[test]
    fn multi_turn_thread_coalesces_with_annotation() {
        let tmp = tempfile::tempdir().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("spec.md"), "# Spec\n\n").unwrap();
        let thread = make_thread(
            "t1",
            vec![
                ThreadTurn {
                    question: "Pick width".into(),
                    answer: "probably 4".into(),
                    turn_index: 0,
                    record_as: RecordAs::None,
                    cancelled: false,
                },
                ThreadTurn {
                    question: "Specifically 4?".into(),
                    answer: "yes 4".into(),
                    turn_index: 1,
                    record_as: RecordAs::None,
                    cancelled: false,
                },
                ThreadTurn {
                    question: "Confirm 4-wide".into(),
                    answer: "yes confirmed 4".into(),
                    turn_index: 2,
                    record_as: RecordAs::AutoDecision,
                    cancelled: false,
                },
            ],
            ClosedAs::AutoDecision,
        );
        persist_resolved_thread(tmp.path(), &thread).unwrap();
        let body = std::fs::read_to_string(docs.join("spec.md")).unwrap();
        // Exactly one entry, even though 3 turns happened.
        let count = body.matches("**decision**").count();
        assert_eq!(count, 1);
        assert!(body.contains("3 rounds of clarification"));
    }

    #[test]
    fn cancelled_thread_persists_as_unresolved_open_question() {
        let tmp = tempfile::tempdir().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("spec.md"), "# Spec\n\n## Open Questions\n\n").unwrap();
        let thread = make_thread(
            "t1",
            vec![
                ThreadTurn {
                    question: "Pick width".into(),
                    answer: "maybe 4".into(),
                    turn_index: 0,
                    record_as: RecordAs::None,
                    cancelled: false,
                },
                ThreadTurn {
                    question: "Specifically 4?".into(),
                    answer: String::new(),
                    turn_index: 1,
                    record_as: RecordAs::None,
                    cancelled: true,
                },
            ],
            ClosedAs::ThreadCancelled,
        );
        persist_resolved_thread(tmp.path(), &thread).unwrap();
        let body = std::fs::read_to_string(docs.join("spec.md")).unwrap();
        assert!(body.contains("User cancelled clarification after 2 exchanges"));
    }

    #[test]
    fn spec_md_missing_writes_qa_buffer() {
        let tmp = tempfile::tempdir().unwrap();
        let thread = make_thread(
            "t1",
            vec![ThreadTurn {
                question: "How wide?".into(),
                answer: "4".into(),
                turn_index: 0,
                record_as: RecordAs::OpenQuestion,
                cancelled: false,
            }],
            ClosedAs::OpenQuestion,
        );
        let anchor = persist_resolved_thread(tmp.path(), &thread).unwrap();
        assert_eq!(anchor, QA_BUFFER_ANCHOR);
        let buf = tmp
            .path()
            .join(".sim-flow")
            .join("spec-ingest")
            .join("qa-buffer.toml");
        assert!(buf.is_file());
        let body = std::fs::read_to_string(&buf).unwrap();
        assert!(body.contains("How wide?"));
        assert!(body.contains("record_as"));
    }

    #[test]
    fn appending_creates_missing_section() {
        let tmp = tempfile::tempdir().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("spec.md"), "# Spec\n\n## Intro\n\nstuff.\n").unwrap();
        let thread = make_thread(
            "t1",
            vec![ThreadTurn {
                question: "x?".into(),
                answer: "y".into(),
                turn_index: 0,
                record_as: RecordAs::OpenQuestion,
                cancelled: false,
            }],
            ClosedAs::OpenQuestion,
        );
        persist_resolved_thread(tmp.path(), &thread).unwrap();
        let body = std::fs::read_to_string(docs.join("spec.md")).unwrap();
        assert!(body.contains("## Open Questions"));
        assert!(body.contains("x?"));
    }
}
