// Shared child-process environment helpers. Centralized here so the
// orchestrator pump (SocketSessionPump) and the one-shot `sim-flow new`
// / gate / runs / etc. CLI invocations (cli/executor.ts) inherit the
// same PATH augmentations. Without these the spawned children fail
// the first time they `Command::new("cargo")` -- VS Code launched
// from Finder/Dock inherits the GUI environment, which doesn't load
// the user's shell login dotfiles, so `~/.cargo/bin` is missing from
// `process.env.PATH`.

import * as os from "node:os";
import * as path from "node:path";

/**
 * Prepend likely cargo install locations to an inherited PATH:
 *
 *   - `~/.cargo/bin` (rustup default)
 *   - `/usr/local/bin` (macOS system Rust + Intel Homebrew)
 *   - `/usr/bin` (Linux system Rust)
 *   - `/opt/homebrew/bin` (Apple Silicon Homebrew)
 *
 * Duplicates are de-duplicated so re-spawning doesn't keep growing
 * the PATH. The caller passes the inherited PATH explicitly
 * (typically `process.env.PATH`) and merges the result back into
 * the child's env.
 */
export function augmentPathForCargo(inheritedPath: string | undefined): string {
  const sep = process.platform === "win32" ? ";" : ":";
  const additions = [
    path.join(os.homedir(), ".cargo", "bin"),
    process.platform === "darwin" ? "/usr/local/bin" : "/usr/bin",
    "/opt/homebrew/bin",
  ];
  const existing = (inheritedPath ?? "").split(sep).filter((p) => p.length > 0);
  const merged: string[] = [];
  const seen = new Set<string>();
  for (const dir of [...additions, ...existing]) {
    if (seen.has(dir)) {
      continue;
    }
    seen.add(dir);
    merged.push(dir);
  }
  return merged.join(sep);
}
