import { spawn } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  cleanupStalePids,
  isProcessAlive,
  pidsDir,
  readPidRecords,
  removePidRecord,
  writePidRecord,
} from "./processRegistry";

let tmpRoot: string;
let projectDir: string;

beforeEach(() => {
  tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), "sim-flow-pidreg-"));
  projectDir = path.join(tmpRoot, "proj");
  fs.mkdirSync(projectDir);
});

afterEach(() => {
  fs.rmSync(tmpRoot, { recursive: true, force: true });
});

describe("processRegistry: write/read/remove", () => {
  it("writes a json record under <project>/.sim-flow/pids/", () => {
    writePidRecord(projectDir, {
      pid: 12345,
      sessionId: "session-a",
      binary: "/opt/sim-flow",
      label: "auto",
      spawnedAtMs: 1_700_000_000_000,
    });
    const file = path.join(pidsDir(projectDir), "session-a.json");
    expect(fs.existsSync(file)).toBe(true);
    const parsed = JSON.parse(fs.readFileSync(file, "utf8")) as Record<string, unknown>;
    expect(parsed).toMatchObject({
      pid: 12345,
      sessionId: "session-a",
      binary: "/opt/sim-flow",
    });
  });

  it("reads back every record in the pids dir, ignoring malformed entries", () => {
    writePidRecord(projectDir, {
      pid: 100,
      sessionId: "ok-1",
      binary: "/opt/sim-flow",
      label: "auto",
      spawnedAtMs: 1,
    });
    writePidRecord(projectDir, {
      pid: 200,
      sessionId: "ok-2",
      binary: "/opt/sim-flow",
      label: "session DM0.work",
      spawnedAtMs: 2,
    });
    // Drop a malformed file in the same dir.
    fs.writeFileSync(path.join(pidsDir(projectDir), "junk.json"), "not json", "utf8");

    const records = readPidRecords(projectDir);
    const ids = records.map((r) => r.sessionId).sort();
    expect(ids).toEqual(["ok-1", "ok-2"]);
  });

  it("removes a record by sessionId and treats already-removed as a no-op", () => {
    writePidRecord(projectDir, {
      pid: 100,
      sessionId: "gone",
      binary: "/opt/sim-flow",
      label: "auto",
      spawnedAtMs: 1,
    });
    removePidRecord(projectDir, "gone");
    expect(readPidRecords(projectDir)).toEqual([]);
    // Second call: no throw.
    expect(() => removePidRecord(projectDir, "gone")).not.toThrow();
  });

  it("returns empty when the pids dir doesn't exist", () => {
    expect(readPidRecords(projectDir)).toEqual([]);
  });
});

describe("processRegistry: liveness probe", () => {
  it("reports the running test process as alive", () => {
    expect(isProcessAlive(process.pid)).toBe(true);
  });

  it("reports a definitely-dead pid as not alive", () => {
    // Pid 0 is the kernel scheduler placeholder on POSIX; process.kill
    // rejects it with EPERM on macOS / Linux which we treat as alive.
    // -1 fails ESRCH.
    expect(isProcessAlive(-1)).toBe(false);
  });
});

describe("processRegistry: cleanupStalePids", () => {
  it("deletes records for processes that already exited", () => {
    writePidRecord(projectDir, {
      pid: 1, // init — alive but cmdline definitely doesn't include sim-flow
      sessionId: "init-pid",
      binary: "/opt/sim-flow",
      label: "auto",
      spawnedAtMs: 1,
    });
    writePidRecord(projectDir, {
      pid: 999_999_999, // unlikely to exist
      sessionId: "dead",
      binary: "/opt/sim-flow",
      label: "auto",
      spawnedAtMs: 1,
    });
    const summary = cleanupStalePids(projectDir);
    // pid 1 lives and isn't sim-flow → "skipped"; pid 999999999 is
    // dead → "stale". Both records get removed.
    expect(summary.total).toBe(2);
    expect(summary.stale).toBe(1);
    expect(summary.skipped).toBe(1);
    expect(summary.killed).toBe(0);
    expect(readPidRecords(projectDir)).toEqual([]);
  });

  it("kills records that match a currently-running sim-flow-shaped process", async () => {
    if (process.platform === "win32") {
      // Skip — the test relies on a POSIX `sleep` binary as a stand-in
      // for sim-flow that we can match by command line.
      return;
    }
    // Spawn a long-running placeholder we can match on by inserting
    // the literal string "sim-flow" into argv0. We do that via
    // `bash -c 'exec -a sim-flow-test sleep 30'` — `exec -a` rewrites
    // argv[0] so `ps -o args=` reports the chosen name.
    const child = spawn("/bin/bash", ["-c", "exec -a sim-flow-test sleep 30"], {
      detached: false,
      stdio: "ignore",
    });
    expect(child.pid).toBeGreaterThan(0);
    try {
      writePidRecord(projectDir, {
        pid: child.pid as number,
        sessionId: "spawned",
        binary: "/path/to/sim-flow", // doesn't have to match exactly; "sim-flow" substring is enough
        label: "auto",
        spawnedAtMs: Date.now(),
      });

      const summary = cleanupStalePids(projectDir);
      expect(summary.total).toBe(1);
      expect(summary.killed).toBe(1);
      expect(readPidRecords(projectDir)).toEqual([]);
      // Wait for the child to actually exit (SIGTERM is async).
      await new Promise<void>((resolve) => {
        if (child.exitCode !== null || child.signalCode !== null) {
          resolve();
          return;
        }
        child.once("exit", () => resolve());
      });
      expect(isProcessAlive(child.pid as number)).toBe(false);
    } finally {
      // Belt-and-braces in case the SIGTERM didn't take.
      try {
        child.kill("SIGKILL");
      } catch {
        // already gone
      }
    }
  });
});
