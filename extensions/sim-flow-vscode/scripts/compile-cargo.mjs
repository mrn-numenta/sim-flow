// Build the sim-flow Rust binary that bundle-bin.mjs stages into the
// VSIX. Pulled out as its own script (instead of an inline `cargo
// build` in package.json) so we can resolve the workspace root from
// the extension's nested location and surface a clear failure message
// when the cargo invocation breaks.

import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const extDir = resolve(here, "..");
// extDir = <repo>/extensions/sim-flow-vscode → ../.. = <repo>.
// Mirrors bundle-bin.mjs.
const repoRoot = resolve(extDir, "..", "..");
const rustdocTargetDir = resolve(repoRoot, "target", "sim-flow-vscode-rustdoc");
const normalizedDocsDir = resolve(repoRoot, "target", "sim-flow-vscode-api-docs");

// When `SIM_FLOW_BUNDLE_BINARY` is set, bundle-bin.mjs uses that path
// instead of `<repoRoot>/target/release/sim-flow`, so the cargo build
// step here is redundant. Skip it. The cargo doc + rustdoc-normalize
// steps below stay -- the VSIX still needs foundation docs even when
// the binary was supplied externally (e.g. by sim-models' wrapper that
// pre-built sim-flow against sim-models' Cargo.lock).
const externalBinary = process.env.SIM_FLOW_BUNDLE_BINARY?.trim();
if (externalBinary) {
  console.log(
    `compile:cargo: skipping cargo build (SIM_FLOW_BUNDLE_BINARY=${externalBinary}).`,
  );
} else {
  const buildResult = spawnSync(
    "cargo",
    ["build", "--release", "-p", "sim-flow"],
    { cwd: repoRoot, stdio: "inherit" },
  );
  if (buildResult.status !== 0) {
    console.error("compile:cargo: cargo build failed.");
    process.exit(buildResult.status ?? 1);
  }
}

const docResult = spawnSync(
  "cargo",
  [
    "doc",
    "--no-deps",
    "--target-dir",
    rustdocTargetDir,
    "-p",
    "foundation-framework",
    "-p",
    "foundation-macros",
  ],
  { cwd: repoRoot, stdio: "inherit" },
);
if (docResult.status !== 0) {
  console.error("compile:cargo: cargo doc failed.");
  process.exit(docResult.status ?? 1);
}

const renderResult = spawnSync(
  "node",
  [
    resolve(extDir, "scripts", "render-rustdoc-api.mjs"),
    resolve(rustdocTargetDir, "doc"),
    normalizedDocsDir,
  ],
  { cwd: extDir, stdio: "inherit" },
);
if (renderResult.status !== 0) {
  console.error("compile:cargo: rustdoc normalization failed.");
  process.exit(renderResult.status ?? 1);
}
