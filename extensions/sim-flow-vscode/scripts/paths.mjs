import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

export const extDir = resolve(here, "..");
// sim-flow is its own repo now; extDir = <repo>/extensions/sim-flow-vscode,
// so the repo root is two levels up. The historical four-level walk was
// for the tools/sim-flow/extensions/sim-flow-vscode layout under
// sim-foundation.
export const repoRoot = resolve(extDir, "..", "..");
export const buildRoot = join(extDir, "build");
export const stageDir = join(buildRoot, "stage");

export function vsixPath(version) {
  return join(buildRoot, `sim-flow-vscode-${version}.vsix`);
}
