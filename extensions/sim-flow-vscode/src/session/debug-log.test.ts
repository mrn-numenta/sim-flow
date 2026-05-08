import * as fs from "node:fs";
import * as path from "node:path";
import { tmpdir } from "node:os";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { categoriesAny, DebugLog, parseCategories } from "./debug-log";

let projectDir: string;

beforeEach(() => {
  projectDir = fs.mkdtempSync(path.join(tmpdir(), "sim-flow-debuglog-"));
});

afterEach(() => {
  fs.rmSync(projectDir, { recursive: true, force: true });
});

const logPath = (): string =>
  path.join(projectDir, ".sim-flow", "logs", "extension-chat.log");

function readLog(): string {
  return fs.existsSync(logPath()) ? fs.readFileSync(logPath(), "utf8") : "";
}

describe("parseCategories", () => {
  it("returns all-false for undefined / empty", () => {
    expect(parseCategories(undefined)).toEqual({ events: false, raw: false, llm: false });
    expect(parseCategories("")).toEqual({ events: false, raw: false, llm: false });
    expect(parseCategories("   ")).toEqual({ events: false, raw: false, llm: false });
  });

  it("parses individual tokens", () => {
    expect(parseCategories("events")).toEqual({ events: true, raw: false, llm: false });
    expect(parseCategories("raw")).toEqual({ events: false, raw: true, llm: false });
    expect(parseCategories("llm")).toEqual({ events: false, raw: false, llm: true });
  });

  it("supports `1` and `true` shortcuts (events + llm)", () => {
    expect(parseCategories("1")).toEqual({ events: true, raw: false, llm: true });
    expect(parseCategories("true")).toEqual({ events: true, raw: false, llm: true });
  });

  it("supports the `all` shortcut", () => {
    expect(parseCategories("all")).toEqual({ events: true, raw: true, llm: true });
  });

  it("merges multiple comma-separated tokens", () => {
    expect(parseCategories("events,raw")).toEqual({ events: true, raw: true, llm: false });
    expect(parseCategories(" events , llm ")).toEqual({
      events: true,
      raw: false,
      llm: true,
    });
  });

  it("ignores unknown tokens with a warning (no throw)", () => {
    // The implementation prints a console.warn; we just check the
    // result is unaffected.
    expect(parseCategories("garbage,events")).toEqual({
      events: true,
      raw: false,
      llm: false,
    });
  });
});

describe("categoriesAny", () => {
  it("returns false when no categories are enabled", () => {
    expect(categoriesAny({ events: false, raw: false, llm: false })).toBe(false);
  });

  it("returns true when any category is enabled", () => {
    expect(categoriesAny({ events: true, raw: false, llm: false })).toBe(true);
    expect(categoriesAny({ events: false, raw: true, llm: false })).toBe(true);
    expect(categoriesAny({ events: false, raw: false, llm: true })).toBe(true);
  });
});

describe("DebugLog (disabled)", () => {
  it("does not create files or directories when no categories are set", () => {
    const log = DebugLog.fromTokens("", projectDir);
    log.logEventIn({ event: "session-end", reason: "completed" } as never);
    log.logRawIn("anything");
    log.logLlmEnd(0, 0);
    log.logProcessSpawn("/bin/sim-flow", ["auto"], 1234);
    log.dispose();
    expect(fs.existsSync(path.join(projectDir, ".sim-flow"))).toBe(false);
  });
});

describe("DebugLog (events category)", () => {
  it("creates the log file with a session banner", () => {
    const log = DebugLog.fromTokens("events", projectDir);
    expect(fs.existsSync(logPath())).toBe(true);
    const text = readLog();
    expect(text).toMatch(/## Session started at/);
    log.dispose();
  });

  it("logs incoming and outgoing events but not raw lines", () => {
    const log = DebugLog.fromTokens("events", projectDir);
    log.logEventIn({ event: "hello-ack" } as never);
    log.logEventOut({ event: "hello" } as never);
    log.logRawIn("ignored");
    log.logRawOut("ignored");
    log.dispose();
    const text = readLog();
    expect(text).toContain("← hello-ack");
    expect(text).toContain("→ hello");
    expect(text).not.toContain("ignored");
  });
});

describe("DebugLog (raw category)", () => {
  it("logs raw lines but not parsed event sections", () => {
    const log = DebugLog.fromTokens("raw", projectDir);
    log.logRawIn("incoming line\n");
    log.logRawOut("outgoing line");
    log.logEventIn({ event: "hello-ack" } as never);
    log.dispose();
    const text = readLog();
    expect(text).toContain("raw← `incoming line`");
    expect(text).toContain("raw→ `outgoing line`");
    expect(text).not.toContain("← hello-ack");
  });
});

describe("DebugLog (llm category)", () => {
  it("logs dispatch / chunk / end / error markers", () => {
    const log = DebugLog.fromTokens("llm", projectDir);
    log.logLlmDispatch([
      { role: "system", content: "be helpful" },
      { role: "user", content: "hi" },
    ]);
    log.logLlmChunk("partial text");
    log.logLlmEnd(12, 1);
    log.logLlmError(new Error("boom"));
    log.dispose();
    const text = readLog();
    expect(text).toContain("llm→ dispatch (2 message(s))");
    expect(text).toContain("[0] system");
    expect(text).toContain("be helpful");
    expect(text).toContain("[1] user");
    expect(text).toContain("hi");
    expect(text).toContain("llm← chunk (12 chars)");
    expect(text).toContain("partial text");
    expect(text).toContain("llm← end (1 chunk(s), 12 total chars)");
    expect(text).toContain("llm← error");
    expect(text).toContain("boom");
  });

  it("handles a non-Error value in logLlmError", () => {
    const log = DebugLog.fromTokens("llm", projectDir);
    log.logLlmError("a string error");
    log.dispose();
    expect(readLog()).toContain("a string error");
  });

  it("does not log llm markers when only the events category is enabled", () => {
    const log = DebugLog.fromTokens("events", projectDir);
    log.logLlmChunk("should be skipped");
    log.dispose();
    expect(readLog()).not.toContain("should be skipped");
  });
});

describe("DebugLog (process lifecycle markers)", () => {
  it("logs process spawn / exit / spawn-error always when log is open", () => {
    // These methods don't gate on a category -- they fire whenever
    // the log file is open, so the user always has a breadcrumb.
    const log = DebugLog.fromTokens("events", projectDir);
    log.logProcessSpawn("/abs/sim-flow", ["auto", "--llm-backend", "vllm"], 12345);
    log.logSpawnError("ENOENT");
    log.logProcessExit(0, null, "no stderr noise here");
    log.logProcessExit(137, "SIGKILL", "");
    log.dispose();
    const text = readLog();
    expect(text).toContain("process spawned (pid=12345)");
    expect(text).toContain('"/abs/sim-flow"');
    expect(text).toContain('"--llm-backend"');
    expect(text).toContain("process spawn error");
    expect(text).toContain("ENOENT");
    expect(text).toContain("process exited");
    expect(text).toContain("code: 0");
    expect(text).toContain("signal: (none)");
    expect(text).toContain("no stderr noise here");
    expect(text).toContain("code: 137");
    expect(text).toContain("signal: SIGKILL");
    expect(text).toContain("stderr: (empty)");
  });

  it("includes a `(pid=?)` placeholder when pid is undefined", () => {
    const log = DebugLog.fromTokens("events", projectDir);
    log.logProcessSpawn("/abs/sim-flow", ["auto"], undefined);
    log.dispose();
    expect(readLog()).toContain("process spawned (pid=?)");
  });

  it("does not write process markers when the log is disabled", () => {
    const log = DebugLog.fromTokens("", projectDir);
    log.logProcessSpawn("/abs/sim-flow", ["auto"], 1);
    log.logProcessExit(1, null, "");
    log.dispose();
    expect(fs.existsSync(logPath())).toBe(false);
  });
});

describe("DebugLog (timestamp formatting)", () => {
  it("emits a `[+SSS.mmm s]` elapsed marker on every line", async () => {
    const log = DebugLog.fromTokens("events", projectDir);
    // Sleep briefly so the elapsed counter ticks past zero.
    await new Promise((r) => setTimeout(r, 10));
    log.logEventIn({ event: "hello-ack" } as never);
    log.dispose();
    const text = readLog();
    // Pattern: leading whitespace allowed for short elapsed (e.g.
    // "[+  0.012s]" or "[+ 12.345s]"). Confirm the bracket form.
    expect(text).toMatch(/\[\+\s*\d+\.\d{3}s\]/);
  });
});

describe("DebugLog (dispose)", () => {
  it("is idempotent", () => {
    const log = DebugLog.fromTokens("events", projectDir);
    log.dispose();
    log.dispose();
    // No throw == pass.
  });

  it("after dispose, further log calls are silently no-ops", () => {
    const log = DebugLog.fromTokens("events", projectDir);
    log.dispose();
    const before = readLog();
    log.logEventIn({ event: "hello-ack" } as never);
    log.logRawIn("nope");
    log.logLlmChunk("nope");
    expect(readLog()).toBe(before);
  });
});
