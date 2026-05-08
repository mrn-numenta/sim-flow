import * as fs from "node:fs";
import * as path from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { tmpdir } from "node:os";

import { acquirePumpLock, type AcquireFailure, type AcquireResult } from "./pumpLock";

let projectDir: string;

beforeEach(() => {
  projectDir = fs.mkdtempSync(path.join(tmpdir(), "sim-flow-pumplock-"));
});

afterEach(() => {
  fs.rmSync(projectDir, { recursive: true, force: true });
});

const lockPath = (): string => path.join(projectDir, ".sim-flow", ".pump.lock");

function expectAcquired(
  result: AcquireResult | AcquireFailure,
): asserts result is AcquireResult {
  if (!result.ok) {
    throw new Error(`expected acquire to succeed, got: ${result.message}`);
  }
}

function expectFailed(
  result: AcquireResult | AcquireFailure,
): asserts result is AcquireFailure {
  if (result.ok) {
    throw new Error("expected acquire to fail, got success");
  }
}

describe("acquirePumpLock", () => {
  it("creates the .sim-flow directory and lock file on first acquire", () => {
    const result = acquirePumpLock(projectDir, "session-1");
    expectAcquired(result);
    expect(fs.existsSync(lockPath())).toBe(true);
    const record = JSON.parse(fs.readFileSync(lockPath(), "utf8")) as {
      pid: number;
      sessionId: string;
      acquiredAtMs: number;
      nonce: string;
    };
    expect(record.pid).toBe(process.pid);
    expect(record.sessionId).toBe("session-1");
    expect(typeof record.nonce).toBe("string");
    expect(record.nonce).toMatch(/^[0-9a-f]{16}$/);
    expect(typeof record.acquiredAtMs).toBe("number");
  });

  it("works when the .sim-flow directory already exists", () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"), { recursive: true });
    const result = acquirePumpLock(projectDir, "session-2");
    expectAcquired(result);
    expect(fs.existsSync(lockPath())).toBe(true);
  });

  it("release() removes the lock file and is idempotent", () => {
    const result = acquirePumpLock(projectDir, "session-3");
    expectAcquired(result);
    expect(fs.existsSync(lockPath())).toBe(true);
    result.lock.release();
    expect(fs.existsSync(lockPath())).toBe(false);
    // Second release: must not throw, must not recreate.
    result.lock.release();
    expect(fs.existsSync(lockPath())).toBe(false);
  });

  it("after release, the same project can be re-acquired", () => {
    const first = acquirePumpLock(projectDir, "first");
    expectAcquired(first);
    first.lock.release();
    const second = acquirePumpLock(projectDir, "second");
    expectAcquired(second);
    expect(fs.existsSync(lockPath())).toBe(true);
    const record = JSON.parse(fs.readFileSync(lockPath(), "utf8")) as { sessionId: string };
    expect(record.sessionId).toBe("second");
  });

  it("rejects when a live holder pid different from ours owns the lock", () => {
    // Plant a record claiming pid=1 (init -- always alive on POSIX,
    // and almost certainly not us). The acquire should treat that as
    // a live foreign holder.
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(
      lockPath(),
      JSON.stringify({
        pid: 1,
        sessionId: "the-other-window",
        acquiredAtMs: Date.now(),
        nonce: "deadbeefdeadbeef",
      }),
      "utf8",
    );
    const result = acquirePumpLock(projectDir, "ours");
    expectFailed(result);
    expect(result.holderPid).toBe(1);
    expect(result.holderSessionId).toBe("the-other-window");
    expect(result.message).toMatch(/Another sim-flow pump is running/);
    // Existing file must be left alone -- we don't steal a live holder's lock.
    expect(fs.existsSync(lockPath())).toBe(true);
  });

  it("reclaims a lock written by a dead pid (extension-host crash)", () => {
    // Pid 0x7FFFFFFF is well beyond any plausible live process id.
    // `isProcessAlive` returns false; `acquirePumpLock` should
    // unlink and retake.
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    const stalePid = 0x7fffffff;
    fs.writeFileSync(
      lockPath(),
      JSON.stringify({
        pid: stalePid,
        sessionId: "crashed",
        acquiredAtMs: Date.now() - 60_000,
        nonce: "stalenonce000000",
      }),
      "utf8",
    );
    const result = acquirePumpLock(projectDir, "fresh");
    expectAcquired(result);
    const record = JSON.parse(fs.readFileSync(lockPath(), "utf8")) as {
      pid: number;
      sessionId: string;
      nonce: string;
    };
    expect(record.pid).toBe(process.pid);
    expect(record.sessionId).toBe("fresh");
    // A new nonce must be assigned -- the stale one would let the
    // crashed pid's release path silently no-op our owner.
    expect(record.nonce).not.toBe("stalenonce000000");
  });

  it("reclaims a lock that has malformed JSON", () => {
    // Defense-in-depth: a half-written file from a crash should
    // still let us recover instead of failing-fast forever.
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(lockPath(), "{ this is not json", "utf8");
    const result = acquirePumpLock(projectDir, "recovery");
    expectAcquired(result);
    const record = JSON.parse(fs.readFileSync(lockPath(), "utf8")) as { sessionId: string };
    expect(record.sessionId).toBe("recovery");
  });

  it("reclaims a lock that holds our own pid (host re-entry)", () => {
    // Same-pid reclaim path: a previous owner in this process
    // released without unlinking (lost-race scenario). Acquire
    // should still succeed.
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(
      lockPath(),
      JSON.stringify({
        pid: process.pid,
        sessionId: "previous-owner",
        acquiredAtMs: Date.now() - 1_000,
        nonce: "ourselves0000000",
      }),
      "utf8",
    );
    const result = acquirePumpLock(projectDir, "next-owner");
    expectAcquired(result);
    const record = JSON.parse(fs.readFileSync(lockPath(), "utf8")) as { sessionId: string };
    expect(record.sessionId).toBe("next-owner");
  });

  it("release() leaves a successor's lock file alone (nonce guard)", () => {
    const first = acquirePumpLock(projectDir, "old");
    expectAcquired(first);
    // Simulate the race: same project gets re-acquired by someone
    // with a different nonce while `first` still thinks it owns
    // the lock. (In production this would be the same host
    // process taking out a fresh lock after `release()`; here we
    // just rewrite the file directly to keep the test
    // deterministic.)
    fs.writeFileSync(
      lockPath(),
      JSON.stringify({
        pid: process.pid,
        sessionId: "successor",
        acquiredAtMs: Date.now(),
        nonce: "successornonceok",
      }),
      "utf8",
    );
    first.lock.release();
    // Successor's file must still be there.
    expect(fs.existsSync(lockPath())).toBe(true);
    const record = JSON.parse(fs.readFileSync(lockPath(), "utf8")) as { sessionId: string };
    expect(record.sessionId).toBe("successor");
  });

  it("release() tolerates the lock file already being gone", () => {
    const result = acquirePumpLock(projectDir, "session");
    expectAcquired(result);
    fs.unlinkSync(lockPath());
    // Should not throw.
    result.lock.release();
  });

  it("two consecutive acquires assign distinct nonces", () => {
    // The release-path nonce check would be useless if we
    // accidentally generated the same nonce twice.
    const first = acquirePumpLock(projectDir, "a");
    expectAcquired(first);
    const firstNonce = (JSON.parse(fs.readFileSync(lockPath(), "utf8")) as { nonce: string }).nonce;
    first.lock.release();
    const second = acquirePumpLock(projectDir, "b");
    expectAcquired(second);
    const secondNonce = (JSON.parse(fs.readFileSync(lockPath(), "utf8")) as { nonce: string })
      .nonce;
    expect(secondNonce).not.toBe(firstNonce);
  });

  it("rejects records that are missing required fields", () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    // Missing `nonce` -- valid JSON, but not a usable record.
    fs.writeFileSync(
      lockPath(),
      JSON.stringify({
        pid: process.pid,
        sessionId: "partial",
        acquiredAtMs: Date.now(),
      }),
      "utf8",
    );
    // The reader returns null for an incomplete record, so the
    // acquire path treats the file as stale and reclaims it.
    const result = acquirePumpLock(projectDir, "complete");
    expectAcquired(result);
    const record = JSON.parse(fs.readFileSync(lockPath(), "utf8")) as { sessionId: string };
    expect(record.sessionId).toBe("complete");
  });
});
