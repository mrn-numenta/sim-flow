//! Block-diagram rendering helper. The CLI's `block-diagram`
//! subcommand and the auto-driver's DM2d -> DM3a transition both
//! call into this so an SVG always lands at
//! `<project>/.sim-flow/block-diagram.svg` from the same code path.
//!
//! Pipeline:
//!   1. Run `cargo run --quiet -- --dump-netlist-json <tmp>` in the
//!      project so the model's main binary serializes its
//!      `ConnectivityPlan` to a JSON file. This step depends on the
//!      project wiring `cli::DumpNetlist` (or equivalent) into its
//!      main; without that wiring the cargo run succeeds but no
//!      netlist lands on disk -- we surface that as a clear error.
//!   2. Hand the netlist JSON to the workspace `block-diagram` crate
//!      to render an SVG via Sugiyama layout.
//!   3. Write the SVG to the requested output path.
//!
//! Errors are typed via `crate::Error::State` so callers can decide
//! whether to abort (CLI subcommand) or surface a warning and
//! continue (auto-driver advance hook).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Error, Result};

/// Configuration for one render pass. `output` defaults to
/// `<project>/.sim-flow/block-diagram.svg` when `None`. `direction`
/// accepts `"tb"` / `"top-to-bottom"` (default) and `"lr"` /
/// `"left-to-right"`.
pub struct RenderConfig<'a> {
    pub project_dir: &'a Path,
    pub output: Option<&'a Path>,
    pub direction: &'a str,
    pub show_types: bool,
    /// Caller-supplied netlist path. When `None`, the helper
    /// invokes the project's binary to produce one.
    pub netlist_in: Option<&'a Path>,
}

/// Render the block diagram and return the path the SVG was
/// written to.
pub fn render_for_project(cfg: RenderConfig<'_>) -> Result<PathBuf> {
    use block_diagram::__internal::render::RenderOptions;
    use block_diagram::__internal::sugiyama::{Direction, LayoutConfig};

    let dot = cfg.project_dir.join(".sim-flow");
    std::fs::create_dir_all(&dot).map_err(|err| Error::Io {
        path: dot.clone(),
        source: err,
    })?;
    let output_path = cfg
        .output
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| dot.join("block-diagram.svg"));

    let direction = match cfg.direction {
        "lr" | "left-to-right" | "LR" => Direction::LeftToRight,
        _ => Direction::TopToBottom,
    };

    // Get a netlist JSON. Either the caller pre-generated one (e.g.
    // via a custom build step) or we run the project's own binary
    // with `--dump-netlist-json`. The framework's CLI integration
    // adds that flag to every model that wires it up.
    let owned_netlist;
    let netlist_path: &Path = match cfg.netlist_in {
        Some(p) => p,
        None => {
            let tmp = dot.join("block-diagram.netlist.json");
            run_dump_netlist(cfg.project_dir, &tmp)?;
            owned_netlist = tmp;
            owned_netlist.as_path()
        }
    };

    let layout = LayoutConfig {
        direction,
        ..LayoutConfig::default()
    };
    let svg = block_diagram::__internal::render_netlist_file(
        netlist_path,
        layout,
        RenderOptions {
            show_types: cfg.show_types,
        },
    )
    .map_err(|e| Error::State(format!("block-diagram: {e}")))?;
    std::fs::write(&output_path, svg).map_err(|err| Error::Io {
        path: output_path.clone(),
        source: err,
    })?;
    Ok(output_path)
}

fn run_dump_netlist(project: &Path, out: &Path) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("run")
        .arg("--quiet")
        .arg("--")
        .arg("--dump-netlist-json")
        .arg(out)
        .current_dir(project);
    let status = cmd.status().map_err(|err| Error::Io {
        path: project.to_path_buf(),
        source: err,
    })?;
    if !status.success() {
        return Err(Error::State(format!(
            "block-diagram: `cargo run -- --dump-netlist-json {}` exited {}; ensure the project builds and its CLI uses foundation-framework's CliIntegration (which provides --dump-netlist-json)",
            out.display(),
            status.code().unwrap_or(-1)
        )));
    }
    if !out.exists() {
        return Err(Error::State(format!(
            "block-diagram: cargo finished but no netlist at {}; the project's binary may not have invoked `dump_netlist_json`",
            out.display()
        )));
    }
    Ok(())
}
