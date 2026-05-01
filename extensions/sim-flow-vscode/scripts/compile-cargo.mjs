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
// extDir = <repo>/tools/sim-flow/extensions/sim-flow-vscode
// → ../../../.. = <repo>. Mirrors bundle-bin.mjs.
const repoRoot = resolve(extDir, "..", "..", "..", "..");
const rustdocTargetDir = resolve(repoRoot, "target", "sim-flow-vscode-rustdoc");
const normalizedDocsDir = resolve(repoRoot, "target", "sim-flow-vscode-api-docs");

const buildResult = spawnSync(
  "cargo",
  ["build", "--release", "-p", "sim-flow"],
  { cwd: repoRoot, stdio: "inherit" },
);
if (buildResult.status !== 0) {
  console.error("compile:cargo: cargo build failed.");
  process.exit(buildResult.status ?? 1);
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
