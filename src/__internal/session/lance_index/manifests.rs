//! Manifest types and serdes (Chapter 3 §3.3 / §3.8).
//!
//! Each lance-index tree carries two TOML manifests next to the
//! `*.lance/` directories: one describing the index itself
//! (`manifest.toml`) and one recording the embedder identity used at
//! build time (`embedder.toml`). The build pipelines write both;
//! queries read both to decide whether the on-disk index is fresh
//! and compatible with the orchestrator's runtime embedder.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Per-tree embedder identity. The match rules in §3.3 are:
/// provider + model + dimension must match exactly. `base_url` is
/// informational only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedderManifest {
    pub schema_version: u32,
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub dimension: usize,
    pub indexed_at: String,
    /// Optional `[auth]` block (recorded for diagnostics; not used
    /// to decide match).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<EmbedderManifestAuth>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedderManifestAuth {
    pub header_name: String,
    pub env_var: String,
    #[serde(default)]
    pub value_prefix: String,
}

impl EmbedderManifest {
    /// Two manifests match iff provider, model, and dimension agree
    /// exactly. (`base_url` is recorded but not compared -- the index
    /// can be queried from a different host serving the same model.)
    pub fn matches(&self, other: &EmbedderManifest) -> bool {
        self.provider == other.provider
            && self.model == other.model
            && self.dimension == other.dimension
    }

    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let body = std::fs::read_to_string(path).map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&body).map_err(|source| ManifestError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), ManifestError> {
        let body = toml::to_string_pretty(self).map_err(|source| ManifestError::Serialize {
            path: path.to_path_buf(),
            source,
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ManifestError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(path, body).map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }
}

/// Framework-tree manifest (Chapter 3 §3.8, first TOML block).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiIndexManifest {
    pub schema_version: u32,
    pub indexed_at: String,
    pub framework_version: String,
    pub framework_workspace_hash: String,
    pub vector_index_type: String,
    pub row_count: u64,
}

impl ApiIndexManifest {
    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let body = std::fs::read_to_string(path).map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&body).map_err(|source| ManifestError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), ManifestError> {
        let body = toml::to_string_pretty(self).map_err(|source| ManifestError::Serialize {
            path: path.to_path_buf(),
            source,
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ManifestError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(path, body).map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }
}

/// Per-project spec-tree manifest (Chapter 3 §3.8, second TOML block).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecIndexManifest {
    pub schema_version: u32,
    pub indexed_at: String,
    pub spec_ingest_manifest: String,
    pub spec_ingest_source_sha256: String,
    pub spec_md_sha256: String,
    #[serde(default)]
    pub counts: BTreeMap<String, u64>,
}

impl SpecIndexManifest {
    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let body = std::fs::read_to_string(path).map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&body).map_err(|source| ManifestError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), ManifestError> {
        let body = toml::to_string_pretty(self).map_err(|source| ManifestError::Serialize {
            path: path.to_path_buf(),
            source,
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ManifestError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(path, body).map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum ManifestError {
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: std::path::PathBuf,
        source: toml::de::Error,
    },
    Serialize {
        path: std::path::PathBuf,
        source: toml::ser::Error,
    },
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Io { path, source } => {
                write!(
                    f,
                    "lance-index manifest I/O at {}: {source}",
                    path.display()
                )
            }
            ManifestError::Parse { path, source } => write!(
                f,
                "lance-index manifest parse at {}: {source}",
                path.display()
            ),
            ManifestError::Serialize { path, source } => write!(
                f,
                "lance-index manifest serialize at {}: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ManifestError::Io { source, .. } => Some(source),
            ManifestError::Parse { source, .. } => Some(source),
            ManifestError::Serialize { source, .. } => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_embedder() -> EmbedderManifest {
        EmbedderManifest {
            schema_version: 1,
            provider: "openai-compat".into(),
            base_url: "http://localhost:11434/v1".into(),
            model: "nomic-embed-text".into(),
            dimension: 768,
            indexed_at: "2026-05-17T10:23:45Z".into(),
            auth: None,
        }
    }

    #[test]
    fn embedder_manifest_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("embedder.toml");
        let m = sample_embedder();
        m.save(&path).expect("save");
        let loaded = EmbedderManifest::load(&path).expect("load");
        assert_eq!(loaded, m);
    }

    #[test]
    fn embedder_matches_compares_provider_model_dimension() {
        let a = sample_embedder();
        let b = EmbedderManifest {
            base_url: "http://other-host/v1".into(),
            indexed_at: "different time".into(),
            ..a.clone()
        };
        assert!(a.matches(&b), "base_url + indexed_at do not affect match");

        let dim_off = EmbedderManifest {
            dimension: 1024,
            ..a.clone()
        };
        assert!(!a.matches(&dim_off));

        let model_off = EmbedderManifest {
            model: "other-model".into(),
            ..a.clone()
        };
        assert!(!a.matches(&model_off));

        let provider_off = EmbedderManifest {
            provider: "voyage".into(),
            ..a.clone()
        };
        assert!(!a.matches(&provider_off));
    }

    #[test]
    fn api_index_manifest_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("manifest.toml");
        let m = ApiIndexManifest {
            schema_version: 1,
            indexed_at: "2026-05-17T10:23:45Z".into(),
            framework_version: "0.1.0".into(),
            framework_workspace_hash: "abc123".into(),
            vector_index_type: "ivf_flat".into(),
            row_count: 2734,
        };
        m.save(&path).expect("save");
        let loaded = ApiIndexManifest::load(&path).expect("load");
        assert_eq!(loaded, m);
    }

    #[test]
    fn spec_index_manifest_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("manifest.toml");
        let mut counts = BTreeMap::new();
        counts.insert("spec_chunks".to_string(), 87);
        counts.insert("signal_table_rows".to_string(), 142);
        counts.insert("cross_spec_refs".to_string(), 3);
        let m = SpecIndexManifest {
            schema_version: 1,
            indexed_at: "2026-05-17T10:23:45Z".into(),
            spec_ingest_manifest: "/tmp/.sim-flow/spec-ingest/manifest.toml".into(),
            spec_ingest_source_sha256: "deadbeef".into(),
            spec_md_sha256: "feedface".into(),
            counts,
        };
        m.save(&path).expect("save");
        let loaded = SpecIndexManifest::load(&path).expect("load");
        assert_eq!(loaded, m);
    }
}
