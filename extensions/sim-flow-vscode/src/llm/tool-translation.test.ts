import { describe, expect, it } from "vitest";

import {
  extractToolFences,
  makeToolCallId,
  normalizeToolArgsForOpenAi,
  parseToolResultsEnvelope,
} from "./tool-translation";

describe("extractToolFences", () => {
  it("returns the original text and no calls when there are no fences", () => {
    const r = extractToolFences("just plain text\nwith multiple lines");
    expect(r.toolCalls).toEqual([]);
    expect(r.content).toBe("just plain text\nwith multiple lines");
  });

  it("extracts a single tool fence and strips it from content", () => {
    const text =
      "Let me check the file.\n\n```tool:read_file\n{\"path\":\"docs/spec.md\"}\n```\n";
    const r = extractToolFences(text);
    expect(r.toolCalls).toEqual([
      { name: "read_file", args: '{"path":"docs/spec.md"}' },
    ]);
    expect(r.content).toBe("Let me check the file.");
  });

  it("extracts multiple fences in source order", () => {
    const text =
      "First check:\n```tool:list_dir\nsrc/\n```\n\nThen read:\n```tool:read_file\n{\"path\":\"src/lib.rs\"}\n```\n";
    const r = extractToolFences(text);
    expect(r.toolCalls).toEqual([
      { name: "list_dir", args: "src/" },
      { name: "read_file", args: '{"path":"src/lib.rs"}' },
    ]);
    // Both fences removed; surrounding text preserved (trimmed).
    expect(r.content).toBe("First check:\n\nThen read:");
  });

  it("extracts json-fenced tool calls used by fallback backends", () => {
    const text =
      'before\n```json\n{"name":"read_file","arguments":{"path":"docs/spec.md"}}\n```\nafter';
    const r = extractToolFences(text);
    expect(r.toolCalls).toEqual([
      { name: "read_file", args: '{"path":"docs/spec.md"}' },
    ]);
    expect(r.content).toBe("before\nafter");
  });

  it("leaves non-tool json fences in residual content", () => {
    const text = '```json\n{"foo":"bar"}\n```';
    const r = extractToolFences(text);
    expect(r.toolCalls).toEqual([]);
    expect(r.content).toBe(text);
  });

  it("preserves multi-line fence bodies verbatim", () => {
    const text =
      "```tool:edit_file\n{\n  \"path\": \"x\",\n  \"old_string\": \"a\",\n  \"new_string\": \"b\"\n}\n```\n";
    const r = extractToolFences(text);
    expect(r.toolCalls).toHaveLength(1);
    expect(r.toolCalls[0].name).toBe("edit_file");
    expect(r.toolCalls[0].args).toBe(
      '{\n  "path": "x",\n  "old_string": "a",\n  "new_string": "b"\n}',
    );
  });

  it("ignores non-tool fences (regular code blocks)", () => {
    const text = "```rust\nfn main() {}\n```\n";
    const r = extractToolFences(text);
    expect(r.toolCalls).toEqual([]);
    // Non-tool fences are kept as content (they're not OUR fences,
    // they're the model's regular markdown).
    expect(r.content).toBe("```rust\nfn main() {}\n```");
  });

  it("handles a non-JSON args body (path-only form)", () => {
    const text = "```tool:read_file\nsrc/lib.rs\n```\n";
    const r = extractToolFences(text);
    expect(r.toolCalls).toEqual([{ name: "read_file", args: "src/lib.rs" }]);
  });

  it("preserves unclosed fences in residual content rather than dropping them", () => {
    const text = "```tool:read_file\nsrc/lib.rs\n";
    const r = extractToolFences(text);
    expect(r.toolCalls).toEqual([]);
    expect(r.content).toContain("```tool:read_file");
    expect(r.content).toContain("src/lib.rs");
  });
});

describe("parseToolResultsEnvelope", () => {
  it("returns null for plain user content", () => {
    expect(parseToolResultsEnvelope("Hi, please run the next step.")).toBeNull();
  });

  it("returns null when the content is just the header without a body", () => {
    expect(parseToolResultsEnvelope("Tool results:")).toBeNull();
    expect(parseToolResultsEnvelope("Tool results:\n\n")).toBeNull();
  });

  it("splits a single-section results message", () => {
    const content = "Tool results:\n\n[read_file `docs/spec.md`]\n\n# Spec\n\nClock: 2 GHz\n";
    const sections = parseToolResultsEnvelope(content);
    expect(sections).toEqual([
      "[read_file `docs/spec.md`]\n\n# Spec\n\nClock: 2 GHz",
    ]);
  });

  it("splits multi-section results on the orchestrator's separator", () => {
    const content =
      "Tool results:\n\n[list_dir `src/`]\n\n- lib.rs\n- main.rs\n\n---\n\n[read_file `src/lib.rs`]\n\nfn main() {}\n";
    const sections = parseToolResultsEnvelope(content);
    expect(sections).toEqual([
      "[list_dir `src/`]\n\n- lib.rs\n- main.rs",
      "[read_file `src/lib.rs`]\n\nfn main() {}",
    ]);
  });
});

describe("normalizeToolArgsForOpenAi", () => {
  it("passes valid JSON through verbatim", () => {
    expect(normalizeToolArgsForOpenAi('{"path":"x"}')).toBe('{"path":"x"}');
  });

  it("wraps non-JSON bodies as { raw: ... }", () => {
    expect(normalizeToolArgsForOpenAi("docs/spec.md")).toBe(
      JSON.stringify({ raw: "docs/spec.md" }),
    );
  });

  it("returns {} for empty bodies", () => {
    expect(normalizeToolArgsForOpenAi("")).toBe("{}");
    expect(normalizeToolArgsForOpenAi("   \n  ")).toBe("{}");
  });
});

describe("makeToolCallId", () => {
  it("is deterministic for the same (messageIndex, callIndex)", () => {
    expect(makeToolCallId(3, 1)).toBe("call_3_1");
    expect(makeToolCallId(3, 1)).toBe("call_3_1");
  });

  it("differs across positions", () => {
    const ids = new Set([
      makeToolCallId(0, 0),
      makeToolCallId(0, 1),
      makeToolCallId(1, 0),
    ]);
    expect(ids.size).toBe(3);
  });
});
