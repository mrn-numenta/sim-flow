//! `spec_semantic_search(query, k?, source?, kind?)` -- L2 retrieval
//! tool backed by the per-project spec lance index (Chapter 4 §4.3).
//!
//! Returns ranked chunks from the source-spec corpus, augmenting each
//! hit with the on-disk chunk path plus its `contained_signal_tables`
//! and `contained_figures` lists pulled from the chunk's frontmatter.
//! The agent uses `chunk_path` to `read_file` the full body when the
//! snippet is insufficient.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use super::api_semantic_search::snippet_for;
use super::{Tool, ToolContext, ToolResult};
use crate::__internal::session::retrieval::{RetrievalError, RetrievalService};
use crate::Result;

const DEFAULT_K: usize = 5;
const MIN_K: usize = 1;
const MAX_K: usize = 20;

pub struct SpecSemanticSearchTool {
    service: Arc<RetrievalService>,
}

impl SpecSemanticSearchTool {
    pub fn new(service: Arc<RetrievalService>) -> Self {
        Self { service }
    }
}

impl Tool for SpecSemanticSearchTool {
    fn name(&self) -> &'static str {
        "spec_semantic_search"
    }

    fn description(&self) -> &'static str {
        "Semantic retrieval over the project's source-spec corpus (ingested via `sim-flow ingest`). Use this when spec.md does not carry enough detail and you want the relevant section of the underlying material spec. Each hit returns `chunk_path` (a path the `read_file` tool can read) plus the `contained_signal_tables` / `contained_figures` lists from the chunk's frontmatter so you can pivot to structured artifacts without a second search."
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural-language description of the spec content you need."
                },
                "k": {
                    "type": "integer",
                    "minimum": MIN_K,
                    "maximum": MAX_K,
                    "default": DEFAULT_K
                },
                "source": {
                    "type": "string",
                    "description": "Optional filter: 'primary' for the project's primary spec, or a peer id from manifest.toml."
                },
                "kind": {
                    "type": "string",
                    "enum": ["prose", "table", "stub", "mixed"]
                }
            }
        })
    }

    fn invoke(&self, ctx: &ToolContext, args: &Value) -> Result<ToolResult> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim().to_string(),
            _ => {
                return Ok(ToolResult::err(
                    "spec_semantic_search: missing or empty `query` arg",
                ));
            }
        };
        let k = args
            .get("k")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_K)
            .clamp(MIN_K, MAX_K);
        let source = args
            .get("source")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let kind = args
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // The "no source spec registered" special case (Architecture
        // §4.3 failure modes): when the spec lance connection is
        // missing AND the project has no ingest manifest, return an
        // empty-hits-with-note result rather than an error. We
        // approximate "no source spec" by the absence of
        // `.sim-flow/spec-ingest/manifest.toml`.
        if !self.service.has_spec() && !has_ingest_manifest(ctx.project_dir) {
            let payload = json!({
                "hits": [],
                "note": "no source spec registered",
                "embedder_used": self.service.embedder_label(),
                "elapsed_ms": 0,
            });
            return Ok(ToolResult::ok(payload.to_string()));
        }

        if !self.service.has_spec() {
            return Ok(ToolResult::err(
                "spec_semantic_search: spec index not built; run `sim-flow build-spec-index`",
            ));
        }

        let start = std::time::Instant::now();
        let vector = match self.service.embed_one_sync(&query) {
            Ok(v) => v,
            Err(e) => {
                return Ok(ToolResult::err(format!(
                    "spec_semantic_search: {}",
                    render_retrieval_error(&e)
                )));
            }
        };

        let hits = match self.service.semantic_search_spec_sync(
            &vector,
            k,
            source.as_deref(),
            kind.as_deref(),
        ) {
            Ok(h) => h,
            Err(RetrievalError::IndexMissing { which }) => {
                return Ok(ToolResult::err(format!(
                    "spec_semantic_search: {which} index not built; run `sim-flow build-spec-index`"
                )));
            }
            Err(e) => {
                return Ok(ToolResult::err(format!(
                    "spec_semantic_search: {}",
                    render_retrieval_error(&e)
                )));
            }
        };

        let elapsed_ms = start.elapsed().as_millis() as u64;
        let k_returned = hits.len();

        tracing::info!(
            target: "sim_flow::metrics",
            event = "retrieval_call",
            tool = "spec_semantic_search",
            elapsed_ms = elapsed_ms,
            k_requested = k,
            k_returned = k_returned,
        );

        // Build the chunk-id -> chunk-file map for every source we
        // actually returned a hit from. Cached by source so we don't
        // re-scan the directory per hit.
        let mut chunk_maps: HashMap<String, HashMap<String, ChunkFrontmatter>> = HashMap::new();
        for h in &hits {
            chunk_maps.entry(h.source_id.clone()).or_insert_with(|| {
                load_chunk_frontmatter_for_source(ctx.project_dir, &h.source_id)
            });
        }

        let augmented: Vec<Value> = hits
            .iter()
            .map(|h| {
                let map = chunk_maps.get(&h.source_id);
                let fm = map.and_then(|m| m.get(&h.id));
                let chunk_path_rel = fm
                    .and_then(|f| f.path.as_ref())
                    .map(|p| project_relative(ctx.project_dir, p))
                    .unwrap_or_default();
                json!({
                    "chunk_id": h.id,
                    "source_id": h.source_id,
                    "section_heading": h.section_heading,
                    "snippet": snippet_for(&h.text),
                    "chunk_path": chunk_path_rel,
                    "contained_signal_tables": fm.map(|f| f.contained_signal_tables.clone()).unwrap_or_default(),
                    "contained_figures": fm.map(|f| f.contained_figures.clone()).unwrap_or_default(),
                    "breadcrumb": fm.map(|f| f.breadcrumb.clone()).unwrap_or_default(),
                    "source_page_range": fm
                        .map(|f| json!([f.source_page_start, f.source_page_end]))
                        .unwrap_or(json!([0, 0])),
                    "score": h.distance,
                })
            })
            .collect();

        let payload = json!({
            "hits": augmented,
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

fn has_ingest_manifest(project_dir: &Path) -> bool {
    project_dir
        .join(".sim-flow")
        .join("spec-ingest")
        .join("manifest.toml")
        .is_file()
}

/// Lightweight frontmatter shape: only the fields the tool surfaces.
/// Mirrors the structure that Phase 2's emit stage writes and
/// Phase 4's `lance_index::build::spec` decodes.
#[derive(Debug, Default, Clone)]
struct ChunkFrontmatter {
    chunk_id: String,
    breadcrumb: Vec<String>,
    source_page_start: u32,
    source_page_end: u32,
    contained_signal_tables: Vec<String>,
    contained_figures: Vec<String>,
    /// Populated by the loader, not the parser.
    path: Option<PathBuf>,
}

/// Scan a source's chunks directory once and build a chunk_id ->
/// frontmatter map. Returns an empty map if the directory doesn't
/// exist.
fn load_chunk_frontmatter_for_source(
    project_dir: &Path,
    source_id: &str,
) -> HashMap<String, ChunkFrontmatter> {
    let dir = project_dir
        .join(".sim-flow")
        .join("spec-ingest")
        .join(source_id)
        .join("chunks");
    let Ok(read) = std::fs::read_dir(&dir) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for entry in read.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Some(frontmatter) = extract_frontmatter(&body) else {
            continue;
        };
        let Ok(mut fm) = parse_chunk_frontmatter(frontmatter) else {
            continue;
        };
        fm.path = Some(p.clone());
        if !fm.chunk_id.is_empty() {
            out.insert(fm.chunk_id.clone(), fm);
        }
    }
    out
}

/// Minimal YAML-ish parser for the strict subset Phase 2's emit stage
/// writes (scalar strings, lists of strings as bullets or inline
/// arrays). Mirrors `lance_index::build::spec::serde_yaml_compat_parse`
/// but lives here so the tool stays free of cross-module coupling.
fn parse_chunk_frontmatter(input: &str) -> std::result::Result<ChunkFrontmatter, String> {
    let mut out = ChunkFrontmatter::default();
    let mut current_list: Option<String> = None;
    for raw_line in input.lines() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("- ") {
            if let Some(key) = current_list.as_deref() {
                let item = strip_quotes(rest.trim()).to_string();
                match key {
                    "breadcrumb" => out.breadcrumb.push(item),
                    "contained_signal_tables" => out.contained_signal_tables.push(item),
                    "contained_figures" => out.contained_figures.push(item),
                    _ => {}
                }
                continue;
            } else {
                return Err(format!("bullet `{rest}` with no active list key"));
            }
        }
        current_list = None;
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("expected `key: value`, got `{line}`"));
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "chunk_id" => out.chunk_id = strip_quotes(value).to_string(),
            "source_page_start" => {
                if let Ok(n) = value.parse() {
                    out.source_page_start = n;
                }
            }
            "source_page_end" => {
                if let Ok(n) = value.parse() {
                    out.source_page_end = n;
                }
            }
            "breadcrumb" => {
                current_list = Some("breadcrumb".into());
                if !value.is_empty() {
                    out.breadcrumb = split_inline_array(value);
                    current_list = None;
                }
            }
            "contained_signal_tables" => {
                current_list = Some("contained_signal_tables".into());
                if !value.is_empty() {
                    out.contained_signal_tables = split_inline_array(value);
                    current_list = None;
                }
            }
            "contained_figures" => {
                current_list = Some("contained_figures".into());
                if !value.is_empty() {
                    out.contained_figures = split_inline_array(value);
                    current_list = None;
                }
            }
            _ => {} // tolerate other fields (section_heading, kind, etc.)
        }
    }
    Ok(out)
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    s.trim_start_matches('"').trim_end_matches('"')
}

fn split_inline_array(value: &str) -> Vec<String> {
    let trimmed = value.trim().trim_start_matches('[').trim_end_matches(']');
    if trimmed.trim().is_empty() {
        return Vec::new();
    }
    trimmed
        .split(',')
        .map(|s| strip_quotes(s).to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Pull the YAML block between the first pair of `---` delimiters.
fn extract_frontmatter(body: &str) -> Option<&str> {
    let body = body.strip_prefix("---\n")?;
    let end = body.find("\n---")?;
    Some(&body[..end])
}

fn project_relative(project_dir: &Path, abs: &Path) -> String {
    abs.strip_prefix(project_dir)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| abs.to_string_lossy().into_owned())
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

    fn make_service_no_index() -> (tempfile::TempDir, Arc<RetrievalService>) {
        let tmp = tempfile::tempdir().unwrap();
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dim: 8 });
        let service = Arc::new(RetrievalService::new(tmp.path(), embedder).expect("construct"));
        (tmp, service)
    }

    #[test]
    fn returns_no_source_note_when_no_ingest_manifest_and_no_index() {
        let (tmp, service) = make_service_no_index();
        let tool = SpecSemanticSearchTool::new(service);
        let ctx = ToolContext::new(tmp.path(), None, None, None);
        let r = tool
            .invoke(&ctx, &json!({"query": "anything"}))
            .expect("invoke");
        assert!(r.ok, "display = {}", r.display);
        let v: Value = serde_json::from_str(&r.display).expect("json");
        assert_eq!(v["note"], "no source spec registered");
        assert_eq!(v["hits"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn errors_when_ingest_manifest_present_but_no_lance_index() {
        let (tmp, service) = make_service_no_index();
        // Lay down an ingest manifest so the "no source spec" branch
        // does not trigger.
        let ingest = tmp.path().join(".sim-flow").join("spec-ingest");
        std::fs::create_dir_all(&ingest).unwrap();
        std::fs::write(ingest.join("manifest.toml"), "schema_version = 1\n").unwrap();

        let tool = SpecSemanticSearchTool::new(service);
        let ctx = ToolContext::new(tmp.path(), None, None, None);
        let r = tool
            .invoke(&ctx, &json!({"query": "anything"}))
            .expect("invoke");
        assert!(!r.ok);
        assert!(
            r.display.contains("spec index not built"),
            "display = {}",
            r.display
        );
    }

    #[test]
    fn missing_query_arg_is_rejected() {
        let (tmp, service) = make_service_no_index();
        let tool = SpecSemanticSearchTool::new(service);
        let ctx = ToolContext::new(tmp.path(), None, None, None);
        let r = tool.invoke(&ctx, &json!({})).expect("invoke");
        assert!(!r.ok);
        assert!(r.display.contains("missing"));
    }

    #[test]
    fn extract_frontmatter_parses_well_formed_block() {
        let body = "---\nchunk_id: \"abc\"\nbreadcrumb: [\"A\"]\n---\nbody text\n";
        let fm = extract_frontmatter(body).expect("frontmatter present");
        assert!(fm.contains("chunk_id"));
        assert!(!fm.contains("body text"));
    }

    #[test]
    fn extract_frontmatter_returns_none_when_no_delimiters() {
        assert!(extract_frontmatter("# heading\nno frontmatter").is_none());
    }

    #[test]
    fn load_chunk_frontmatter_picks_up_chunk_id() {
        let tmp = tempfile::tempdir().unwrap();
        let chunks = tmp
            .path()
            .join(".sim-flow")
            .join("spec-ingest")
            .join("primary")
            .join("chunks");
        std::fs::create_dir_all(&chunks).unwrap();
        std::fs::write(
            chunks.join("000-intro.md"),
            "---\nchunk_id: \"abc123\"\nbreadcrumb: [\"Intro\"]\nsource_page_start: 1\nsource_page_end: 2\nkind: \"prose\"\ncontained_signal_tables: [\"tables/signals/if.toml\"]\ncontained_figures: []\n---\nbody text\n",
        )
        .unwrap();
        let map = load_chunk_frontmatter_for_source(tmp.path(), "primary");
        assert!(map.contains_key("abc123"));
        let entry = &map["abc123"];
        assert_eq!(entry.breadcrumb, vec!["Intro".to_string()]);
        assert_eq!(
            entry.contained_signal_tables,
            vec!["tables/signals/if.toml".to_string()]
        );
    }
}
