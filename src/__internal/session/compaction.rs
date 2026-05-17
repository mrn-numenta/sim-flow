//! Deterministic prompt-stack compaction (Phase 1a).
//!
//! Compaction runs immediately before each `dispatch_with_tools`
//! call and shrinks the working `messages: Vec<LlmMessage>` by
//! dropping turns whose information is now redundant or
//! affirmatively misleading. No LLM round-trip is involved -- the
//! rules are purely structural.
//!
//! Rules implemented today:
//!
//! * **Path-keyed dedup** -- when the agent reads the same path
//!   twice with the same tool, the earlier result's content is
//!   replaced with a metadata stub (`<superseded: see turn N>`)
//!   and the original turn is reported as evicted. The agent
//!   still sees the *stub* turn so it knows the read existed but
//!   doesn't carry the content forward. Wired tools today:
//!   `read_file`, `list_directory`.
//!
//! Rules planned for follow-up commits:
//!
//! * Mutation invalidation (`write_file` / `edit_file` invalidates
//!   prior `read_file` results for the same path).
//! * Phase-boundary cleanup (drop tool results not cited by the
//!   final assistant text of a sub-session).
//! * Universal per-tool output caps.
//!
//! Tests live alongside the rules in this file; the module is
//! deliberately pure (no I/O, no host wiring) so it can be
//! exercised cheaply.
//!
//! Caller contract: pass slices of (id, message) pairs into the
//! rule functions. The function returns an `EvictionReport`
//! describing which ids dropped out and (when applicable) the
//! stubbed replacement content for in-place rewriting. Callers
//! handle the mutation + the `Event::ContextEvicted` emission.

use serde_json::Value as JsonValue;

use crate::session::protocol::{ContextEvictionReason, LlmMessage, LlmRole, LlmToolCall};

/// Build the position-based id for the message at index `idx` in
/// the orchestrator's current `messages: Vec<LlmMessage>` stack.
/// Stable as long as messages are never *removed* from the vec --
/// stub-replacement preserves the index. The next compaction
/// rule that wants to remove (phase-boundary cleanup, overflow
/// trim) needs a stronger id scheme; that lands together with
/// the removal logic.
pub fn position_id(idx: usize) -> String {
    format!("msg-{idx}")
}

/// Reverse of `position_id`: parse a "msg-N" id back to an index.
/// Returns `None` for ids that don't match the scheme.
pub fn position_id_index(id: &str) -> Option<usize> {
    id.strip_prefix("msg-")
        .and_then(|s| s.parse::<usize>().ok())
}

/// Borrow each (id, message) pair from a `Vec<LlmMessage>` for
/// passing into compaction rules. The id is the message's position
/// in the slice, formatted as `msg-N`.
pub fn position_pairs(messages: &[LlmMessage]) -> Vec<(String, &LlmMessage)> {
    messages
        .iter()
        .enumerate()
        .map(|(i, m)| (position_id(i), m))
        .collect()
}

/// Outcome of one compaction pass. Each rule produces zero or more
/// evictions; the caller aggregates them and emits a single
/// `ContextEvicted` event per rule firing.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct EvictionReport {
    /// IDs of messages that should be dropped from the prompt
    /// stack. Caller looks them up by id and removes them.
    pub dropped: Vec<String>,
    /// Per-id replacement content the caller should swap in BEFORE
    /// dropping the originals. Used to keep a placeholder stub so
    /// the agent still sees that a read existed without re-fetching.
    ///
    /// Today only path-keyed dedup populates this (it replaces the
    /// superseded `Tool` message's content with a stub pointing at
    /// the live one); other rules leave it empty.
    pub stubs: Vec<(String, String)>,
}

impl EvictionReport {
    pub fn is_empty(&self) -> bool {
        self.dropped.is_empty() && self.stubs.is_empty()
    }
}

/// Tool calls we know how to dedup. Adding a new tool here is the
/// minimum change needed to extend dedup coverage; the matcher
/// shape is "same tool + same argument value" so any
/// idempotent-result-by-arg tool qualifies.
fn dedup_path_tool(name: &str) -> Option<&'static str> {
    match name {
        "read_file" => Some("path"),
        "list_directory" => Some("path"),
        _ => None,
    }
}

/// Helper: extract the value of an argument from a tool call's
/// `arguments_json` field. Returns `None` when the field is
/// missing or the json is malformed -- callers treat that as
/// "can't dedup this one" and leave it alone.
fn extract_arg(call: &LlmToolCall, arg_name: &str) -> Option<String> {
    let parsed: JsonValue = serde_json::from_str(&call.arguments_json).ok()?;
    parsed
        .get(arg_name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Build the stub-replacement content shown to the agent on the
/// older (superseded) tool result. Carries enough metadata to
/// preserve the loop-avoidance hint (agent knows it has read this
/// before) without re-shipping the body.
fn supersession_stub(tool: &str, arg_name: &str, arg_value: &str) -> String {
    format!(
        "<superseded: a later turn re-invoked `{tool}({arg_name}={arg_value})`; \
         the agent has the fresh result; original body compacted out of context.>"
    )
}

/// Walk the supplied `(id, message)` pairs from oldest to newest,
/// tracking the most recent (tool, arg) -> message-id mapping. For
/// each older message superseded by a newer one with the same key,
/// emit an eviction + a stub.
///
/// The walk lives on the caller's storage; this function never
/// mutates the supplied slice. The caller applies `report.stubs`
/// in place AND removes the `report.dropped` ids in whatever order
/// works for their backing structure.
///
/// Pure function; no I/O; cheap to call once per dispatch.
pub fn run_path_keyed_dedup(messages: &[(String, &LlmMessage)]) -> EvictionReport {
    use std::collections::HashMap;

    let mut last_seen: HashMap<(String, String, String), String> = HashMap::new();
    let mut report = EvictionReport::default();

    // First pass: collect (tool, arg, value) -> latest id.
    for (id, msg) in messages {
        if msg.role != LlmRole::Assistant {
            continue;
        }
        for call in &msg.tool_calls {
            let Some(arg_name) = dedup_path_tool(&call.name) else {
                continue;
            };
            let Some(arg_value) = extract_arg(call, arg_name) else {
                continue;
            };
            let key = (call.name.clone(), arg_name.to_string(), arg_value);
            // The assistant's tool_call lives on the assistant message,
            // but the *result* lives on the next Tool-role message with
            // a matching tool_call_id. The eviction target is the
            // Tool-role message: that's where the (potentially large)
            // result body lives.
            //
            // We can't easily look forward here, so the matcher below
            // scans Tool-role messages keyed by tool_call_id. Record
            // the assistant id only for reference.
            last_seen.insert(key, id.clone());
        }
    }

    // Second pass: for each Tool-role result, walk back to the
    // assistant call that produced it. If a LATER assistant call
    // with the same (tool, arg, value) exists, evict this Tool
    // message and stub it.
    //
    // Map from tool_call_id -> assistant message id + (tool, arg,
    // value) so we can correlate Tool results to their calls.
    let mut call_keys: HashMap<String, (String, String, String, String)> = HashMap::new();
    for (assistant_id, msg) in messages {
        if msg.role != LlmRole::Assistant {
            continue;
        }
        for call in &msg.tool_calls {
            let Some(arg_name) = dedup_path_tool(&call.name) else {
                continue;
            };
            let Some(arg_value) = extract_arg(call, arg_name) else {
                continue;
            };
            let Some(call_id) = call.id.as_ref() else {
                continue;
            };
            call_keys.insert(
                call_id.clone(),
                (
                    assistant_id.clone(),
                    call.name.clone(),
                    arg_name.to_string(),
                    arg_value,
                ),
            );
        }
    }

    // Now, for each (id, Tool-role msg), look up its call key.
    // Determine if a later assistant message produced a call with
    // the same (tool, arg, value). If so, this Tool result is
    // superseded.
    //
    // Latest assistant per key is in `last_seen`. The Tool message
    // is superseded when its assistant ancestor is *not* the
    // latest one.
    for (tool_id, msg) in messages {
        if msg.role != LlmRole::Tool {
            continue;
        }
        let Some(call_id) = msg.tool_call_id.as_ref() else {
            continue;
        };
        let Some((assistant_id, tool_name, arg_name, arg_value)) = call_keys.get(call_id) else {
            continue;
        };
        let key = (tool_name.clone(), arg_name.clone(), arg_value.clone());
        let Some(latest_assistant) = last_seen.get(&key) else {
            continue;
        };
        if latest_assistant == assistant_id {
            // This Tool result IS the latest. Keep it.
            continue;
        }
        // Superseded.
        report.dropped.push(tool_id.clone());
        report.stubs.push((
            tool_id.clone(),
            supersession_stub(tool_name, arg_name, arg_value),
        ));
    }

    report
}

/// Convenience wrapper: stamp dedup evictions with the standard
/// `SupersededByDedup` reason. Other rules will get their own
/// wrapper as they land.
pub fn dedup_reason() -> ContextEvictionReason {
    ContextEvictionReason::SupersededByDedup
}

/// Reason stamp for the mutation-invalidation rule.
pub fn mutation_reason() -> ContextEvictionReason {
    ContextEvictionReason::InvalidatedByMutation
}

/// Tool calls that *mutate* a path on disk. A `read_file` result
/// for any path mutated later in the session is now misleading --
/// the cached body shows the pre-edit content the agent might
/// re-edit. The rule below scans for these and evicts.
fn mutation_path_tool(name: &str) -> Option<&'static str> {
    match name {
        "write_file" => Some("path"),
        "edit_file" => Some("path"),
        "delete_file" => Some("path"),
        _ => None,
    }
}

/// Walk the (id, message) pairs and evict `read_file` Tool-role
/// results whose path was subsequently mutated by a `write_file` /
/// `edit_file` / `delete_file` call. The stub points at the
/// mutation turn so the agent knows the cached read is no longer
/// authoritative.
///
/// Same semantics as `run_path_keyed_dedup`: pure, no I/O,
/// in-place caller mutation via the returned stubs.
pub fn run_mutation_invalidation(messages: &[(String, &LlmMessage)]) -> EvictionReport {
    use std::collections::{HashMap, HashSet};

    let mut report = EvictionReport::default();

    // Phase 1: collect the set of mutated paths and the FIRST
    // turn index at which each path was mutated (by message
    // position in the slice). A later mutation doesn't invalidate
    // a later read; only earlier reads are stale.
    let mut first_mutated_at: HashMap<String, usize> = HashMap::new();
    for (idx, (_id, msg)) in messages.iter().enumerate() {
        if msg.role != LlmRole::Assistant {
            continue;
        }
        for call in &msg.tool_calls {
            let Some(arg_name) = mutation_path_tool(&call.name) else {
                continue;
            };
            let Some(arg_value) = extract_arg(call, arg_name) else {
                continue;
            };
            first_mutated_at.entry(arg_value).or_insert(idx);
        }
    }

    if first_mutated_at.is_empty() {
        return report;
    }

    // Phase 2: index every assistant `read_file` call by its
    // tool_call_id so we can correlate Tool-role results back to
    // an assistant call + path + position.
    let mut read_calls: HashMap<String, (usize, String)> = HashMap::new();
    for (idx, (_id, msg)) in messages.iter().enumerate() {
        if msg.role != LlmRole::Assistant {
            continue;
        }
        for call in &msg.tool_calls {
            if call.name != "read_file" {
                continue;
            }
            let Some(arg_value) = extract_arg(call, "path") else {
                continue;
            };
            let Some(call_id) = call.id.as_ref() else {
                continue;
            };
            read_calls.insert(call_id.clone(), (idx, arg_value));
        }
    }

    // Phase 3: for each Tool-role read result, evict when its
    // path appears in `first_mutated_at` AND the read happened
    // BEFORE that mutation. We track ids we've already evicted
    // via dedup separately -- callers run dedup before mutation
    // invalidation, so a `read_file` already stubbed by dedup
    // won't be re-evicted here (the stub content doesn't match
    // an assistant read_call any more).
    let mut already_evicted: HashSet<String> = HashSet::new();
    for (tool_id, msg) in messages {
        if msg.role != LlmRole::Tool {
            continue;
        }
        let Some(call_id) = msg.tool_call_id.as_ref() else {
            continue;
        };
        let Some((read_at, path)) = read_calls.get(call_id) else {
            continue;
        };
        let Some(mutated_at) = first_mutated_at.get(path) else {
            continue;
        };
        if *read_at >= *mutated_at {
            // Read happened at or after the mutation; the cached
            // content is fine.
            continue;
        }
        if !already_evicted.insert(tool_id.clone()) {
            continue;
        }
        report.dropped.push(tool_id.clone());
        report
            .stubs
            .push((tool_id.clone(), invalidation_stub(path)));
    }

    report
}

fn invalidation_stub(path: &str) -> String {
    format!(
        "<invalidated: a later turn wrote / edited `{path}`; \
         the cached read result no longer reflects disk content; \
         compacted out of context.>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::{LlmMessage, LlmRole, LlmToolCall};

    fn assistant_with_call(
        id: &str,
        call_id: &str,
        tool: &str,
        arg_json: &str,
    ) -> (String, LlmMessage) {
        (
            id.to_string(),
            LlmMessage {
                role: LlmRole::Assistant,
                content: String::new(),
                attachments: vec![],
                tool_call_id: None,
                tool_calls: vec![LlmToolCall {
                    id: Some(call_id.to_string()),
                    name: tool.to_string(),
                    arguments_json: arg_json.to_string(),
                }],
                reasoning: None,
            },
        )
    }

    fn tool_result(id: &str, call_id: &str, body: &str) -> (String, LlmMessage) {
        (
            id.to_string(),
            LlmMessage {
                role: LlmRole::Tool,
                content: body.to_string(),
                attachments: vec![],
                tool_call_id: Some(call_id.to_string()),
                tool_calls: vec![],
                reasoning: None,
            },
        )
    }

    fn user(id: &str, body: &str) -> (String, LlmMessage) {
        (
            id.to_string(),
            LlmMessage {
                role: LlmRole::User,
                content: body.to_string(),
                attachments: vec![],
                tool_call_id: None,
                tool_calls: vec![],
                reasoning: None,
            },
        )
    }

    fn as_refs(pairs: &[(String, LlmMessage)]) -> Vec<(String, &LlmMessage)> {
        pairs.iter().map(|(id, m)| (id.clone(), m)).collect()
    }

    #[test]
    fn dedup_leaves_single_read_alone() {
        let msgs = vec![
            assistant_with_call("a1", "c1", "read_file", r#"{"path":"docs/spec.md"}"#),
            tool_result("t1", "c1", "..."),
        ];
        let report = run_path_keyed_dedup(&as_refs(&msgs));
        assert!(
            report.is_empty(),
            "single read should not produce evictions"
        );
    }

    #[test]
    fn dedup_evicts_earlier_read_of_same_path() {
        let msgs = vec![
            assistant_with_call("a1", "c1", "read_file", r#"{"path":"docs/spec.md"}"#),
            tool_result("t1", "c1", "old content"),
            user("u1", "go on"),
            assistant_with_call("a2", "c2", "read_file", r#"{"path":"docs/spec.md"}"#),
            tool_result("t2", "c2", "fresh content"),
        ];
        let report = run_path_keyed_dedup(&as_refs(&msgs));
        assert_eq!(report.dropped, vec!["t1".to_string()]);
        assert_eq!(report.stubs.len(), 1);
        assert_eq!(report.stubs[0].0, "t1");
        assert!(report.stubs[0].1.contains("superseded"));
        assert!(report.stubs[0].1.contains("read_file"));
        assert!(report.stubs[0].1.contains("docs/spec.md"));
    }

    #[test]
    fn dedup_keeps_latest_when_three_reads() {
        let msgs = vec![
            assistant_with_call("a1", "c1", "read_file", r#"{"path":"src/lib.rs"}"#),
            tool_result("t1", "c1", "v1"),
            assistant_with_call("a2", "c2", "read_file", r#"{"path":"src/lib.rs"}"#),
            tool_result("t2", "c2", "v2"),
            assistant_with_call("a3", "c3", "read_file", r#"{"path":"src/lib.rs"}"#),
            tool_result("t3", "c3", "v3"),
        ];
        let report = run_path_keyed_dedup(&as_refs(&msgs));
        // Earlier two reads superseded; the latest stays.
        assert_eq!(report.dropped.len(), 2);
        assert!(report.dropped.contains(&"t1".to_string()));
        assert!(report.dropped.contains(&"t2".to_string()));
        assert!(!report.dropped.contains(&"t3".to_string()));
    }

    #[test]
    fn dedup_treats_different_paths_as_independent() {
        let msgs = vec![
            assistant_with_call("a1", "c1", "read_file", r#"{"path":"docs/spec.md"}"#),
            tool_result("t1", "c1", "spec body"),
            assistant_with_call("a2", "c2", "read_file", r#"{"path":"docs/targets.md"}"#),
            tool_result("t2", "c2", "targets body"),
        ];
        let report = run_path_keyed_dedup(&as_refs(&msgs));
        assert!(report.is_empty(), "different paths should not collide");
    }

    #[test]
    fn dedup_handles_list_directory_too() {
        let msgs = vec![
            assistant_with_call("a1", "c1", "list_directory", r#"{"path":"docs/"}"#),
            tool_result("t1", "c1", "old listing"),
            assistant_with_call("a2", "c2", "list_directory", r#"{"path":"docs/"}"#),
            tool_result("t2", "c2", "fresh listing"),
        ];
        let report = run_path_keyed_dedup(&as_refs(&msgs));
        assert_eq!(report.dropped, vec!["t1".to_string()]);
    }

    #[test]
    fn dedup_does_not_cross_tool_boundaries() {
        let msgs = vec![
            assistant_with_call("a1", "c1", "read_file", r#"{"path":"docs/spec.md"}"#),
            tool_result("t1", "c1", "read body"),
            assistant_with_call("a2", "c2", "list_directory", r#"{"path":"docs/spec.md"}"#),
            tool_result("t2", "c2", "listing body"),
        ];
        let report = run_path_keyed_dedup(&as_refs(&msgs));
        assert!(
            report.is_empty(),
            "read_file and list_directory have different result shapes; should not collapse"
        );
    }

    #[test]
    fn dedup_skips_unparsable_arguments_json() {
        let msgs = vec![
            assistant_with_call("a1", "c1", "read_file", "this is not json"),
            tool_result("t1", "c1", "body"),
            assistant_with_call("a2", "c2", "read_file", r#"{"path":"x.md"}"#),
            tool_result("t2", "c2", "body2"),
        ];
        let report = run_path_keyed_dedup(&as_refs(&msgs));
        // The unparseable arg means we can't say t1 is superseded by t2;
        // leave both alone.
        assert!(report.is_empty());
    }

    #[test]
    fn dedup_ignores_tools_without_a_path_arg() {
        let msgs = vec![
            assistant_with_call("a1", "c1", "run_shell", r#"{"cmd":"ls"}"#),
            tool_result("t1", "c1", "result"),
            assistant_with_call("a2", "c2", "run_shell", r#"{"cmd":"ls"}"#),
            tool_result("t2", "c2", "result"),
        ];
        let report = run_path_keyed_dedup(&as_refs(&msgs));
        assert!(report.is_empty(), "run_shell isn't on the dedup allowlist");
    }

    #[test]
    fn dedup_reason_constant_is_set() {
        assert_eq!(dedup_reason(), ContextEvictionReason::SupersededByDedup);
    }

    #[test]
    fn mutation_reason_constant_is_set() {
        assert_eq!(
            mutation_reason(),
            ContextEvictionReason::InvalidatedByMutation
        );
    }

    #[test]
    fn mutation_invalidation_no_writes_is_noop() {
        let msgs = vec![
            assistant_with_call("a1", "c1", "read_file", r#"{"path":"src/lib.rs"}"#),
            tool_result("t1", "c1", "body"),
        ];
        let report = run_mutation_invalidation(&as_refs(&msgs));
        assert!(report.is_empty());
    }

    #[test]
    fn mutation_invalidation_evicts_pre_write_read() {
        let msgs = vec![
            assistant_with_call("a1", "c1", "read_file", r#"{"path":"src/lib.rs"}"#),
            tool_result("t1", "c1", "pre-edit body"),
            assistant_with_call(
                "a2",
                "c2",
                "write_file",
                r#"{"path":"src/lib.rs","content":"new"}"#,
            ),
            tool_result("t2", "c2", "ok"),
        ];
        let report = run_mutation_invalidation(&as_refs(&msgs));
        assert_eq!(report.dropped, vec!["t1".to_string()]);
        assert_eq!(report.stubs.len(), 1);
        assert!(report.stubs[0].1.contains("invalidated"));
        assert!(report.stubs[0].1.contains("src/lib.rs"));
    }

    #[test]
    fn mutation_invalidation_keeps_post_write_read() {
        let msgs = vec![
            assistant_with_call(
                "a1",
                "c1",
                "write_file",
                r#"{"path":"src/lib.rs","content":"new"}"#,
            ),
            tool_result("t1", "c1", "ok"),
            assistant_with_call("a2", "c2", "read_file", r#"{"path":"src/lib.rs"}"#),
            tool_result("t2", "c2", "fresh body"),
        ];
        let report = run_mutation_invalidation(&as_refs(&msgs));
        assert!(report.is_empty(), "the read happened AFTER the write");
    }

    #[test]
    fn mutation_invalidation_handles_edit_and_delete() {
        let msgs = vec![
            assistant_with_call("a1", "c1", "read_file", r#"{"path":"a.md"}"#),
            tool_result("t1", "c1", "a body"),
            assistant_with_call("a2", "c2", "read_file", r#"{"path":"b.md"}"#),
            tool_result("t2", "c2", "b body"),
            assistant_with_call(
                "a3",
                "c3",
                "edit_file",
                r#"{"path":"a.md","old":"x","new":"y"}"#,
            ),
            tool_result("t3", "c3", "ok"),
            assistant_with_call("a4", "c4", "delete_file", r#"{"path":"b.md"}"#),
            tool_result("t4", "c4", "ok"),
        ];
        let report = run_mutation_invalidation(&as_refs(&msgs));
        assert_eq!(report.dropped.len(), 2);
        assert!(report.dropped.contains(&"t1".to_string()));
        assert!(report.dropped.contains(&"t2".to_string()));
    }

    #[test]
    fn mutation_invalidation_does_not_touch_unrelated_paths() {
        let msgs = vec![
            assistant_with_call("a1", "c1", "read_file", r#"{"path":"a.md"}"#),
            tool_result("t1", "c1", "body"),
            assistant_with_call(
                "a2",
                "c2",
                "write_file",
                r#"{"path":"b.md","content":"new"}"#,
            ),
            tool_result("t2", "c2", "ok"),
        ];
        let report = run_mutation_invalidation(&as_refs(&msgs));
        assert!(report.is_empty(), "different paths; read is still valid");
    }
}
