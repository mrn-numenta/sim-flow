//! `.sim-flow/state.toml` schema, load/save, and gate-transition logic.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const STATE_FILE: &str = "state.toml";

/// Which flow a project is currently in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Flow {
    DirectModeling,
    DesignStudy,
    /// SystemVerilog conversion. Translates a DirectModeling-completed
    /// project's Rust model + UVM-lite testbench into synthesizable
    /// SystemVerilog RTL + a UVM testbench, with milestone-walked
    /// per-module emission and a verilator-driven validation gate.
    /// Opted-in by switching the flow after DM4b passes.
    SystemVerilogConvert,
}

impl Flow {
    pub fn as_str(&self) -> &'static str {
        match self {
            Flow::DirectModeling => "direct-modeling",
            Flow::DesignStudy => "design-study",
            Flow::SystemVerilogConvert => "systemverilog-convert",
        }
    }
}

/// A gate's aggregate status. Per-candidate details live in `candidates`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Gate {
    #[serde(default)]
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub candidates: BTreeMap<String, Gate>,
}

/// Root state document stored at `.sim-flow/state.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub flow: Flow,
    pub current_step: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,
    #[serde(default)]
    pub gates: BTreeMap<String, Gate>,
    /// Archived gate history from a prior flow; populated by DS9 when a
    /// study transitions into the Direct Modeling Flow in place.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub archived_gates: BTreeMap<String, BTreeMap<String, Gate>>,
}

impl State {
    pub fn new(flow: Flow, current_step: impl Into<String>) -> Self {
        Self {
            flow,
            current_step: current_step.into(),
            started: None,
            gates: BTreeMap::new(),
            archived_gates: BTreeMap::new(),
        }
    }

    /// Load from `<dir>/state.toml`.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(STATE_FILE);
        let text = std::fs::read_to_string(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| Error::TomlParse { path, source })
    }

    /// Atomic save to `<dir>/state.toml`.
    pub fn save(&self, dir: &Path) -> Result<()> {
        let path = dir.join(STATE_FILE);
        let text = toml::to_string_pretty(self)?;
        write_atomic(&path, text.as_bytes())
    }

    /// Mark a (flat) gate passed.
    pub fn mark_passed(&mut self, step: &str, timestamp: impl Into<String>) {
        let gate = self.gates.entry(step.to_string()).or_default();
        gate.passed = true;
        gate.timestamp = Some(timestamp.into());
    }

    /// Mark a per-candidate gate passed. The aggregate is recomputed from
    /// candidate children.
    /// Mark a per-candidate gate passed. Does NOT touch the aggregate;
    /// call [`State::recompute_aggregate`] with the full expected
    /// candidate set to flip the aggregate when every expected candidate
    /// has passed.
    pub fn mark_candidate_passed(
        &mut self,
        step: &str,
        candidate: &str,
        timestamp: impl Into<String>,
    ) {
        let gate = self.gates.entry(step.to_string()).or_default();
        let child = gate.candidates.entry(candidate.to_string()).or_default();
        child.passed = true;
        child.timestamp = Some(timestamp.into());
    }

    /// Recompute the aggregate pass for a per-candidate step given the
    /// canonical expected candidate list. The aggregate passes only when
    /// every expected candidate has an entry with `passed = true`.
    pub fn recompute_aggregate(
        &mut self,
        step: &str,
        expected: &[&str],
        timestamp: impl Into<String>,
    ) {
        let gate = self.gates.entry(step.to_string()).or_default();
        let all_pass = !expected.is_empty()
            && expected
                .iter()
                .all(|c| gate.candidates.get(*c).map(|g| g.passed).unwrap_or(false));
        gate.passed = all_pass;
        if gate.passed && gate.timestamp.is_none() {
            gate.timestamp = Some(timestamp.into());
        }
    }

    /// Return whether the given step's aggregate gate has passed.
    pub fn is_passed(&self, step: &str) -> bool {
        self.gates.get(step).map(|g| g.passed).unwrap_or(false)
    }

    /// Reset a step and cascade the reset to every downstream step as
    /// determined by `order`. Steps not in `order` are left alone.
    pub fn reset(&mut self, step: &str, order: &[&str]) -> Result<()> {
        let idx = order
            .iter()
            .position(|s| *s == step)
            .ok_or_else(|| Error::InvalidStep(step.to_string()))?;
        for downstream in &order[idx..] {
            if let Some(gate) = self.gates.get_mut(*downstream) {
                gate.passed = false;
                gate.timestamp = None;
                for child in gate.candidates.values_mut() {
                    child.passed = false;
                    child.timestamp = None;
                }
            }
        }
        self.current_step = step.to_string();
        Ok(())
    }

    /// In-place flip from DSF to DMF. Preserves DSF gate history under
    /// `archived_gates` so audit is possible.
    pub fn flip_to_dmf(&mut self, dm0_step: &str) {
        if self.flow != Flow::DesignStudy {
            return;
        }
        let prior = std::mem::take(&mut self.gates);
        self.archived_gates.insert("ds".to_string(), prior);
        self.flow = Flow::DirectModeling;
        self.current_step = dm0_step.to_string();
    }

    /// In-place flip from Direct Modeling Flow to SystemVerilog
    /// Convert. Preserves DMF gate history under `archived_gates` so
    /// `sim-flow status` / audit tools can still see that DM4b
    /// passed (the SV-Convert prerequisite). Resets `current_step` to
    /// SV0. A no-op when the project isn't currently in DMF, so
    /// double-calls don't clobber an in-progress SV-Convert run.
    pub fn flip_to_sv_convert(&mut self, sv0_step: &str) {
        if self.flow != Flow::DirectModeling {
            return;
        }
        let prior = std::mem::take(&mut self.gates);
        self.archived_gates.insert("dm".to_string(), prior);
        self.flow = Flow::SystemVerilogConvert;
        self.current_step = sv0_step.to_string();
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| Error::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let tmp = tmp_path(path);
    std::fs::write(&tmp, bytes).map_err(|source| Error::Io {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trip_minimal() {
        let dir = tempdir().unwrap();
        let state = State::new(Flow::DirectModeling, "DM0");
        state.save(dir.path()).unwrap();
        let loaded = State::load(dir.path()).unwrap();
        assert_eq!(loaded.flow, Flow::DirectModeling);
        assert_eq!(loaded.current_step, "DM0");
        assert!(loaded.gates.is_empty());
    }

    #[test]
    fn mark_and_query_gate() {
        let mut state = State::new(Flow::DirectModeling, "DM0");
        assert!(!state.is_passed("DM0"));
        state.mark_passed("DM0", "2026-04-20T00:00:00Z");
        assert!(state.is_passed("DM0"));
    }

    #[test]
    fn candidate_aggregate_pass() {
        let mut state = State::new(Flow::DesignStudy, "DS5a");
        state.mark_candidate_passed("DS5a", "mesh", "t1");
        state.recompute_aggregate("DS5a", &["mesh", "ring"], "agg-t");
        assert!(!state.is_passed("DS5a"));
        state.mark_candidate_passed("DS5a", "ring", "t2");
        state.recompute_aggregate("DS5a", &["mesh", "ring"], "agg-t");
        assert!(state.is_passed("DS5a"));
    }

    #[test]
    fn reset_cascades() {
        let order = ["DM0", "DM1", "DM2a", "DM2b"];
        let mut state = State::new(Flow::DirectModeling, "DM0");
        for step in &order {
            state.mark_passed(step, "t");
        }
        state.reset("DM2a", &order).unwrap();
        assert!(state.is_passed("DM0"));
        assert!(state.is_passed("DM1"));
        assert!(!state.is_passed("DM2a"));
        assert!(!state.is_passed("DM2b"));
        assert_eq!(state.current_step, "DM2a");
    }

    #[test]
    fn flip_to_dmf_preserves_history() {
        let mut state = State::new(Flow::DesignStudy, "DS9");
        state.mark_passed("DS0", "t");
        state.mark_passed("DS9", "t");
        state.flip_to_dmf("DM0");
        assert_eq!(state.flow, Flow::DirectModeling);
        assert_eq!(state.current_step, "DM0");
        assert!(state.gates.is_empty());
        let archived = state.archived_gates.get("ds").unwrap();
        assert!(archived.get("DS0").map(|g| g.passed).unwrap_or(false));
        assert!(archived.get("DS9").map(|g| g.passed).unwrap_or(false));
    }

    #[test]
    fn flip_to_sv_convert_archives_dm_history() {
        let mut state = State::new(Flow::DirectModeling, "DM4b");
        state.mark_passed("DM0", "t");
        state.mark_passed("DM2d", "t");
        state.mark_passed("DM4b", "t");
        state.flip_to_sv_convert("SV0");
        assert_eq!(state.flow, Flow::SystemVerilogConvert);
        assert_eq!(state.current_step, "SV0");
        assert!(state.gates.is_empty());
        let archived = state.archived_gates.get("dm").unwrap();
        assert!(archived.get("DM0").map(|g| g.passed).unwrap_or(false));
        assert!(archived.get("DM4b").map(|g| g.passed).unwrap_or(false));
    }

    #[test]
    fn flip_to_sv_convert_is_no_op_when_not_in_dmf() {
        // Already SV-Convert: double-call must not clobber state.
        let mut state = State::new(Flow::SystemVerilogConvert, "SV1");
        state.mark_passed("SV0", "t");
        state.flip_to_sv_convert("SV0");
        assert_eq!(state.flow, Flow::SystemVerilogConvert);
        assert_eq!(state.current_step, "SV1");
        assert!(state.is_passed("SV0"));

        // Design study: should reject (no flip).
        let mut state = State::new(Flow::DesignStudy, "DS5");
        state.flip_to_sv_convert("SV0");
        assert_eq!(state.flow, Flow::DesignStudy);
        assert_eq!(state.current_step, "DS5");
    }
}
