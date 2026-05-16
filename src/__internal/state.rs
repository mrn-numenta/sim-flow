//! `.sim-flow/state.toml` schema, load/save, and gate-transition logic.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const STATE_FILE: &str = "state.toml";
pub const STATE_LOCK_FILE: &str = "state.lock";

/// Guard holding an exclusive advisory file lock on
/// `<dot_dir>/state.lock`. Released on drop. Used by
/// [`State::transaction`] to serialize concurrent state.toml
/// mutations across processes (two terminals running
/// `sim-flow auto`, a dashboard issuing Advance while auto is
/// running, etc.). The lock is per-`.sim-flow/` so projects in
/// different directories are independent.
///
/// Wraps `fs2::FileExt::lock_exclusive` (flock on Unix,
/// LockFileEx on Windows). The lockfile itself contains no data;
/// it exists purely as a kernel-tracked lock target.
///
/// See orchestrator audit #2 (2026-05-16). Adoption is in-progress
/// -- callers that load-mutate-save state should migrate to
/// `State::transaction`; until then the legacy `State::load` +
/// `state.save` path remains vulnerable to TOCTOU clobbers
/// between load and save (orchestrator audit #14).
pub struct StateLock {
    /// Held open for the duration of the lock; dropping it
    /// releases the flock.
    _file: std::fs::File,
}

impl StateLock {
    pub fn acquire(dot_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dot_dir).map_err(|source| Error::Io {
            path: dot_dir.to_path_buf(),
            source,
        })?;
        let path = dot_dir.join(STATE_LOCK_FILE);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
        file.lock_exclusive().map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        Ok(Self { _file: file })
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = self._file.unlock();
    }
}

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

    /// Atomic save to `<dir>/state.toml`. Acquires
    /// `<dir>/state.lock` for the write so two concurrent saves
    /// from different sim-flow processes serialize at the FS
    /// level. Does NOT protect the read-mutate-save cycle from
    /// load+save TOCTOU -- callers needing that should use
    /// [`State::transaction`].
    pub fn save(&self, dir: &Path) -> Result<()> {
        let _lock = StateLock::acquire(dir)?;
        let path = dir.join(STATE_FILE);
        let text = toml::to_string_pretty(self)?;
        write_atomic(&path, text.as_bytes())
    }

    /// Run `f` under an exclusive cross-process lock on
    /// `<dir>/state.lock`, loading the current state, passing
    /// `&mut self` so `f` can mutate, then saving on success.
    /// Both load and save run while the lock is held, so a
    /// concurrent sim-flow process performing its own transaction
    /// will see this transaction's changes (and vice versa)
    /// -- closes the TOCTOU window in [`State::load`] +
    /// `state.save` patterns. See orchestrator audit #2 + #14
    /// (2026-05-16).
    pub fn transaction<R>(dir: &Path, f: impl FnOnce(&mut Self) -> Result<R>) -> Result<R> {
        let _lock = StateLock::acquire(dir)?;
        let mut state = Self::load(dir)?;
        let result = f(&mut state)?;
        let path = dir.join(STATE_FILE);
        let text = toml::to_string_pretty(&state)?;
        write_atomic(&path, text.as_bytes())?;
        Ok(result)
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
    ///
    /// If `archived_gates["ds"]` already exists (a prior flip-to-DMF
    /// that was reverted), suffix the new key with a sequence number
    /// so the older archive isn't silently overwritten. See
    /// orchestrator audit #9 (2026-05-16).
    pub fn flip_to_dmf(&mut self, dm0_step: &str) {
        if self.flow != Flow::DesignStudy {
            return;
        }
        let prior = std::mem::take(&mut self.gates);
        let key = next_archive_key(&self.archived_gates, "ds");
        self.archived_gates.insert(key, prior);
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
        // Suffix the archive key if "dm" already exists (a prior
        // SV-Convert flip that was reverted, e.g. via --force flow
        // restore). The audit-trail promise -- visible via
        // `sim-flow status` -- would otherwise be silently
        // violated. See orchestrator audit #9 (2026-05-16).
        let key = next_archive_key(&self.archived_gates, "dm");
        self.archived_gates.insert(key, prior);
        self.flow = Flow::SystemVerilogConvert;
        self.current_step = sv0_step.to_string();
    }
}

/// Return a fresh key for `archived_gates` based on `base`:
/// `<base>` if unused, else `<base>-2`, `<base>-3`, ... Used so
/// successive flip operations (or a `--force` revert followed by
/// another flip) don't clobber each other's archive entries.
fn next_archive_key(archived: &BTreeMap<String, BTreeMap<String, Gate>>, base: &str) -> String {
    if !archived.contains_key(base) {
        return base.to_string();
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base}-{n}");
        if !archived.contains_key(&candidate) {
            return candidate;
        }
        n = n.saturating_add(1);
        // Pathological upper bound -- nobody flips this many times.
        if n > 1_000 {
            return format!("{base}-overflow");
        }
    }
}

/// Crash-safe atomic write: open temp file, write all bytes,
/// `fsync(tmp)`, rename(tmp, dest), `fsync(parent_dir)`. Without
/// these fsyncs the kernel can lose either the file contents OR
/// the rename's directory-entry update across a power loss /
/// host crash, leaving `state.toml` truncated, empty, or missing
/// on the next boot -- and the orchestrator exits at start because
/// the gate map / current_step was lost. See orchestrator audit
/// #1 (2026-05-16).
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| Error::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let tmp = tmp_path(path);
    // Open + write + sync the tempfile explicitly so the file
    // contents hit disk before we rename. `std::fs::write` doesn't
    // call fsync.
    {
        let mut file = std::fs::File::create(&tmp).map_err(|source| Error::Io {
            path: tmp.clone(),
            source,
        })?;
        file.write_all(bytes).map_err(|source| Error::Io {
            path: tmp.clone(),
            source,
        })?;
        file.sync_all().map_err(|source| Error::Io {
            path: tmp.clone(),
            source,
        })?;
    }
    std::fs::rename(&tmp, path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    // Sync the parent directory so the rename's directory-entry
    // update is durable. Without this, a crash after the rename
    // returned can still leave the dirent pointing at the old
    // file (or no file). On platforms where opening a directory
    // for fsync is unsupported, we silently skip -- best effort.
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
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
