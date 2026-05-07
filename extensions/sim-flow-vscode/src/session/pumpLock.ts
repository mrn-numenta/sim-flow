// Per-project flock for the JSONL pump. Two VS Code windows opening
// the same sim-foundation project would otherwise race -- both spawn
// `sim-flow auto`, both try to bind the same socket path, both fight
// over `.sim-flow/state.toml`. The lock file pins ownership to one
// extension host at a time. Stale lock files (left behind by an
// extension-host crash) are detected via pid liveness and reclaimed.

import * as crypto from "node:crypto";
import * as fs from "node:fs";
import * as path from "node:path";

import { isProcessAlive } from "./processRegistry";

const LOCK_FILE = path.join(".sim-flow", ".pump.lock");

interface LockRecord {
  /** Extension-host pid that holds the lock. */
  pid: number;
  /** Pump session id (informational, for diagnostics). */
  sessionId: string;
  /** Wall-clock acquisition time, ms since epoch. */
  acquiredAtMs: number;
  /**
   * Random per-acquisition tag. The release path requires the
   * file's nonce to match the one we wrote at acquire -- this
   * prevents stealing a successor lock (same sessionId, different
   * acquisition) when our dispose runs after a sibling already
   * re-acquired in the same host.
   */
  nonce: string;
}

export interface PumpLock {
  /** Drop the lock. Idempotent. */
  release(): void;
}

export interface AcquireResult {
  ok: true;
  lock: PumpLock;
}

export interface AcquireFailure {
  ok: false;
  /** Pid currently holding the lock (live process). */
  holderPid: number;
  /** Session id from the existing record (informational). */
  holderSessionId: string;
  /** Human-readable explanation suitable for a Diagnostic. */
  message: string;
}

/**
 * Try to acquire the per-project pump lock. Non-blocking: returns
 * immediately, either with a lock object or with a failure record
 * describing the live holder.
 *
 * The lock file lives at `<project>/.sim-flow/.pump.lock` and is
 * cleaned up by `release()` (called from `SocketSessionPump.dispose`).
 * If a previous extension host crashed without releasing, the next
 * acquire detects the dead pid via `isProcessAlive` and reclaims the
 * file.
 */
export function acquirePumpLock(
  projectDir: string,
  sessionId: string,
): AcquireResult | AcquireFailure {
  const lockPath = path.join(projectDir, LOCK_FILE);
  try {
    fs.mkdirSync(path.dirname(lockPath), { recursive: true });
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code !== "EEXIST") {
      throw err;
    }
  }

  // Two passes: try to claim, and if `wx` says EEXIST, inspect the
  // existing record. If it's stale, remove it and retry once. We
  // don't loop indefinitely -- a third collision means another
  // window won the race during step 2.
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const nonce = crypto.randomBytes(8).toString("hex");
    try {
      const record: LockRecord = {
        pid: process.pid,
        sessionId,
        acquiredAtMs: Date.now(),
        nonce,
      };
      const fd = fs.openSync(lockPath, "wx");
      try {
        fs.writeFileSync(fd, JSON.stringify(record, null, 2), "utf8");
      } finally {
        fs.closeSync(fd);
      }
      return { ok: true, lock: makeLock(lockPath, nonce) };
    } catch (err) {
      if ((err as NodeJS.ErrnoException).code !== "EEXIST") {
        throw err;
      }
    }
    // Existing file -- inspect and possibly reclaim.
    const existing = readLockRecord(lockPath);
    if (existing && isProcessAlive(existing.pid) && existing.pid !== process.pid) {
      return {
        ok: false,
        holderPid: existing.pid,
        holderSessionId: existing.sessionId,
        message:
          `Another sim-flow pump is running for this project (pid ${existing.pid}, session \`${existing.sessionId}\`). ` +
          "Disconnect it from the other window before launching a second flow here.",
      };
    }
    // Either the holder pid is dead (stale, e.g. extension-host
    // crash) or the holder is our own process (host re-entry after
    // the previous owner released without the file being removed --
    // can happen if the unlink lost a race but wx then succeeded).
    // Remove and retry once.
    try {
      fs.unlinkSync(lockPath);
    } catch (err) {
      if ((err as NodeJS.ErrnoException).code !== "ENOENT") {
        throw err;
      }
    }
  }
  return {
    ok: false,
    holderPid: -1,
    holderSessionId: "",
    message:
      "Lost the race to acquire the per-project pump lock. Try again in a moment, or check for a runaway sim-flow process in another window.",
  };
}

function makeLock(lockPath: string, ourNonce: string): PumpLock {
  let released = false;
  return {
    release() {
      if (released) {
        return;
      }
      released = true;
      try {
        const record = readLockRecord(lockPath);
        if (record && record.nonce !== ourNonce) {
          // Someone re-acquired the lock after we released. Their
          // record overwrote ours; unlinking now would steal their
          // lock. Bail out silently -- the file is theirs.
          return;
        }
        fs.unlinkSync(lockPath);
      } catch (err) {
        if ((err as NodeJS.ErrnoException).code !== "ENOENT") {
          // Don't throw out of dispose paths.
          console.error(`sim-flow: failed to release pump lock: ${(err as Error).message}`);
        }
      }
    },
  };
}

function readLockRecord(lockPath: string): LockRecord | null {
  try {
    const raw = fs.readFileSync(lockPath, "utf8");
    const parsed = JSON.parse(raw) as Partial<LockRecord>;
    if (
      typeof parsed.pid === "number" &&
      typeof parsed.sessionId === "string" &&
      typeof parsed.acquiredAtMs === "number" &&
      typeof parsed.nonce === "string"
    ) {
      return {
        pid: parsed.pid,
        sessionId: parsed.sessionId,
        acquiredAtMs: parsed.acquiredAtMs,
        nonce: parsed.nonce,
      };
    }
    return null;
  } catch {
    return null;
  }
}
