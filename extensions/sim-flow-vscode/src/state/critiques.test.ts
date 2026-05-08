import * as fs from "node:fs";
import * as path from "node:path";
import { tmpdir } from "node:os";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  CRITIQUES_DIR,
  critiquePath,
  critiquesDir,
  listCritiqueFiles,
  parseFindings,
  readCritique,
} from "./critiques";

describe("parseFindings", () => {
  it("returns an empty finding list for prose-only markdown", () => {
    const body = "# DM0 Critique\n\nEverything looks good so far.\n";
    const { findings, hasBlocking } = parseFindings(body);
    expect(findings).toEqual([]);
    expect(hasBlocking).toBe(false);
  });

  it("classifies RESOLVED / UNRESOLVED / BLOCKER markers", () => {
    const body = [
      "# DM2c Critique",
      "",
      "## Findings",
      "- RESOLVED: tidied output wiring",
      "- UNRESOLVED: pipeline bubble rate is higher than estimated",
      "- BLOCKER: scoreboard does not verify output ordering",
      "",
    ].join("\n");
    const { findings, hasBlocking } = parseFindings(body);
    expect(findings).toHaveLength(3);
    expect(findings[0]).toMatchObject({ kind: "resolved", text: "tidied output wiring" });
    expect(findings[1]).toMatchObject({ kind: "unresolved" });
    expect(findings[2]).toMatchObject({ kind: "blocker" });
    expect(hasBlocking).toBe(true);
  });

  it("accepts markers without the leading list bullet", () => {
    const body = "UNRESOLVED: raw form\n";
    const { findings, hasBlocking } = parseFindings(body);
    expect(findings).toHaveLength(1);
    expect(findings[0].kind).toBe("unresolved");
    expect(hasBlocking).toBe(true);
  });

  it("handles leading whitespace and `*` list markers", () => {
    const body = "   * BLOCKER: indented starlist\n";
    const { findings } = parseFindings(body);
    expect(findings).toHaveLength(1);
    expect(findings[0].kind).toBe("blocker");
    expect(findings[0].text).toBe("indented starlist");
  });

  it("treats RESOLVED lines as non-blocking", () => {
    const body = "- RESOLVED: fixed a thing\n- RESOLVED: fixed another\n";
    const { findings, hasBlocking } = parseFindings(body);
    expect(findings).toHaveLength(2);
    expect(hasBlocking).toBe(false);
  });

  it("assigns 1-based line numbers", () => {
    const body = ["# header", "", "- UNRESOLVED: line 3", "prose", "- BLOCKER: line 5"].join("\n");
    const { findings } = parseFindings(body);
    expect(findings[0].line).toBe(3);
    expect(findings[1].line).toBe(5);
  });

  it("handles numbered markdown lists with bold-wrapped headings", () => {
    // The shape agents tend to produce when asked for a critique with
    // structured findings.
    const body = [
      "## Issues",
      "",
      "1. **UNRESOLVED: missing coverage for illegal opcodes.**",
      "   Spec defines 10 encodings; behavior on undefined codes is unspecified.",
      "",
      "2. **UNRESOLVED: word_op coverage is implicit.**",
      "   Sign-extension of the 32-bit result is not called out.",
      "",
      "3. **BLOCKER: testbench file missing entirely.**",
      "",
      "4. **RESOLVED: removed dead branch.**",
    ].join("\n");
    const { findings, hasBlocking } = parseFindings(body);
    expect(findings).toHaveLength(4);
    expect(findings.map((f) => f.kind)).toEqual(["unresolved", "unresolved", "blocker", "resolved"]);
    expect(findings[0].text).toBe("missing coverage for illegal opcodes.");
    expect(findings[2].text).toBe("testbench file missing entirely.");
    expect(hasBlocking).toBe(true);
  });
});

describe("path helpers", () => {
  it("CRITIQUES_DIR is the relative docs/critiques path", () => {
    expect(CRITIQUES_DIR).toBe(path.join("docs", "critiques"));
  });

  it("critiquesDir composes the project-rooted absolute path", () => {
    expect(critiquesDir("/abs/p")).toBe(path.join("/abs/p", "docs", "critiques"));
  });

  it("critiquePath uses the `<step>-critique.md` filename convention", () => {
    expect(critiquePath("/abs/p", "DM2c")).toBe(
      path.join("/abs/p", "docs", "critiques", "DM2c-critique.md"),
    );
  });
});

describe("listCritiqueFiles / readCritique", () => {
  let projectDir: string;

  beforeEach(() => {
    projectDir = fs.mkdtempSync(path.join(tmpdir(), "sim-flow-critiques-"));
  });

  afterEach(() => {
    fs.rmSync(projectDir, { recursive: true, force: true });
  });

  it("listCritiqueFiles returns an empty array when the directory is missing", async () => {
    const got = await listCritiqueFiles(projectDir);
    expect(got).toEqual([]);
  });

  it("listCritiqueFiles returns parsed entries for every `<step>-critique.md` file", async () => {
    const dir = critiquesDir(projectDir);
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, "DM0-critique.md"), "- BLOCKER: nope\n", "utf8");
    fs.writeFileSync(
      path.join(dir, "DM1-critique.md"),
      "- RESOLVED: addressed earlier\n",
      "utf8",
    );
    // Non-critique files should be ignored.
    fs.writeFileSync(path.join(dir, "README.md"), "ignored", "utf8");
    const list = await listCritiqueFiles(projectDir);
    expect(list).toHaveLength(2);
    expect(list[0].step).toBe("DM0");
    expect(list[0].hasBlocking).toBe(true);
    expect(list[0].path).toBe(path.join(dir, "DM0-critique.md"));
    expect(list[1].step).toBe("DM1");
    expect(list[1].hasBlocking).toBe(false);
  });

  it("listCritiqueFiles propagates non-ENOENT errors", async () => {
    // Replace the critiques directory with a regular file so
    // readdir returns ENOTDIR (not ENOENT). The helper should
    // re-throw rather than silently returning an empty list.
    fs.mkdirSync(path.join(projectDir, "docs"), { recursive: true });
    fs.writeFileSync(critiquesDir(projectDir), "not a dir", "utf8");
    await expect(listCritiqueFiles(projectDir)).rejects.toThrow();
  });

  it("readCritique returns null when the file is missing", async () => {
    expect(await readCritique(projectDir, "DM2c")).toBeNull();
  });

  it("readCritique parses an existing file", async () => {
    const p = critiquePath(projectDir, "DM3a");
    fs.mkdirSync(path.dirname(p), { recursive: true });
    fs.writeFileSync(
      p,
      ["# DM3a critique", "", "- BLOCKER: missing scoreboard", ""].join("\n"),
      "utf8",
    );
    const got = await readCritique(projectDir, "DM3a");
    expect(got).not.toBeNull();
    expect(got!.step).toBe("DM3a");
    expect(got!.hasBlocking).toBe(true);
    expect(got!.findings.map((f) => f.kind)).toEqual(["blocker"]);
  });

  it("readCritique propagates non-ENOENT errors", async () => {
    // Replace the file path with a directory so readFile fails
    // with EISDIR rather than ENOENT.
    const p = critiquePath(projectDir, "DM3b");
    fs.mkdirSync(p, { recursive: true });
    await expect(readCritique(projectDir, "DM3b")).rejects.toThrow();
  });
});
