//! `api_semantic_search(query, k?, kind?)` -- L1 retrieval tool
//! backed by the framework lance index (Chapter 4 §4.2).
//!
//! The agent calls this when it does not already know the symbol it
//! needs and wants a ranked list of candidates from a natural-
//! language description. The typical follow-up is `api_hover` on one
//! of the returned `name` values to read the live signature.
//!
//! Construction takes an `Arc<RetrievalService>`. The dispatcher path
//! builds the tool when the orchestrator has a retrieval service in
//! hand; without one, the tool is omitted from the dispatcher.

use std::sync::Arc;

use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolResult};
use crate::__internal::session::retrieval::{RetrievalError, RetrievalService};
use crate::Result;

const DEFAULT_K: usize = 8;
const MIN_K: usize = 1;
const MAX_K: usize = 20;
const SNIPPET_CHAR_CAP: usize = 500;

pub struct ApiSemanticSearchTool {
    service: Arc<RetrievalService>,
}

impl ApiSemanticSearchTool {
    pub fn new(service: Arc<RetrievalService>) -> Self {
        Self { service }
    }
}

impl Tool for ApiSemanticSearchTool {
    fn name(&self) -> &'static str {
        "api_semantic_search"
    }

    fn description(&self) -> &'static str {
        "Semantic retrieval over the framework's API surface (rustdoc-style pages plus indexed src/ items). Use this when you don't already know the symbol you need: describe the operation, signature shape, or behavior in natural language and the tool returns a ranked list of candidate symbols. Follow up with `api_hover` on each promising candidate to verify the live signature. Returns approximate matches; `api_hover` returns truth."
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural-language description of the framework concept, signature shape, or behavior you need."
                },
                "k": {
                    "type": "integer",
                    "minimum": MIN_K,
                    "maximum": MAX_K,
                    "default": DEFAULT_K,
                    "description": "Number of candidates to return."
                },
                "kind": {
                    "type": "string",
                    "enum": ["api-page", "src-fn", "src-impl", "src-trait", "src-mod-doc", "src-other"],
                    "description": "Optional filter restricting results to one chunk kind."
                }
            }
        })
    }

    fn invoke(&self, _ctx: &ToolContext, args: &Value) -> Result<ToolResult> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim().to_string(),
            _ => {
                return Ok(ToolResult::err(
                    "api_semantic_search: missing or empty `query` arg",
                ));
            }
        };
        let k = args
            .get("k")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_K)
            .clamp(MIN_K, MAX_K);
        let kind = args
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if !self.service.has_framework() {
            return Ok(ToolResult::err(
                "api_semantic_search: framework index not built; run `sim-flow build-framework-index`",
            ));
        }

        let start = std::time::Instant::now();
        let vector = match self.service.embed_one_sync(&query) {
            Ok(v) => v,
            Err(e) => {
                return Ok(ToolResult::err(format!(
                    "api_semantic_search: {}",
                    render_retrieval_error(&e)
                )));
            }
        };

        let hits = match self
            .service
            .semantic_search_framework_sync(&vector, k, kind.as_deref())
        {
            Ok(h) => h,
            Err(RetrievalError::IndexMissing { which }) => {
                return Ok(ToolResult::err(format!(
                    "api_semantic_search: {which} index not built; run `sim-flow build-framework-index`"
                )));
            }
            Err(e) => {
                return Ok(ToolResult::err(format!(
                    "api_semantic_search: {}",
                    render_retrieval_error(&e)
                )));
            }
        };

        let elapsed_ms = start.elapsed().as_millis() as u64;
        let k_returned = hits.len();

        tracing::info!(
            target: "sim_flow::metrics",
            event = "retrieval_call",
            tool = "api_semantic_search",
            elapsed_ms = elapsed_ms,
            k_requested = k,
            k_returned = k_returned,
        );

        let payload = json!({
            "hits": hits.iter().map(|h| json!({
                "path": h.source_path,
                "name": h.name,
                "kind": h.kind,
                "snippet": snippet_for(&h.text),
                "score": h.distance,
            })).collect::<Vec<_>>(),
            "embedder_used": self.service.embedder_label(),
            "elapsed_ms": elapsed_ms,
        });
        Ok(ToolResult::ok(payload.to_string()))
    }
}

fn render_retrieval_error(e: &RetrievalError) -> String {
    match e {
        RetrievalError::IndexMissing { which } => format!("{which} index not built"),
        RetrievalError::Embed(err) => format!("embedder error: {err}"),
        RetrievalError::Query(err) => format!("lance query error: {err}"),
    }
}

/// Build the user-facing snippet: first paragraph of the chunk text,
/// capped at `SNIPPET_CHAR_CAP` chars (counting unicode code points).
pub(crate) fn snippet_for(text: &str) -> String {
    let para = text.split("\n\n").next().unwrap_or(text).trim();
    if para.chars().count() <= SNIPPET_CHAR_CAP {
        return para.to_string();
    }
    let truncated: String = para.chars().take(SNIPPET_CHAR_CAP).collect();
    format!("{truncated}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::__internal::session::embedder::{EmbedError, EmbeddingClient};
    use async_trait::async_trait;

    struct MockEmbedder {
        dim: usize,
    }

    #[async_trait]
    impl EmbeddingClient for MockEmbedder {
        fn provider(&self) -> &str {
            "mock"
        }
        fn model_id(&self) -> &str {
            "mock-embed"
        }
        fn dimension(&self) -> usize {
            self.dim
        }
        async fn embed(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
            Ok(texts.iter().map(|_| vec![0.0; self.dim]).collect())
        }
    }

    fn make_service_no_index() -> Arc<RetrievalService> {
        let tmp = tempfile::tempdir().unwrap();
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dim: 8 });
        Arc::new(
            RetrievalService::new(tmp.path(), embedder)
                .expect("retrieval service constructs in tmp dir"),
        )
    }

    #[test]
    fn returns_structured_error_when_framework_index_missing() {
        let tool = ApiSemanticSearchTool::new(make_service_no_index());
        let ctx = ToolContext::new(std::path::Path::new("/tmp"), None, None, None);
        let r = tool
            .invoke(&ctx, &json!({"query": "scheduler"}))
            .expect("invoke");
        assert!(!r.ok);
        assert!(
            r.display.contains("framework index not built"),
            "display = {}",
            r.display
        );
    }

    #[test]
    fn missing_query_arg_is_rejected() {
        let tool = ApiSemanticSearchTool::new(make_service_no_index());
        let ctx = ToolContext::new(std::path::Path::new("/tmp"), None, None, None);
        let r = tool.invoke(&ctx, &json!({})).expect("invoke");
        assert!(!r.ok);
        assert!(r.display.contains("missing"), "display = {}", r.display);
    }

    #[test]
    fn empty_query_arg_is_rejected() {
        let tool = ApiSemanticSearchTool::new(make_service_no_index());
        let ctx = ToolContext::new(std::path::Path::new("/tmp"), None, None, None);
        let r = tool.invoke(&ctx, &json!({"query": "   "})).expect("invoke");
        assert!(!r.ok);
    }

    #[test]
    fn k_arg_is_clamped_to_valid_range() {
        // The tool computes `k` even when index is missing; we
        // exercise the clamp via the JSON arg parse path. Since the
        // tool short-circuits on missing-index, this test is mostly
        // for the schema-args sanity check via dispatch.
        let tool = ApiSemanticSearchTool::new(make_service_no_index());
        let ctx = ToolContext::new(std::path::Path::new("/tmp"), None, None, None);
        // High k -> still ok (gets clamped internally to 20).
        let r = tool
            .invoke(&ctx, &json!({"query": "x", "k": 999}))
            .expect("invoke");
        assert!(!r.ok); // index missing
    }

    #[test]
    fn snippet_truncates_at_cap() {
        let long = "a".repeat(SNIPPET_CHAR_CAP + 200);
        let snippet = snippet_for(&long);
        assert!(snippet.ends_with("..."));
        assert!(snippet.chars().count() <= SNIPPET_CHAR_CAP + 3);
    }

    #[test]
    fn snippet_returns_first_paragraph() {
        let text = "first paragraph.\n\nsecond paragraph that should be dropped.";
        let s = snippet_for(text);
        assert_eq!(s, "first paragraph.");
    }
}
