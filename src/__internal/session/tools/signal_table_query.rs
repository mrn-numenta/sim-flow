//! `signal_table_query(filter, conflicts_only?, limit?)` -- L7
//! retrieval tool backed by `signal_table_rows` (Chapter 4 §4.4).
//!
//! Two modes:
//!
//! - Regular: scalar query (equality filters AND'd together) returns
//!   matching rows.
//! - Conflicts-only: returns the spec-md vs source-spec pairs that
//!   disagree on (direction, peer, description) for the same
//!   (stage, signal_name).

use std::sync::Arc;

use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolResult};
use crate::__internal::session::lance_index::query::SignalFilter;
use crate::__internal::session::retrieval::{RetrievalError, RetrievalService};
use crate::Result;

const DEFAULT_LIMIT: usize = 50;
const MIN_LIMIT: usize = 1;
const MAX_LIMIT: usize = 500;

/// Whitelist of `filter` object keys; an unknown key is a structured
/// error so the agent self-corrects.
const FILTER_KEYS: &[&str] = &[
    "signal_name",
    "stage",
    "peer",
    "direction",
    "source_kind",
    "source_id",
];

pub struct SignalTableQueryTool {
    service: Arc<RetrievalService>,
}

impl SignalTableQueryTool {
    pub fn new(service: Arc<RetrievalService>) -> Self {
        Self { service }
    }
}

impl Tool for SignalTableQueryTool {
    fn name(&self) -> &'static str {
        "signal_table_query"
    }

    fn description(&self) -> &'static str {
        "Structured query over the project's signal-table rows (per-block I/O signals, both source-spec and spec-md sourced). Use this to enumerate I/O for a stage/block or to look up a signal by name. Set `conflicts_only=true` to surface (stage, signal_name) pairs where the spec.md row disagrees with the source-spec row."
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["filter"],
            "properties": {
                "filter": {
                    "type": "object",
                    "properties": {
                        "signal_name": { "type": "string" },
                        "stage": { "type": "string" },
                        "peer": { "type": "string" },
                        "direction": { "type": "string", "enum": ["in", "out", "inout"] },
                        "source_kind": { "type": "string", "enum": ["source-spec", "spec-md"] },
                        "source_id": { "type": "string" }
                    },
                    "additionalProperties": false,
                    "description": "Equality filters; any subset. AND'd together."
                },
                "conflicts_only": {
                    "type": "boolean",
                    "default": false
                },
                "limit": {
                    "type": "integer",
                    "minimum": MIN_LIMIT,
                    "maximum": MAX_LIMIT,
                    "default": DEFAULT_LIMIT
                }
            }
        })
    }

    fn invoke(&self, _ctx: &ToolContext, args: &Value) -> Result<ToolResult> {
        // `filter` is required.
        let filter_value = match args.get("filter") {
            Some(v) if v.is_object() => v,
            _ => {
                return Ok(ToolResult::err(
                    "signal_table_query: missing or non-object `filter` arg",
                ));
            }
        };

        // Validate filter keys against the whitelist.
        if let Some(obj) = filter_value.as_object() {
            for key in obj.keys() {
                if !FILTER_KEYS.contains(&key.as_str()) {
                    return Ok(ToolResult::err(format!(
                        "signal_table_query: unknown filter key `{key}` (allowed: {})",
                        FILTER_KEYS.join(", ")
                    )));
                }
            }
        }

        let conflicts_only = args
            .get("conflicts_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(MIN_LIMIT, MAX_LIMIT);

        if !self.service.has_spec() {
            // Per Architecture §4.4 "no signal-table data" failure
            // mode: return empty rows with a note when the index
            // simply isn't built.
            let payload = json!({
                "rows": [],
                "note": "no signal tables registered for this project",
                "embedder_used": self.service.embedder_label(),
                "elapsed_ms": 0,
            });
            return Ok(ToolResult::ok(payload.to_string()));
        }

        // Cold-start UX: see api_semantic_search.
        if self.service.take_cold_start() {
            tracing::info!(
                target: "sim_flow::diagnostics",
                level = "info",
                message = "warming retrieval index (first call may take 5-15s on cold embedder)"
            );
        }

        let filter = SignalFilter {
            signal_name: read_string(filter_value, "signal_name"),
            stage: read_string(filter_value, "stage"),
            peer: read_string(filter_value, "peer"),
            direction: read_string(filter_value, "direction"),
            source_kind: read_string(filter_value, "source_kind"),
            source_id: read_string(filter_value, "source_id"),
        };

        let start = std::time::Instant::now();
        let payload = if conflicts_only {
            match self.service.find_signal_conflicts_sync() {
                Ok(conflicts) => {
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    tracing::info!(
                        target: "sim_flow::metrics",
                        event = "retrieval_call",
                        tool = "signal_table_query",
                        mode = "conflicts_only",
                        elapsed_ms = elapsed_ms,
                        k_requested = limit,
                        k_returned = conflicts.len(),
                    );
                    let conflict_pairs: Vec<Value> = conflicts
                        .iter()
                        .map(|c| {
                            let differs_on = c
                                .reason
                                .split(';')
                                .filter_map(|piece| {
                                    piece.trim().split(' ').next().map(|s| s.to_string())
                                })
                                .filter(|s| !s.is_empty())
                                .collect::<Vec<_>>();
                            json!({
                                "stage": c.stage,
                                "signal_name": c.signal_name,
                                "spec_md_row": row_to_json(&c.spec_md),
                                "source_spec_row": row_to_json(&c.source_spec),
                                "differs_on": differs_on,
                                "reason": c.reason,
                            })
                        })
                        .collect();
                    json!({
                        "rows": [],
                        "total_matching": conflicts.len(),
                        "limited": false,
                        "conflict_pairs": conflict_pairs,
                        "embedder_used": self.service.embedder_label(),
                        "elapsed_ms": elapsed_ms,
                    })
                }
                Err(RetrievalError::IndexMissing { which }) => {
                    return Ok(ToolResult::err(format!(
                        "signal_table_query: {which} index not built; run `sim-flow build-spec-index`"
                    )));
                }
                Err(e) => {
                    return Ok(ToolResult::err(format!(
                        "signal_table_query: {}",
                        render_retrieval_error(&e)
                    )));
                }
            }
        } else {
            match self.service.query_signal_table_sync(&filter, limit) {
                Ok(rows) => {
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    let total = rows.len();
                    let limited = total == limit;
                    tracing::info!(
                        target: "sim_flow::metrics",
                        event = "retrieval_call",
                        tool = "signal_table_query",
                        mode = "rows",
                        elapsed_ms = elapsed_ms,
                        k_requested = limit,
                        k_returned = total,
                    );
                    let rows_json: Vec<Value> = rows.iter().map(row_to_json).collect();
                    json!({
                        "rows": rows_json,
                        "total_matching": total,
                        "limited": limited,
                        "conflict_pairs": [],
                        "embedder_used": self.service.embedder_label(),
                        "elapsed_ms": elapsed_ms,
                    })
                }
                Err(RetrievalError::IndexMissing { which }) => {
                    return Ok(ToolResult::err(format!(
                        "signal_table_query: {which} index not built; run `sim-flow build-spec-index`"
                    )));
                }
                Err(e) => {
                    return Ok(ToolResult::err(format!(
                        "signal_table_query: {}",
                        render_retrieval_error(&e)
                    )));
                }
            }
        };

        Ok(ToolResult::ok(payload.to_string()))
    }
}

fn read_string(obj: &Value, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn render_retrieval_error(e: &RetrievalError) -> String {
    match e {
        RetrievalError::IndexMissing { which } => format!("{which} index not built"),
        RetrievalError::Embed(err) => format!("embedder error: {err}"),
        RetrievalError::Query(err) => format!("lance query error: {err}"),
    }
}

fn row_to_json(row: &crate::__internal::session::lance_index::query::SignalRow) -> Value {
    json!({
        "row_id": row.row_id,
        "source_kind": row.source_kind,
        "source_id": row.source_id,
        "chunk_id": row.chunk_id,
        "stage": row.stage,
        "signal_name": row.signal_name,
        "direction": row.direction,
        "peer": row.peer,
        "description": row.description,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::__internal::session::embedder::{EmbedError, EmbeddingClient};
    use async_trait::async_trait;

    struct MockEmbedder;

    #[async_trait]
    impl EmbeddingClient for MockEmbedder {
        fn provider(&self) -> &str {
            "mock"
        }
        fn model_id(&self) -> &str {
            "mock-embed"
        }
        fn dimension(&self) -> usize {
            8
        }
        async fn embed(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
            Ok(texts.iter().map(|_| vec![0.0; 8]).collect())
        }
    }

    fn make_service() -> (tempfile::TempDir, Arc<RetrievalService>) {
        let tmp = tempfile::tempdir().unwrap();
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder);
        let service = Arc::new(RetrievalService::new(tmp.path(), embedder).expect("construct"));
        (tmp, service)
    }

    #[test]
    fn missing_filter_is_rejected() {
        let (tmp, service) = make_service();
        let tool = SignalTableQueryTool::new(service);
        let ctx = ToolContext::new(tmp.path(), None, None, None);
        let r = tool.invoke(&ctx, &json!({})).expect("invoke");
        assert!(!r.ok);
        assert!(r.display.contains("missing"));
    }

    #[test]
    fn unknown_filter_key_is_rejected() {
        let (tmp, service) = make_service();
        let tool = SignalTableQueryTool::new(service);
        let ctx = ToolContext::new(tmp.path(), None, None, None);
        let r = tool
            .invoke(
                &ctx,
                &json!({"filter": {"signal_name": "x", "bogus_field": "y"}}),
            )
            .expect("invoke");
        assert!(!r.ok);
        assert!(r.display.contains("bogus_field"));
    }

    #[test]
    fn returns_empty_with_note_when_no_spec_index() {
        let (tmp, service) = make_service();
        let tool = SignalTableQueryTool::new(service);
        let ctx = ToolContext::new(tmp.path(), None, None, None);
        let r = tool
            .invoke(&ctx, &json!({"filter": {"signal_name": "pc"}}))
            .expect("invoke");
        assert!(r.ok);
        let v: Value = serde_json::from_str(&r.display).expect("json");
        assert_eq!(v["rows"].as_array().unwrap().len(), 0);
        assert_eq!(v["note"], "no signal tables registered for this project");
    }

    #[test]
    fn conflicts_only_returns_empty_when_no_spec_index() {
        let (tmp, service) = make_service();
        let tool = SignalTableQueryTool::new(service);
        let ctx = ToolContext::new(tmp.path(), None, None, None);
        let r = tool
            .invoke(&ctx, &json!({"filter": {}, "conflicts_only": true}))
            .expect("invoke");
        assert!(r.ok);
        let v: Value = serde_json::from_str(&r.display).expect("json");
        assert_eq!(v["rows"].as_array().unwrap().len(), 0);
    }
}
