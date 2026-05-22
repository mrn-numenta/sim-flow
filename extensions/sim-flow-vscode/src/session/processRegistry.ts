// Per-project on-disk registry of sim-flow child processes the
// extension has spawned. Each pump writes a JSON record with the
// child's PID + minimal identifying metadata; clean shutdowns remove
// the record. On extension activate we walk every known project,
// terminate any leftover processes (verified to look like sim-flow
// to avoid SIGTERM-ing arbitrary PIDs after recycling), and reap the
// stale records.
//
// Why this exists: VS Code can crash, the user can `kill -9` the
// extension host, the OS can reboot, etc. — at which point the
// pumps' in-memory child references vanish but the spawned
// sim-flow processes keep running, attached to no one. Re-launching
// then leaves duplicates running until the user notices and kills
// them by hand. The registry catches these on the next extension
// startup.

import { execSync } from "node:child_process";
import { promises as fsp } from "node:fs";
import * as fs from "node:fs";
import * as path from "node:path";

const PIDS_SUBDIR = path.join(".sim-flow", "pids");

export interface PidRecord {
  /** OS pid we want to track. */
  pid: number;
  /** Session id the pump generated; also the pid file's basename. */
  sessionId: string;
  /** Absolute path to the sim-flow binary that was launched. */
  binary: string;
  /** Subcommand label, e.g. `auto` / `session DM0.work`. Informational. */
  label: string;
  /** Wall-clock spawn time (ms since epoch). */
  spawnedAtMs: number;
}

export function pidsDir(projectDir: string): string {
  return path.join(projectDir, PIDS_SUBDIR);
}

export function writePidRecord(projectDir: string, record: PidRecord): void {
  const dir = pidsDir(projectDir);
  fs.mkdirSync(dir, { recursive: true });
  const file = path.join(dir, `${record.sessionId}.json`);
  // Sync write so the record is durable BEFORE the caller could
  // crash. The body is small (<200 bytes); the cost is negligible.
  fs.writeFileSync(file, JSON.stringify(record, null, 2), "utf8");
}

export function removePidRecord(projectDir: string, sessionId: string): void {
  const file = path.join(pidsDir(projectDir), `${sessionId}.json`);
  try {
    fs.unlinkSync(file);
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code !== "ENOENT") {
      // Log via console; we don't want a stale-pid cleanup to crash
      // the dispose path.
      console.error(`sim-flow: failed to remove pid record ${file}: ${(err as Error).message}`);
    }
  }
}

export function readPidRecords(projectDir: string): PidRecord[] {
  const dir = pidsDir(projectDir);
  let entries: string[];
  try {
    entries = fs.readdirSync(dir);
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") {
      return [];
    }
    throw err;
  }
  const out: PidRecord[] = [];
  for (const name of entries) {
    if (!name.endsWith(".json")) {
      continue;
    }
    const file = path.join(dir, name);
    try {
      const raw = fs.readFileSync(file, "utf8");
      const parsed = JSON.parse(raw) as Partial<PidRecord>;
      if (
        typeof parsed.pid === "number" &&
        typeof parsed.sessionId === "string" &&
        typeof parsed.binary === "string"
      ) {
        out.push({
          pid: parsed.pid,
          sessionId: parsed.sessionId,
          binary: parsed.binary,
          label: parsed.label ?? "",
          spawnedAtMs: parsed.spawnedAtMs ?? 0,
        });
      }
    } catch {
      // Skip malformed records; cleanup will remove them.
    }
  }
  return out;
}

/**
 * Best-effort liveness probe. `process.kill(pid, 0)` doesn't actually
 * signal — it just validates the pid exists and (on POSIX) we have
 * permission to signal it. EPERM means it exists but we can't touch
 * it (rare in practice — sim-flow children inherit our uid). ESRCH
 * means it's gone.
 */
export function isProcessAlive(pid: number): boolean {
  if (pid <= 0) {
    return false;
  }
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    return (err as NodeJS.ErrnoException).code === "EPERM";
  }
}

/**
 * Read the target process's argv (or as close as the OS gives us).
 * Used to verify a recycled PID isn't pointed at some unrelated
 * program before we SIGTERM it. Returns null when the process is
 * gone or `ps` / `wmic` failed; treat null as "don't kill."
 */
export function processCommandLine(pid: number): string | null {
  try {
    if (process.platform === "win32") {
      const out = execSync(`wmic process where ProcessId=${pid} get CommandLine /format:list`, {
        timeout: 1000,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
      });
      const m = /CommandLine=(.*)/m.exec(out);
      return m ? m[1].trim() : null;
    }
    const out = execSync(`ps -p ${pid} -o args=`, {
      timeout: 1000,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    });
    const trimmed = out.trim();
    return trimmed.length > 0 ? trimmed : null;
  } catch {
    return null;
  }
}

export type KillOutcome = "killed" | "not-running" | "not-ours" | "kill-failed";

/**
 * Kill the process named in `record` if it's still running AND looks
 * like a sim-flow process (binary path matches, or argv contains
 * `sim-flow`). The verification guards against PID recycling — by
 * the time we revisit a stale record at extension activate, the OS
 * may have reassigned the pid to an unrelated program; we don't
 * want to SIGTERM that. Sends SIGTERM only; long-running orphans
 * should respect that and exit. We don't escalate to SIGKILL
 * automatically — if a sim-flow process ignores SIGTERM something
 * is genuinely wrong and the user needs to see it.
 */
export function killIfOurs(record: PidRecord): KillOutcome {
  if (!isProcessAlive(record.pid)) {
    return "not-running";
  }
  const cmd = processCommandLine(record.pid);
  if (cmd === null) {
    return "not-ours";
  }
  const looksLikeOurs = cmd.includes(record.binary) || cmd.includes("sim-flow");
  if (!looksLikeOurs) {
    return "not-ours";
  }
  try {
    process.kill(record.pid, "SIGTERM");
    return "killed";
  } catch {
    return "kill-failed";
  }
}

export interface CleanupSummary {
  /** Records that pointed at a still-running sim-flow we successfully signalled. */
  killed: number;
  /** Records whose pid was already gone. */
  stale: number;
  /** Records whose pid was alive but didn't look like a sim-flow process. */
  skipped: number;
  /** Total records processed (records files always removed regardless). */
  total: number;
  /** Names of any pid files we couldn't remove (rare; kept for diagnostics). */
  removeFailures: string[];
}

/**
 * Walk `<projectDir>/.sim-flow/pids/`, kill orphaned sim-flow
 * processes, and reap every record file regardless. Safe to call
 * on a project with no pid directory (returns a zero summary).
 */
export function cleanupStalePids(projectDir: string): CleanupSummary {
  const summary: CleanupSummary = {
    killed: 0,
    stale: 0,
    skipped: 0,
    total: 0,
    removeFailures: [],
  };
  const records = readPidRecords(projectDir);
  for (const record of records) {
    summary.total += 1;
    switch (killIfOurs(record)) {
      case "killed":
        summary.killed += 1;
        break;
      case "not-running":
        summary.stale += 1;
        break;
      case "not-ours":
        summary.skipped += 1;
        break;
      case "kill-failed":
        // Treat like skipped — leave the record file? No, drop it.
        // The user will notice if the process keeps running.
        summary.skipped += 1;
        break;
    }
    try {
      removePidRecord(projectDir, record.sessionId);
    } catch (err) {
      summary.removeFailures.push(`${record.sessionId}: ${(err as Error).message ?? String(err)}`);
    }
  }
  // Best-effort: also reap an empty pids dir so the next `git status`
  // for the project is quieter.
  try {
    const entries = fs.readdirSync(pidsDir(projectDir));
    if (entries.length === 0) {
      fs.rmdirSync(pidsDir(projectDir));
    }
  } catch {
    // ignore
  }
  return summary;
}

/**
 * Async variant of `cleanupStalePids` that doesn't block the
 * extension activation hook. The kill itself is fast (a single
 * `process.kill` call), but `processCommandLine` shells out and we'd
 * rather not stall activation when the user has dozens of stale
 * records.
 */
export async function cleanupStalePidsAsync(projectDir: string): Promise<CleanupSummary> {
  // The sync helper does sync IO; defer to a microtask so callers
  // can await it without blocking the rest of activate().
  await Promise.resolve();
  const result = cleanupStalePids(projectDir);
  return result;
}

/** Best-effort: ensure the directory exists so callers can stash records. */
export async function ensurePidsDir(projectDir: string): Promise<void> {
  await fsp.mkdir(pidsDir(projectDir), { recursive: true });
}
