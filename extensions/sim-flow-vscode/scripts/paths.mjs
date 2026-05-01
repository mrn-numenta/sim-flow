import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

export const extDir = resolve(here, "..");
export const repoRoot = resolve(extDir, "..", "..", "..", "..");
export const buildRoot = join(extDir, "build");
export const stageDir = join(buildRoot, "stage");

export function vsixPath(version) {
  return join(buildRoot, `sim-flow-vscode-${version}.vsix`);
}
