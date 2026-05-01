// Bundled `sim-flow` binary lookup. The VSIX ships prebuilt binaries
// under `<extensionRoot>/bin/<platform>-<arch>/sim-flow[.exe]`; the
// resolver in `resolve.ts` uses the candidate list produced here when
// neither `sim-flow.binaryPath` nor `$PATH` turns up a usable binary.
//
// libpdfium ships in `<extensionRoot>/bin/<platform>-<arch>/<libname>`
// alongside sim-flow; `bundledPdfiumLibPath()` returns the path so the
// extension can set `SIM_FLOW_PDFIUM_LIB_PATH` when spawning the CLI.

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
 * Path to the bundled libpdfium for the current platform, or
 * `undefined` if the extension root hasn't been registered yet, the
 * platform isn't one we bundle for, or the file isn't present (e.g.
 * a dev build that hasn't run `npm run package`). Used by the
 * SessionPump to set `SIM_FLOW_PDFIUM_LIB_PATH` when spawning
 * `sim-flow auto`.
 */
export function bundledPdfiumLibPath(): string | undefined {
  if (!bundledRoot) {
    return undefined;
  }
  const dir = platformDir(process.platform, process.arch);
  if (!dir) {
    return undefined;
  }
  const libname =
    process.platform === "win32"
      ? "pdfium.dll"
      : process.platform === "darwin"
        ? "libpdfium.dylib"
        : "libpdfium.so";
  const candidate = path.join(bundledRoot, "bin", dir, libname);
  return fs.existsSync(candidate) ? candidate : undefined;
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
