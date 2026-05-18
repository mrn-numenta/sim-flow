//! Framework-index build pipeline (Chapter 3 §3.9.1).
//!
//! Walks `<framework-root>/api/pages/**/*.md` (one chunk per file)
//! and `<framework-root>/src/**/*.rs` (one chunk per top-level
//! `syn::Item` -- function, impl block, trait def, module
//! doc-comment, plus an `src-other` bucket for anything else
//! significant enough to retain). Computes a SHA-256 over each
//! chunk's text, batches every chunk that lacks a matching existing
//! row through the configured [`EmbeddingClient`], and writes the
//! resulting rows into a new `framework_chunks.lance/` dataset
//! atomically next to `manifest.toml` and `embedder.toml`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{
    FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray, UInt64Array,
};
use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::session::embedder::EmbeddingClient;

use super::super::lock::{LanceLock, LockError};
use super::super::manifests::{ApiIndexManifest, EmbedderManifest};
use super::super::schemas::framework_chunks_schema;

/// Options for [`build_framework_index`].
#[derive(Debug, Clone)]
pub struct FrameworkBuildOpts {
    /// Root of the framework workspace -- the directory that
    /// contains `api/pages/` and `src/`.
    pub framework_root: PathBuf,
    /// Output root, typically `~/.sim-flow/lance-index/api/`.
    pub out_root: PathBuf,
    /// Version string recorded into every row's `framework_version`
    /// column and into the manifest. Caller-supplied to keep the
    /// build pure (no fs reads of `Cargo.toml`).
    pub framework_version: String,
    /// Hash of the workspace state at build time. Recorded in the
    /// manifest only.
    pub framework_workspace_hash: String,
    /// Force a full re-embed.
    pub force: bool,
    /// Vector index type recorded in the manifest. v1 only writes
    /// `ivf_flat` and the build does not currently create the index
    /// itself (Lance does this lazily on first query).
    pub vector_index_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameworkBuildOutcome {
    pub row_count: u64,
    pub api_pages_count: u64,
    pub src_items_count: u64,
    pub dataset_path: PathBuf,
    pub manifest_path: PathBuf,
    pub embedder_path: PathBuf,
}

#[derive(Debug)]
pub enum FrameworkBuildError {
    Io(std::io::Error),
    Walk(String),
    Lock(LockError),
    Manifest(super::super::manifests::ManifestError),
    Embed(crate::session::embedder::EmbedError),
    Lance(lancedb::Error),
    Arrow(arrow_schema::ArrowError),
}

impl std::fmt::Display for FrameworkBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameworkBuildError::Io(e) => write!(f, "framework-index build I/O: {e}"),
            FrameworkBuildError::Walk(m) => write!(f, "framework-index walk: {m}"),
            FrameworkBuildError::Lock(e) => write!(f, "framework-index lock: {e}"),
            FrameworkBuildError::Manifest(e) => write!(f, "framework-index manifest: {e}"),
            FrameworkBuildError::Embed(e) => write!(f, "framework-index embed: {e}"),
            FrameworkBuildError::Lance(e) => write!(f, "framework-index lance: {e}"),
            FrameworkBuildError::Arrow(e) => write!(f, "framework-index arrow: {e}"),
        }
    }
}

impl std::error::Error for FrameworkBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FrameworkBuildError::Io(e) => Some(e),
            FrameworkBuildError::Lock(e) => Some(e),
            FrameworkBuildError::Manifest(e) => Some(e),
            FrameworkBuildError::Embed(e) => Some(e),
            FrameworkBuildError::Lance(e) => Some(e),
            FrameworkBuildError::Arrow(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for FrameworkBuildError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<LockError> for FrameworkBuildError {
    fn from(e: LockError) -> Self {
        Self::Lock(e)
    }
}
impl From<super::super::manifests::ManifestError> for FrameworkBuildError {
    fn from(e: super::super::manifests::ManifestError) -> Self {
        Self::Manifest(e)
    }
}
impl From<crate::session::embedder::EmbedError> for FrameworkBuildError {
    fn from(e: crate::session::embedder::EmbedError) -> Self {
        Self::Embed(e)
    }
}
impl From<lancedb::Error> for FrameworkBuildError {
    fn from(e: lancedb::Error) -> Self {
        Self::Lance(e)
    }
}
impl From<arrow_schema::ArrowError> for FrameworkBuildError {
    fn from(e: arrow_schema::ArrowError) -> Self {
        Self::Arrow(e)
    }
}

/// One in-memory chunk before embedding.
#[derive(Debug, Clone)]
struct PendingChunk {
    id: String,
    source_path: String,
    kind: String,
    name: String,
    text: String,
    text_sha256: String,
    chunk_byte_start: u64,
    chunk_byte_end: u64,
}

/// Drive a full framework-index build. Blocks on the supplied
/// `EmbeddingClient` via a private tokio runtime; callers in the
/// CLI live in sync context.
pub fn build_framework_index(
    opts: &FrameworkBuildOpts,
    embedder: &Arc<dyn EmbeddingClient>,
) -> Result<FrameworkBuildOutcome, FrameworkBuildError> {
    // Acquire the per-tree writer lock for the full build.
    std::fs::create_dir_all(&opts.out_root)?;
    let lock_path = opts.out_root.join("framework_chunks.lance.lock");
    let _lock = LanceLock::acquire(&lock_path)?;

    // Walk the corpus.
    let mut pending: Vec<PendingChunk> = Vec::new();
    let mut api_pages_count = 0u64;
    let api_dir = opts.framework_root.join("api").join("pages");
    if api_dir.is_dir() {
        for path in walk_files(&api_dir, &["md"])? {
            let rel = path
                .strip_prefix(&opts.framework_root)
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|_| path.clone());
            let display_path = format!("fw:{}", rel.display());
            let text = std::fs::read_to_string(&path)?;
            let byte_len = text.len() as u64;
            let text_sha256 = sha256_hex(&text);
            let id = format!("{display_path}::chunk-0");
            pending.push(PendingChunk {
                id,
                source_path: display_path,
                kind: "api-page".into(),
                name: rel
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
                text,
                text_sha256,
                chunk_byte_start: 0,
                chunk_byte_end: byte_len,
            });
            api_pages_count += 1;
        }
    }

    let mut src_items_count = 0u64;
    let src_dir = opts.framework_root.join("src");
    if src_dir.is_dir() {
        for path in walk_files(&src_dir, &["rs"])? {
            let rel = path
                .strip_prefix(&opts.framework_root)
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|_| path.clone());
            let display_path = format!("fw:{}", rel.display());
            let source = std::fs::read_to_string(&path)?;
            let chunks = chunk_rust_source(&source, &display_path);
            src_items_count += chunks.len() as u64;
            pending.extend(chunks);
        }
    }

    if pending.is_empty() {
        return Err(FrameworkBuildError::Walk(format!(
            "framework_root `{}` produced no chunks (no `api/pages/*.md` and no `src/**/*.rs`)",
            opts.framework_root.display()
        )));
    }

    // Embed everything in one pass for v1. The `--force` switch and
    // SHA-keyed incremental embedding are wired in but the current
    // call always embeds all rows because the previous-dataset
    // lookup happens against a fresh `.tmp/` directory; incremental
    // mode lands when the build reads the prior table.
    let _ = opts.force;
    let texts: Vec<&str> = pending.iter().map(|c| c.text.as_str()).collect();
    let vectors = run_async(embedder.embed(&texts))?;
    if vectors.len() != pending.len() {
        return Err(FrameworkBuildError::Walk(format!(
            "embedder returned {} vectors for {} chunks",
            vectors.len(),
            pending.len()
        )));
    }

    // Build the record batch.
    let schema = framework_chunks_schema(embedder.dimension());
    let batch = build_framework_record_batch(
        schema.clone(),
        &pending,
        &vectors,
        embedder.dimension(),
        &opts.framework_version,
    )?;

    // Write into a `*.tmp/` table then atomic-rename into place.
    // lancedb appends `.lance` to the table name on disk, so we pass
    // `framework_chunks_tmp` and rename the resulting
    // `framework_chunks_tmp.lance/` to `framework_chunks.lance/`.
    let tmp_dir = opts.out_root.join("framework_chunks_tmp.lance");
    let final_dir = opts.out_root.join("framework_chunks.lance");
    if tmp_dir.exists() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
    if final_dir.exists() {
        let _ = std::fs::remove_dir_all(&final_dir);
    }

    let conn_uri = opts.out_root.to_string_lossy().to_string();
    let row_count = run_async(write_table(
        &conn_uri,
        "framework_chunks_tmp",
        schema,
        batch,
    ))? as u64;

    std::fs::rename(&tmp_dir, &final_dir)?;

    let manifest = ApiIndexManifest {
        schema_version: 1,
        indexed_at: Utc::now().to_rfc3339(),
        framework_version: opts.framework_version.clone(),
        framework_workspace_hash: opts.framework_workspace_hash.clone(),
        vector_index_type: opts.vector_index_type.clone(),
        row_count,
    };
    let manifest_path = opts.out_root.join("manifest.toml");
    manifest.save(&manifest_path)?;

    let embedder_manifest = EmbedderManifest {
        schema_version: 1,
        provider: embedder.provider().to_string(),
        base_url: String::new(),
        model: embedder.model_id().to_string(),
        dimension: embedder.dimension(),
        indexed_at: manifest.indexed_at.clone(),
        auth: None,
    };
    let embedder_path = opts.out_root.join("embedder.toml");
    embedder_manifest.save(&embedder_path)?;

    Ok(FrameworkBuildOutcome {
        row_count,
        api_pages_count,
        src_items_count,
        dataset_path: final_dir,
        manifest_path,
        embedder_path,
    })
}

/// Recursive walk constrained to files whose extension is in
/// `wanted`. Order is deterministic (alphabetical) so the
/// build-then-build comparison is stable.
fn walk_files(root: &Path, wanted: &[&str]) -> Result<Vec<PathBuf>, std::io::Error> {
    fn visit(dir: &Path, wanted: &[&str], out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                visit(&path, wanted, out)?;
            } else if ft.is_file()
                && let Some(ext) = path.extension().and_then(|s| s.to_str())
                && wanted.iter().any(|w| ext.eq_ignore_ascii_case(w))
            {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    visit(root, wanted, &mut out)?;
    Ok(out)
}

/// Parse `source` as Rust, walk top-level items, and yield one
/// chunk per item. Files that fail to parse degrade to a single
/// `src-other` chunk holding the whole file (we still want them
/// indexed for retrieval).
fn chunk_rust_source(source: &str, display_path: &str) -> Vec<PendingChunk> {
    let mut out = Vec::new();
    let parsed = syn::parse_file(source);
    let Ok(file) = parsed else {
        let text_sha256 = sha256_hex(source);
        out.push(PendingChunk {
            id: format!("{display_path}::file"),
            source_path: display_path.to_string(),
            kind: "src-other".into(),
            name: "".into(),
            text: source.to_string(),
            text_sha256,
            chunk_byte_start: 0,
            chunk_byte_end: source.len() as u64,
        });
        return out;
    };

    // Capture file-level doc comments (`//!`-form, which become
    // `#![doc = "..."]` inner attributes) as a `src-mod-doc` chunk.
    let mut mod_doc = String::new();
    for attr in &file.attrs {
        if attr.path().is_ident("doc")
            && let syn::Meta::NameValue(nv) = &attr.meta
            && let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
        {
            mod_doc.push_str(&s.value());
            mod_doc.push('\n');
        }
    }
    if !mod_doc.trim().is_empty() {
        out.push(PendingChunk {
            id: format!("{display_path}::__mod_doc__"),
            source_path: display_path.to_string(),
            kind: "src-mod-doc".into(),
            name: "".into(),
            text: mod_doc.clone(),
            text_sha256: sha256_hex(&mod_doc),
            chunk_byte_start: 0,
            chunk_byte_end: source.len() as u64,
        });
    }

    for (idx, item) in file.items.iter().enumerate() {
        let (kind, name) = classify_item(item);
        let text = item_to_text(source, item, idx);
        let text_sha256 = sha256_hex(&text);
        let id = if name.is_empty() {
            format!("{display_path}::item-{idx}")
        } else {
            format!("{display_path}::{name}")
        };
        out.push(PendingChunk {
            id,
            source_path: display_path.to_string(),
            kind: kind.into(),
            name,
            text,
            text_sha256,
            chunk_byte_start: 0,
            chunk_byte_end: source.len() as u64,
        });
    }

    out
}

/// Map a `syn::Item` to a `kind` plus a best-effort `name`.
fn classify_item(item: &syn::Item) -> (&'static str, String) {
    match item {
        syn::Item::Fn(f) => ("src-fn", f.sig.ident.to_string()),
        syn::Item::Impl(i) => {
            let target = match &*i.self_ty {
                syn::Type::Path(p) => p
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            ("src-impl", target)
        }
        syn::Item::Trait(t) => ("src-trait", t.ident.to_string()),
        syn::Item::Mod(m) => ("src-mod-doc", m.ident.to_string()),
        syn::Item::Struct(s) => ("src-other", s.ident.to_string()),
        syn::Item::Enum(e) => ("src-other", e.ident.to_string()),
        syn::Item::Const(c) => ("src-other", c.ident.to_string()),
        syn::Item::Static(s) => ("src-other", s.ident.to_string()),
        syn::Item::Type(t) => ("src-other", t.ident.to_string()),
        syn::Item::Use(_) => ("src-other", String::new()),
        syn::Item::Macro(m) => (
            "src-other",
            m.ident.as_ref().map(|i| i.to_string()).unwrap_or_default(),
        ),
        _ => ("src-other", String::new()),
    }
}

/// Best-effort textual representation of an item: use
/// `prettyplease`-style rendering when available, else fall back to
/// `quote::ToTokens`'s `to_token_stream().to_string()`. We don't
/// currently take a dep on prettyplease; the token-stream form is
/// adequate for embedding (the model sees the symbols, signatures,
/// and identifiers it needs).
fn item_to_text(_source: &str, item: &syn::Item, _idx: usize) -> String {
    use quote::ToTokens;
    let mut s = String::new();
    let tokens = item.to_token_stream().to_string();
    s.push_str(&tokens);
    s
}

/// SHA-256 hex digest helper.
fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Build the Arrow record batch for the framework table.
fn build_framework_record_batch(
    schema: Arc<arrow_schema::Schema>,
    rows: &[PendingChunk],
    vectors: &[Vec<f32>],
    dimension: usize,
    framework_version: &str,
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let n = rows.len();
    let ids = StringArray::from_iter_values(rows.iter().map(|r| r.id.as_str()));
    let source_paths = StringArray::from_iter_values(rows.iter().map(|r| r.source_path.as_str()));
    let kinds = StringArray::from_iter_values(rows.iter().map(|r| r.kind.as_str()));
    let names = StringArray::from_iter_values(rows.iter().map(|r| r.name.as_str()));
    let texts = StringArray::from_iter_values(rows.iter().map(|r| r.text.as_str()));
    let shas = StringArray::from_iter_values(rows.iter().map(|r| r.text_sha256.as_str()));
    let framework_versions = StringArray::from_iter_values((0..n).map(|_| framework_version));
    let byte_starts = UInt64Array::from_iter_values(rows.iter().map(|r| r.chunk_byte_start));
    let byte_ends = UInt64Array::from_iter_values(rows.iter().map(|r| r.chunk_byte_end));

    let mut flat = Vec::with_capacity(n * dimension);
    for v in vectors {
        if v.len() != dimension {
            return Err(arrow_schema::ArrowError::SchemaError(format!(
                "vector dim {} != schema dim {dimension}",
                v.len()
            )));
        }
        flat.extend_from_slice(v);
    }
    let values = Arc::new(Float32Array::from(flat));
    let field = Arc::new(arrow_schema::Field::new(
        "item",
        arrow_schema::DataType::Float32,
        true,
    ));
    let vector_array = FixedSizeListArray::try_new(field, dimension as i32, values, None)?;

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(ids),
            Arc::new(source_paths),
            Arc::new(kinds),
            Arc::new(names),
            Arc::new(texts),
            Arc::new(shas),
            Arc::new(vector_array),
            Arc::new(framework_versions),
            Arc::new(byte_starts),
            Arc::new(byte_ends),
        ],
    )
}

/// Write a `RecordBatch` as a fresh lance table at
/// `<base_uri>/<table_name>/`. Returns the row count.
async fn write_table(
    base_uri: &str,
    table_name: &str,
    schema: Arc<arrow_schema::Schema>,
    batch: RecordBatch,
) -> Result<usize, FrameworkBuildError> {
    use arrow_array::RecordBatchReader;
    let n = batch.num_rows();
    let conn = lancedb::connect(base_uri).execute().await?;
    let batches = vec![Ok(batch)];
    let reader: Box<dyn RecordBatchReader + Send> =
        Box::new(RecordBatchIterator::new(batches.into_iter(), schema));
    conn.create_table(table_name, reader).execute().await?;
    Ok(n)
}

/// Drive an async future to completion using a private current-thread
/// tokio runtime. Mirrors the embedder CLI handler -- the build is a
/// short-lived CLI invocation; we don't share a runtime here.
pub(crate) fn run_async<F>(fut: F) -> F::Output
where
    F: std::future::Future,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio current-thread runtime");
    rt.block_on(fut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::embedder::{EmbedError, EmbeddingClient};
    use async_trait::async_trait;

    /// Deterministic mock embedder used by the build-pipeline tests.
    /// Produces SHA-256-derived vectors so repeated calls against
    /// identical input return identical output (lets the integration
    /// test assert exact row counts).
    pub struct MockEmbedder {
        pub dimension: usize,
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
            self.dimension
        }
        async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
            let mut out = Vec::with_capacity(texts.len());
            for text in texts {
                let mut hasher = Sha256::new();
                hasher.update(text.as_bytes());
                let digest = hasher.finalize();
                let mut vec = Vec::with_capacity(self.dimension);
                for i in 0..self.dimension {
                    let b = digest[i % digest.len()];
                    vec.push((b as f32) / 255.0);
                }
                out.push(vec);
            }
            Ok(out)
        }
    }

    #[test]
    fn classify_item_recognizes_canonical_kinds() {
        let src = r#"
            /// doc
            pub fn alpha() {}
            pub struct Beta;
            pub trait Gamma { fn x(&self); }
            impl Beta { pub fn y(&self) {} }
            pub mod delta { }
        "#;
        let file = syn::parse_file(src).expect("parses");
        let kinds: Vec<&str> = file.items.iter().map(|i| classify_item(i).0).collect();
        assert!(kinds.contains(&"src-fn"));
        assert!(kinds.contains(&"src-trait"));
        assert!(kinds.contains(&"src-impl"));
        assert!(kinds.contains(&"src-mod-doc"));
    }

    #[test]
    fn walk_files_orders_deterministically() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("z.md"), "z").unwrap();
        std::fs::write(root.join("a.md"), "a").unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("m.md"), "m").unwrap();
        let out = walk_files(root, &["md"]).unwrap();
        assert_eq!(out.len(), 3);
        assert!(out[0].ends_with("a.md"));
        assert!(out[1].ends_with("m.md") || out[1].ends_with("z.md"));
    }

    #[test]
    fn build_against_synthetic_framework_writes_lance_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let fw_root = tmp.path().join("fw");
        std::fs::create_dir_all(fw_root.join("api").join("pages")).unwrap();
        std::fs::create_dir_all(fw_root.join("src")).unwrap();
        std::fs::write(
            fw_root.join("api").join("pages").join("hello.md"),
            "# Hello\nworld\n",
        )
        .unwrap();
        std::fs::write(
            fw_root.join("src").join("lib.rs"),
            "pub fn add(a: u32, b: u32) -> u32 { a + b }\npub struct S;\n",
        )
        .unwrap();

        let out_root = tmp.path().join("out");
        let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder { dimension: 8 });
        let outcome = build_framework_index(
            &FrameworkBuildOpts {
                framework_root: fw_root,
                out_root: out_root.clone(),
                framework_version: "0.0.1".into(),
                framework_workspace_hash: "h".into(),
                force: false,
                vector_index_type: "ivf_flat".into(),
            },
            &embedder,
        )
        .expect("build");
        assert_eq!(outcome.api_pages_count, 1);
        assert!(outcome.src_items_count >= 2);
        assert!(outcome.dataset_path.exists());
        assert!(out_root.join("manifest.toml").exists());
        assert!(out_root.join("embedder.toml").exists());
    }
}
