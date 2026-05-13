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
    /// Coverage acceptance criteria for DM3c. The CLI subcommand
    /// `sim-flow coverage` and the dashboard's Settings tab both
    /// read / write this section. The DM3c critique enforces it
    /// against the live `cargo llvm-cov` report.
    #[serde(default)]
    pub coverage: CoverageSettings,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub steps: BTreeMap<String, StepOverride>,
}

/// DM3c coverage acceptance criteria. `threshold_pct` is the
/// minimum required line-coverage percentage; `level` selects
/// whether the threshold has to hold per-module (every module
/// reaches the bar -- strict) or only on the project-wide total
/// (cheap modules can drag heavy ones up -- lax).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CoverageSettings {
    /// 0.0 ..= 100.0. Stored as f32 because the dashboard surfaces
    /// it as a percentage with one decimal of precision; we never
    /// round-trip it to and from doubles.
    pub threshold_pct: f32,
    pub level: CoverageLevel,
}

impl Default for CoverageSettings {
    fn default() -> Self {
        // 90% / total matches the historical DM3c critique baseline
        // (the prompt previously hard-coded "90%" and didn't
        // distinguish module-vs-total). Default `Total` keeps the
        // pre-config behaviour for projects whose `config.toml`
        // pre-dates this section.
        Self {
            threshold_pct: 90.0,
            level: CoverageLevel::Total,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoverageLevel {
    /// Every reported module must hit `threshold_pct`. Strict
    /// reading; surfaces gaps in any one file.
    Module,
    /// Only the project-wide total has to hit `threshold_pct`.
    /// Heavily-tested modules can offset thinly-tested ones.
    Total,
}

impl CoverageLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            CoverageLevel::Module => "module",
            CoverageLevel::Total => "total",
        }
    }
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

    /// Update the `[coverage]` section in-place. Clamps the
    /// percentage to `[0.0, 100.0]` so a typo in the dashboard's
    /// number input can't write 9000% to disk. Returns the clamped
    /// value the caller can echo back to the user.
    pub fn set_coverage(&mut self, threshold_pct: f32, level: CoverageLevel) -> CoverageSettings {
        let clamped = threshold_pct.clamp(0.0, 100.0);
        self.coverage = CoverageSettings {
            threshold_pct: clamped,
            level,
        };
        self.coverage
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
    fn coverage_defaults_to_90_total() {
        let cfg = Config::default();
        assert_eq!(cfg.coverage.threshold_pct, 90.0);
        assert_eq!(cfg.coverage.level, CoverageLevel::Total);
    }

    #[test]
    fn coverage_round_trips_through_toml() {
        let dir = tempdir().unwrap();
        let cfg = Config {
            coverage: CoverageSettings {
                threshold_pct: 75.5,
                level: CoverageLevel::Module,
            },
            ..Config::default()
        };
        cfg.save(dir.path()).unwrap();
        let loaded = Config::load(dir.path()).unwrap();
        assert_eq!(loaded.coverage.threshold_pct, 75.5);
        assert_eq!(loaded.coverage.level, CoverageLevel::Module);
    }

    #[test]
    fn coverage_loads_from_legacy_config_without_section() {
        // Older `config.toml` files predate the [coverage] section;
        // loading them must not fail and must surface the defaults.
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[client]\nname = \"mock\"\n",
        )
        .unwrap();
        let loaded = Config::load(dir.path()).unwrap();
        assert_eq!(loaded.coverage, CoverageSettings::default());
    }

    #[test]
    fn set_coverage_clamps_out_of_range_values() {
        let mut cfg = Config::default();
        let above = cfg.set_coverage(120.0, CoverageLevel::Module);
        assert_eq!(above.threshold_pct, 100.0);
        let below = cfg.set_coverage(-5.0, CoverageLevel::Total);
        assert_eq!(below.threshold_pct, 0.0);
        assert_eq!(cfg.coverage.level, CoverageLevel::Total);
    }

    #[test]
    fn coverage_level_serde_uses_lowercase() {
        // The dashboard's webview message protocol speaks the
        // lowercase forms; serde rename_all keeps the on-disk form
        // matching that. Round-trip both variants via a wrapper
        // struct because TOML can't serialize a bare enum value at
        // the document root.
        #[derive(Serialize, Deserialize)]
        struct Wrap {
            level: CoverageLevel,
        }
        let module = toml::to_string(&Wrap {
            level: CoverageLevel::Module,
        })
        .unwrap();
        let total = toml::to_string(&Wrap {
            level: CoverageLevel::Total,
        })
        .unwrap();
        assert!(module.contains("\"module\""), "got {module}");
        assert!(total.contains("\"total\""), "got {total}");
        assert_eq!(CoverageLevel::Module.as_str(), "module");
        assert_eq!(CoverageLevel::Total.as_str(), "total");
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
