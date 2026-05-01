import { describe, expect, it } from "vitest";

import { parseFindings } from "./critiques";

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
