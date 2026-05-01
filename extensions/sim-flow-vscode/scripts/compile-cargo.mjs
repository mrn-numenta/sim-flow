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

const result = spawnSync(
  "cargo",
  ["build", "--release", "-p", "sim-flow"],
  { cwd: repoRoot, stdio: "inherit" },
);
if (result.status !== 0) {
  console.error("compile:cargo: cargo build failed.");
  process.exit(result.status ?? 1);
}
