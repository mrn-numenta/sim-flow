// Binary resolution: setting override -> PATH lookup -> bundled
// binary (populated at activate-time via `setBundledRoot`). If none
// of the three produce a usable executable, `resolveBinary` throws a
// `SimFlowCliError` the caller can surface as a notification.

import { accessSync, constants as fsConstants } from "node:fs";
import { delimiter } from "node:path";

import { SimFlowCliError } from "./errors";

export interface ResolveOptions {
  /** Value of the `sim-flow.binaryPath` VS Code setting. Empty string = unset. */
  settingOverride?: string;
  /** Value of `process.env.PATH` at resolution time. Injectable for tests. */
  pathEnv?: string;
  /**
   * Factory producing the candidate list of bundled-binary paths.
   * `bundled.ts` derives this from `<extensionRoot>/bin/<os>-<arch>`.
   */
  bundledCandidates?: () => string[];
  /** Swap this out in tests to avoid hitting the real filesystem. */
  exists?: (path: string) => boolean;
}

export function resolveBinary(options: ResolveOptions = {}): string {
  const exists = options.exists ?? defaultExists;

  const setting = options.settingOverride?.trim();
  if (setting && setting.length > 0) {
    if (!exists(setting)) {
      throw new SimFlowCliError(
        `sim-flow.binaryPath points at ${setting}, which does not exist or is not executable.`,
        { kind: "binary-not-found", command: setting },
      );
    }
    return setting;
  }

  const pathEnv = options.pathEnv ?? process.env.PATH ?? "";
  const pathHit = lookupOnPath("sim-flow", pathEnv, exists);
  if (pathHit) {
    return pathHit;
  }

  const bundled = options.bundledCandidates?.() ?? [];
  for (const candidate of bundled) {
    if (exists(candidate)) {
      return candidate;
    }
  }

  throw new SimFlowCliError(
    "sim-flow binary could not be resolved: no `sim-flow.binaryPath` setting, no `sim-flow` on $PATH, and no bundled binary matched this platform. Install sim-flow (`cargo install --path tools/sim-flow`) or point `sim-flow.binaryPath` at an existing build.",
    { kind: "binary-not-found" },
  );
}

function lookupOnPath(
  name: string,
  pathEnv: string,
  exists: (path: string) => boolean,
): string | null {
  if (!pathEnv) {
    return null;
  }
  for (const dir of pathEnv.split(delimiter)) {
    if (!dir) {
      continue;
    }
    const candidate = join(dir, name);
    if (exists(candidate)) {
      return candidate;
    }
  }
  return null;
}

function join(dir: string, name: string): string {
  return dir.endsWith("/") || dir.endsWith("\\") ? `${dir}${name}` : `${dir}/${name}`;
}

function defaultExists(path: string): boolean {
  try {
    accessSync(path, fsConstants.F_OK | fsConstants.X_OK);
    return true;
  } catch {
    return false;
  }
}
