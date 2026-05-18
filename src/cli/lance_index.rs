//! Handlers for the `sim-flow build-framework-index`,
//! `sim-flow build-spec-index`, and `sim-flow refresh-spec` CLI
//! subcommands (Chapter 3 §3.9).
//!
//! Each handler resolves the relevant paths, builds (or constructs)
//! an `OpenAiCompatEmbedder` against the configured `embedder.toml`,
//! drives the corresponding builder, and reports the resulting row
//! counts. The actual builders live in
//! `__internal::session::lance_index::build`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sim_flow::__internal::session::embedder::{
    EmbedderConfig, EmbeddingClient, OpenAiCompatEmbedder,
};
use sim_flow::__internal::session::lance_index::build::{
    FrameworkBuildOpts, SpecBuildOpts, build_framework_index, build_spec_index,
};
use sim_flow::__internal::session::lance_index::staleness::{
    SpecIndexStaleness, is_spec_index_stale,
};

/// Resolve the embedder config from an optional explicit path, or
/// fall back to the standard priority order (`<cwd>/.sim-flow/...`,
/// `$SIM_FLOW_EMBEDDER_CONFIG`, `~/.sim-flow/...`).
fn load_embedder(config_path: Option<&Path>) -> sim_flow::Result<EmbedderConfig> {
    match config_path {
        Some(p) => EmbedderConfig::load_explicit(p),
        None => EmbedderConfig::load(),
    }
    .map_err(|e| sim_flow::Error::Config(format!("embedder config: {e}")))
}

/// Build the embedder client. The constructor probes the provider
/// once to validate the dimension; errors here are usually
/// configuration / connectivity issues.
fn build_embedder(cfg: EmbedderConfig) -> sim_flow::Result<Arc<dyn EmbeddingClient>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| sim_flow::Error::State(format!("tokio runtime: {e}")))?;
    let emb = rt
        .block_on(OpenAiCompatEmbedder::new(cfg))
        .map_err(|e| sim_flow::Error::Config(format!("embedder construction: {e}")))?;
    Ok(Arc::new(emb))
}

/// Resolve the default framework index output root:
/// `$SIM_FLOW_API_INDEX_ROOT` if set; else `~/.sim-flow/lance-index/api`.
fn default_api_index_root() -> sim_flow::Result<PathBuf> {
    if let Some(p) = std::env::var_os("SIM_FLOW_API_INDEX_ROOT") {
        return Ok(PathBuf::from(p));
    }
    let home = directories::BaseDirs::new()
        .ok_or_else(|| sim_flow::Error::State("no home directory available".into()))?;
    Ok(home
        .home_dir()
        .join(".sim-flow")
        .join("lance-index")
        .join("api"))
}

/// Resolve the framework root. If `explicit` is supplied, use it
/// directly. Otherwise walk up from `cwd` to discover sim-foundation
/// and point at `<root>/crates/framework`.
fn resolve_framework_root(cwd: &Path, explicit: Option<&Path>) -> sim_flow::Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    // Walk up looking for a `crates/framework/src` directory.
    let mut current = cwd.to_path_buf();
    loop {
        let candidate = current.join("crates").join("framework");
        if candidate.join("src").is_dir() {
            return Ok(candidate);
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => break,
        }
    }
    Err(sim_flow::Error::State(format!(
        "could not discover crates/framework from cwd {}; pass --framework-root explicitly",
        cwd.display()
    )))
}

/// Hash a small subset of the framework workspace state for
/// `framework_workspace_hash`. v1 uses the framework's `Cargo.toml`
/// content as a cheap stand-in; finer-grained hashing (Cargo.lock +
/// selected source files) is a future operational concern.
fn framework_workspace_hash(framework_root: &Path) -> String {
    use sha2::{Digest, Sha256};
    let cargo = framework_root.join("Cargo.toml");
    let body = std::fs::read_to_string(&cargo).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn framework_version(framework_root: &Path) -> String {
    // Extract the `version = "..."` line from the framework's
    // Cargo.toml. If missing, fall back to "0.0.0".
    let cargo = framework_root.join("Cargo.toml");
    let body = std::fs::read_to_string(&cargo).unwrap_or_default();
    let mut in_package = false;
    for raw_line in body.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package && let Some(rest) = line.strip_prefix("version") {
            let v = rest.trim().trim_start_matches('=').trim().trim_matches('"');
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    "0.0.0".to_string()
}

/// `sim-flow build-framework-index` handler.
pub fn build_framework_index_cmd(
    project_dir: &Path,
    framework_root: Option<&Path>,
    out: Option<&Path>,
    embedder_path: Option<&Path>,
    force: bool,
) -> sim_flow::Result<()> {
    let stdout = std::io::stdout();
    let mut out_w = stdout.lock();

    let fw_root = resolve_framework_root(project_dir, framework_root)?;
    let out_root = out
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(default_api_index_root)?;

    let cfg = load_embedder(embedder_path)?;
    let _ = writeln!(
        out_w,
        "build-framework-index: framework_root={}, out={}, embedder={}/{}",
        fw_root.display(),
        out_root.display(),
        cfg.provider,
        cfg.model,
    );
    let embedder = build_embedder(cfg)?;

    let opts = FrameworkBuildOpts {
        framework_root: fw_root.clone(),
        out_root: out_root.clone(),
        framework_version: framework_version(&fw_root),
        framework_workspace_hash: framework_workspace_hash(&fw_root),
        force,
        vector_index_type: "ivf_flat".into(),
    };
    let outcome = build_framework_index(&opts, &embedder)
        .map_err(|e| sim_flow::Error::State(format!("build-framework-index: {e}")))?;
    let _ = writeln!(
        out_w,
        "build-framework-index: wrote {} row(s) ({} api-page chunk(s), {} src-item chunk(s)) -> {}",
        outcome.row_count,
        outcome.api_pages_count,
        outcome.src_items_count,
        outcome.dataset_path.display()
    );
    let _ = writeln!(
        out_w,
        "  manifest: {}\n  embedder: {}",
        outcome.manifest_path.display(),
        outcome.embedder_path.display()
    );
    Ok(())
}

/// `sim-flow build-spec-index` handler.
pub fn build_spec_index_cmd(
    project_dir: &Path,
    project_override: Option<&Path>,
    embedder_path: Option<&Path>,
    force: bool,
    check: bool,
) -> sim_flow::Result<()> {
    let stdout = std::io::stdout();
    let mut out_w = stdout.lock();

    let project_root = project_override
        .map(PathBuf::from)
        .unwrap_or_else(|| project_dir.to_path_buf());

    if check {
        // Diagnostic-only path: no embedder construction required.
        let staleness = is_spec_index_stale(&project_root, None);
        let label = match staleness {
            SpecIndexStaleness::Fresh => "Fresh",
            SpecIndexStaleness::SourceChanged => "SourceChanged",
            SpecIndexStaleness::SpecMdChanged => "SpecMdChanged",
            SpecIndexStaleness::EmbedderChanged => "EmbedderChanged",
        };
        let _ = writeln!(out_w, "build-spec-index --check: {label}");
        return Ok(());
    }

    let cfg = load_embedder(embedder_path)?;
    let _ = writeln!(
        out_w,
        "build-spec-index: project={}, embedder={}/{}",
        project_root.display(),
        cfg.provider,
        cfg.model,
    );
    let embedder = build_embedder(cfg)?;

    let opts = SpecBuildOpts {
        project_root: project_root.clone(),
        force,
    };
    let outcome = build_spec_index(&opts, &embedder)
        .map_err(|e| sim_flow::Error::State(format!("build-spec-index: {e}")))?;
    let _ = writeln!(
        out_w,
        "build-spec-index: spec_chunks={} signal_table_rows={} cross_spec_refs={}",
        outcome.spec_chunks_rows, outcome.signal_table_rows, outcome.cross_spec_refs_rows
    );
    let _ = writeln!(
        out_w,
        "  manifest: {}\n  embedder: {}",
        outcome.manifest_path.display(),
        outcome.embedder_path.display()
    );
    Ok(())
}

/// `sim-flow refresh-spec` handler. Equivalent to `sim-flow ingest
/// --rebuild` followed by `sim-flow build-spec-index`.
pub fn refresh_spec_cmd(
    project_dir: &Path,
    project_override: Option<&Path>,
) -> sim_flow::Result<()> {
    let stdout = std::io::stdout();
    let mut out_w = stdout.lock();

    let project_root = project_override
        .map(PathBuf::from)
        .unwrap_or_else(|| project_dir.to_path_buf());

    let _ = writeln!(
        out_w,
        "refresh-spec: re-running ingest --rebuild on {}",
        project_root.display()
    );

    // Run the ingest pipeline programmatically. We use the same shape
    // as `ingest_cmd`'s rebuild branch -- the source path comes from
    // the existing manifest; if it's absent, surface a clear error.
    use sim_flow::__internal::session::spec_ingest::{
        IngestConfig, IngestRequest, SourceSpec, pipeline::run as run_pipeline,
    };
    let manifest_path = project_root
        .join(".sim-flow")
        .join("spec-ingest")
        .join("manifest.toml");
    let manifest_body = std::fs::read_to_string(&manifest_path).map_err(|e| {
        sim_flow::Error::State(format!(
            "refresh-spec: read manifest {}: {e}",
            manifest_path.display()
        ))
    })?;
    let primary_path = parse_manifest_source_path(&manifest_body).ok_or_else(|| {
        sim_flow::Error::State(format!(
            "refresh-spec: source_path missing from {}",
            manifest_path.display()
        ))
    })?;
    let cfg = IngestConfig::load(&project_root)?;
    let request = IngestRequest {
        primary: Some(SourceSpec::new(primary_path)),
        peers: Vec::new(),
        config: cfg,
        project_root: project_root.clone(),
    };
    let outcome = run_pipeline(request)?;
    let _ = writeln!(
        out_w,
        "refresh-spec: ingest wrote {} (chunks={})",
        outcome.manifest_path.display(),
        outcome.primary_chunk_count
    );

    // Then run build-spec-index.
    build_spec_index_cmd(project_dir, Some(&project_root), None, false, false)
}

fn parse_manifest_source_path(body: &str) -> Option<PathBuf> {
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("source_path") {
            let v = rest.trim().trim_start_matches('=').trim();
            let v = v.trim_matches('"');
            if !v.is_empty() {
                return Some(PathBuf::from(v));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_framework_root_walks_up() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let fw = root.join("crates").join("framework");
        std::fs::create_dir_all(fw.join("src")).unwrap();
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        let resolved = resolve_framework_root(&nested, None).expect("walk up");
        assert_eq!(resolved, fw);
    }

    #[test]
    fn resolve_framework_root_uses_explicit() {
        let tmp = tempfile::tempdir().unwrap();
        let custom = tmp.path().join("custom");
        std::fs::create_dir_all(custom.join("src")).unwrap();
        let resolved = resolve_framework_root(tmp.path(), Some(&custom)).unwrap();
        assert_eq!(resolved, custom);
    }

    #[test]
    fn framework_version_reads_cargo_toml() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"1.2.3\"\n",
        )
        .unwrap();
        assert_eq!(framework_version(tmp.path()), "1.2.3");
    }

    #[test]
    fn parse_manifest_source_path_extracts_field() {
        let body = "schema_version = 1\nsource_path = \"/x/y.pdf\"\n";
        assert_eq!(
            parse_manifest_source_path(body),
            Some(PathBuf::from("/x/y.pdf"))
        );
    }
}
