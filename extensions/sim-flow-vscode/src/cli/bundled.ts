// Bundled `sim-flow` binary lookup. The VSIX ships prebuilt binaries
// under `<extensionRoot>/bin/<platform>-<arch>/sim-flow[.exe]`; the
// resolver in `resolve.ts` uses the candidate list produced here when
// neither `sim-flow.binaryPath` nor `$PATH` turns up a usable binary.
//
// Normalized framework API docs ship under
// `<extensionRoot>/foundation-docs/api/`; `bundledFrameworkDocsRoot()`
// returns that root so the orchestrator can expose them via `fw:api/...`.

import * as fs from "node:fs";
import * as path from "node:path";

let bundledRoot: string | undefined;

/**
 * Called from `activate()` with `context.extensionUri.fsPath` so the
 * rest of the code can build bundled-binary candidate paths without
 * carrying the extension context around.
 */
export function setBundledRoot(root: string): void {
  bundledRoot = root;
}

/**
 * Return the list of bundled-binary paths to probe for the current
 * OS/arch, ordered from most to least preferred. Returns an empty
 * list when the extension root has not been registered yet (e.g. in
 * unit tests that never call `activate()`).
 */
export function bundledCandidates(): string[] {
  if (!bundledRoot) {
    return [];
  }
  const dir = platformDir(process.platform, process.arch);
  if (!dir) {
    return [];
  }
  const exe = process.platform === "win32" ? "sim-flow.exe" : "sim-flow";
  return [path.join(bundledRoot, "bin", dir, exe)];
}

/**
 * Path to the bundled normalized framework API docs root, or
 * `undefined` when the extension root is unknown or the packaged docs
 * are missing.
 */
export function bundledFrameworkDocsRoot(): string | undefined {
  if (!bundledRoot) {
    return undefined;
  }
  const candidate = path.join(bundledRoot, "foundation-docs", "api");
  const toc = path.join(candidate, "toc.md");
  return fs.existsSync(toc) ? candidate : undefined;
}

/**
 * Map (platform, arch) to the on-disk subdirectory name that the
 * VSIX bundles binaries under. `null` means we do not currently
 * ship a binary for that combination - the resolver falls through
 * to the "not found" error with a hint to install via `$PATH`.
 *
 * Exposed for tests.
 */
export function platformDir(platform: NodeJS.Platform, arch: string): string | null {
  if (platform === "darwin" && arch === "arm64") {
    return "darwin-arm64";
  }
  if (platform === "darwin" && arch === "x64") {
    return "darwin-x64";
  }
  if (platform === "linux" && arch === "x64") {
    return "linux-x64";
  }
  if (platform === "win32" && arch === "x64") {
    return "win32-x64";
  }
  return null;
}
