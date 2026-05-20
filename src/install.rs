//! Build sim-flow + the VS Code extension, optionally install the VSIX.
//!
//! This module replaces the legacy `scripts/install-vscode-extension.sh`
//! and is the canonical entry point for consumers (sim-models' `xtask`
//! crate, the `sim-flow install-extension` CLI subcommand, etc.) that
//! want to install sim-flow without going through the shell.
//!
//! Lifting the install flow out of bash means callers can supply
//! sim-flow's own crate root and a pre-built binary together, so the
//! VSIX embeds a binary linked against whatever Cargo.lock the caller
//! is resolving — critical for sim-models, which needs the bundled
//! `sim-flow` binary to see the same `sim-foundation` SHA the rest of
//! sim-models is built against.
//!
//! Pipeline:
//!   1. Either build the sim-flow binary (`cargo build [-r] -p
//!      sim-flow --manifest-path <root>/Cargo.toml`) or accept a
//!      caller-supplied path via [`Options::prebuilt_binary`].
//!   2. Spawn `npm` in `<root>/extensions/sim-flow-vscode/` with
//!      `SIM_FLOW_BUNDLE_BINARY` pointing at the binary; `bundle-bin.mjs`
//!      stages it into the VSIX, and `compile-cargo.mjs` skips its own
//!      cargo build because the same env var is set.
//!   3. With [`Options::package_only`] false, also runs `npm run
//!      reload` which force-installs the freshly-built VSIX via the
//!      `code` CLI (`VSCODE_BIN`, or the macOS default location).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Error, Result};

/// Cargo build profile.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Profile {
    /// `cargo build` (no `--release`). Smaller compile-time at the
    /// cost of a slower / larger embedded binary.
    Dev,
    /// `cargo build --release`. What production VSIX installs ship.
    Release,
}

impl Profile {
    fn target_subdir(self) -> &'static str {
        match self {
            Profile::Dev => "debug",
            Profile::Release => "release",
        }
    }
    fn flag(self) -> &'static str {
        match self {
            Profile::Dev => "dev",
            Profile::Release => "release",
        }
    }
}

/// Configuration for one install pass.
#[derive(Debug, Clone)]
pub struct Options {
    /// Root of the sim-flow checkout. `<root>/Cargo.toml` is what
    /// `cargo build` runs against; `<root>/extensions/sim-flow-vscode/`
    /// is where `npm` runs. When the install is driven from the
    /// sim-flow binary itself, `env!("CARGO_MANIFEST_DIR")` is the
    /// right value here.
    pub sim_flow_root: PathBuf,

    /// Profile for the cargo build. Ignored when [`prebuilt_binary`]
    /// is supplied.
    pub profile: Profile,

    /// When true, stop after `npm run package` and print the path to
    /// the produced VSIX. When false, run `npm run reload` which
    /// packages AND force-installs into VS Code.
    pub package_only: bool,

    /// Path to a pre-built sim-flow binary. When supplied, the cargo
    /// build step is skipped and the path is exported to the npm flow
    /// as `SIM_FLOW_BUNDLE_BINARY`. This is the hook sim-models uses
    /// to install a binary it built itself (against sim-models'
    /// Cargo.lock, not sim-flow's).
    pub prebuilt_binary: Option<PathBuf>,

    /// Override for the `code` CLI used by `npm run reload`. When
    /// `None`, the install path probes a small list of well-known
    /// locations (currently just the macOS default) and falls back to
    /// whatever's on `$PATH`.
    pub vscode_bin: Option<PathBuf>,
}

/// Outcome of [`install_extension`].
#[derive(Debug)]
pub struct Outcome {
    /// Absolute path to the binary that ended up bundled into the
    /// VSIX. Either the freshly-built `<root>/target/<profile>/sim-flow`
    /// or [`Options::prebuilt_binary`] unchanged.
    pub binary: PathBuf,
    /// Absolute path to the produced VSIX. Only populated in
    /// [`Options::package_only`] mode — when running the full
    /// reload flow, `npm run reload` builds and installs without
    /// surfacing the VSIX path, so this stays `None`.
    pub vsix: Option<PathBuf>,
}

/// Build sim-flow + the VS Code extension, optionally installing the
/// VSIX in VS Code.
pub fn install_extension(opts: Options) -> Result<Outcome> {
    let ext_dir = opts
        .sim_flow_root
        .join("extensions")
        .join("sim-flow-vscode");
    if !ext_dir.is_dir() {
        return Err(Error::State(format!(
            "extension dir not found: {} (is `sim_flow_root` pointing at the sim-flow repo root?)",
            ext_dir.display()
        )));
    }

    let binary = resolve_binary(
        &opts.sim_flow_root,
        opts.profile,
        opts.prebuilt_binary.as_deref(),
    )?;

    let mut npm = Command::new("npm");
    npm.arg("--prefix").arg(&ext_dir);
    npm.env("SIM_FLOW_BUNDLE_BINARY", &binary);
    if let Some(code) = opts.vscode_bin.as_deref().or_else(|| default_vscode_bin()) {
        npm.env("VSCODE_BIN", code);
    }

    let outcome = if opts.package_only {
        eprintln!("install-extension: packaging VSIX (no install)");
        npm.arg("run").arg("package");
        run_status(&ext_dir, &mut npm, "npm run package")?;
        let vsix = locate_latest_vsix(&ext_dir)?;
        eprintln!("install-extension: VSIX ready: {}", vsix.display());
        eprintln!(
            "install-extension: install on another machine with:\n  code --install-extension {}",
            vsix.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| vsix.display().to_string())
        );
        Outcome {
            binary,
            vsix: Some(vsix),
        }
    } else {
        eprintln!("install-extension: packaging + installing via npm run reload");
        npm.arg("run").arg("reload");
        run_status(&ext_dir, &mut npm, "npm run reload")?;
        Outcome { binary, vsix: None }
    };

    Ok(outcome)
}

fn resolve_binary(
    sim_flow_root: &Path,
    profile: Profile,
    prebuilt: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(path) = prebuilt {
        if !path.exists() {
            return Err(Error::State(format!(
                "prebuilt_binary {} does not exist",
                path.display()
            )));
        }
        eprintln!(
            "install-extension: using pre-built binary: {}",
            path.display()
        );
        return Ok(path.to_path_buf());
    }
    let manifest = sim_flow_root.join("Cargo.toml");
    if !manifest.is_file() {
        return Err(Error::State(format!(
            "Cargo.toml not found at {} (is `sim_flow_root` correct?)",
            manifest.display()
        )));
    }
    eprintln!(
        "install-extension: building sim-flow binary (--{})",
        profile.flag()
    );
    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("-p")
        .arg("sim-flow")
        .arg("--manifest-path")
        .arg(&manifest);
    if profile == Profile::Release {
        cmd.arg("--release");
    }
    run_status(sim_flow_root, &mut cmd, "cargo build")?;
    let exe = if cfg!(windows) {
        "sim-flow.exe"
    } else {
        "sim-flow"
    };
    let bin = sim_flow_root
        .join("target")
        .join(profile.target_subdir())
        .join(exe);
    if !bin.is_file() {
        return Err(Error::State(format!(
            "cargo build succeeded but binary {} not found",
            bin.display()
        )));
    }
    Ok(bin)
}

fn run_status(cwd: &Path, cmd: &mut Command, label: &str) -> Result<()> {
    let status = cmd.status().map_err(|source| Error::Io {
        path: cwd.to_path_buf(),
        source,
    })?;
    if !status.success() {
        return Err(Error::State(format!("{label} exited with status {status}")));
    }
    Ok(())
}

fn locate_latest_vsix(ext_dir: &Path) -> Result<PathBuf> {
    let build = ext_dir.join("build");
    let entries = std::fs::read_dir(&build).map_err(|source| Error::Io {
        path: build.clone(),
        source,
    })?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: build.clone(),
            source,
        })?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !name.starts_with("sim-flow-vscode-") || !name.ends_with(".vsix") {
            continue;
        }
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
        if newest.as_ref().is_none_or(|(t, _)| mtime > *t) {
            newest = Some((mtime, path));
        }
    }
    newest.map(|(_, p)| p).ok_or_else(|| {
        Error::State(format!(
            "package succeeded but no sim-flow-vscode-*.vsix under {}",
            build.display()
        ))
    })
}

fn default_vscode_bin() -> Option<&'static Path> {
    const CANDIDATES: &[&str] =
        &["/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"];
    for candidate in CANDIDATES {
        let path = Path::new(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}
