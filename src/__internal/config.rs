//! `.sim-flow/config.toml` schema with precedence:
//!   1. `.sim-flow/config.toml` (committed source of truth)
//!   2. CLI flags (applied on top of the loaded config)
//!   3. Environment variables
//!
//! Per-step overrides live under `[steps.<step-id>]` and may replace the
//! `client` name, model, or tool allowlist for that step.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub client: ClientSelector,
    #[serde(default)]
    pub claude: ClaudeSettings,
    #[serde(default)]
    pub codex: CodexSettings,
    #[serde(default)]
    pub copilot: CopilotSettings,
    /// Path to the user-supplied source spec the orchestrator should
    /// ingest before DM0 starts. The dashboard writes this when the
    /// user types into the Spec field; the pre-DM0 hook reads it and
    /// runs `ingest_spec_file` if `.sim-flow/source-spec*` is missing
    /// or out of date relative to this path. Empty / `None` means no
    /// source spec is configured (DM0 then prompts the user
    /// interactively in manual mode, or auto-decides in automated
    /// mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_path: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub steps: BTreeMap<String, StepOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientSelector {
    pub name: ClientName,
}

impl Default for ClientSelector {
    fn default() -> Self {
        Self {
            name: ClientName::Mock,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientName {
    Claude,
    Codex,
    Copilot,
    /// Deterministic test client. Not intended for user-facing config, but
    /// kept as a valid enum value so tests can round-trip.
    Mock,
}

impl ClientName {
    pub fn as_str(&self) -> &'static str {
        match self {
            ClientName::Claude => "claude",
            ClientName::Codex => "codex",
            ClientName::Copilot => "copilot",
            ClientName::Mock => "mock",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaudeSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CopilotSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StepOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<ClientName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

impl Config {
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(CONFIG_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| Error::TomlParse { path, source })
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        let path = dir.join(CONFIG_FILE);
        let text = toml::to_string_pretty(self)?;
        std::fs::create_dir_all(dir).map_err(|source| Error::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        std::fs::write(&path, text).map_err(|source| Error::Io { path, source })
    }

    /// Resolve the effective client for a given step, honoring per-step
    /// overrides.
    pub fn effective_client(&self, step: &str) -> ClientName {
        self.steps
            .get(step)
            .and_then(|o| o.client)
            .unwrap_or(self.client.name)
    }

    /// Resolve the effective model name for a given step.
    pub fn effective_model(&self, step: &str) -> Option<String> {
        if let Some(m) = self.steps.get(step).and_then(|o| o.model.clone()) {
            return Some(m);
        }
        match self.effective_client(step) {
            ClientName::Claude => self.claude.model.clone(),
            ClientName::Codex => self.codex.model.clone(),
            ClientName::Copilot => self.copilot.model.clone(),
            ClientName::Mock => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_when_missing() {
        let dir = tempdir().unwrap();
        let cfg = Config::load(dir.path()).unwrap();
        assert_eq!(cfg.client.name, ClientName::Mock);
    }

    #[test]
    fn round_trip() {
        let dir = tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.client.name = ClientName::Claude;
        cfg.claude.model = Some("sonnet".to_string());
        cfg.save(dir.path()).unwrap();
        let loaded = Config::load(dir.path()).unwrap();
        assert_eq!(loaded.client.name, ClientName::Claude);
        assert_eq!(loaded.claude.model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn per_step_override() {
        let mut cfg = Config::default();
        cfg.client.name = ClientName::Claude;
        cfg.claude.model = Some("sonnet".to_string());
        cfg.steps.insert(
            "DM3".to_string(),
            StepOverride {
                client: Some(ClientName::Codex),
                model: Some("o3".to_string()),
                timeout_seconds: None,
            },
        );
        assert_eq!(cfg.effective_client("DM0"), ClientName::Claude);
        assert_eq!(cfg.effective_client("DM3"), ClientName::Codex);
        assert_eq!(cfg.effective_model("DM3").as_deref(), Some("o3"));
    }
}
