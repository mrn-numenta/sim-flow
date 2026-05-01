import { describe, expect, it, vi } from "vitest";

vi.mock("vscode", () => ({
  lm: { selectChatModels: async () => [] },
  CancellationTokenSource: class {
    token = { isCancellationRequested: false };
    cancel(): void {}
  },
  LanguageModelChatMessage: { User: () => null, Assistant: () => null },
  LanguageModelTextPart: class {},
  LanguageModelDataPart: { image: () => null },
  ChatImageMimeType: { JPEG: "image/jpeg", PNG: "image/png", GIF: "image/gif", WEBP: "image/webp" },
}));

const { parseModelHint, extractTextPart, extractToolCallPart } = await import("./vscode");

describe("parseModelHint", () => {
  it("returns nothing for empty / undefined input", () => {
    expect(parseModelHint(undefined)).toEqual({});
    expect(parseModelHint("")).toEqual({});
  });

  it("treats a bare string as a family name (legacy single-field form)", () => {
    expect(parseModelHint("claude-sonnet-4.6")).toEqual({ family: "claude-sonnet-4.6" });
    expect(parseModelHint("gpt-4o")).toEqual({ family: "gpt-4o" });
  });

  it("splits `vendor/family` so the LM selector can scope by vendor", () => {
    // This is the form the dashboard's model dropdown emits when
    // multiple providers offer overlapping families. Without it,
    // Copilot's `claude-sonnet-4.6` shadows Claude Code's and the
    // user gets a "Copilot quota exhausted" error.
    expect(parseModelHint("claude-code/claude-sonnet-4.6")).toEqual({
      vendor: "claude-code",
      family: "claude-sonnet-4.6",
    });
    expect(parseModelHint("copilot/gpt-4o")).toEqual({
      vendor: "copilot",
      family: "gpt-4o",
    });
  });

  it("splits on the FIRST slash so families containing slashes survive", () => {
    // (Hypothetical -- vendor names don't currently contain slashes,
    // and family ids haven't either, but if the LM API ever produces
    // `vendor/some/path` we'd rather lose the slash from the family
    // than from the vendor.)
    expect(parseModelHint("vendor/family/with/slashes")).toEqual({
      vendor: "vendor",
      family: "family/with/slashes",
    });
  });

  it("trims whitespace around vendor and family", () => {
    expect(parseModelHint(" claude-code / claude-sonnet-4.6 ")).toEqual({
      vendor: "claude-code",
      family: "claude-sonnet-4.6",
    });
  });

  it("treats a leading slash as `family only` and a trailing slash as `vendor only`", () => {
    expect(parseModelHint("/family-only")).toEqual({ family: "family-only" });
    expect(parseModelHint("vendor-only/")).toEqual({ vendor: "vendor-only" });
  });
});

describe("extractTextPart", () => {
  it("returns text for the LanguageModelTextPart `.value` shape", () => {
    expect(extractTextPart({ value: "hello" })).toBe("hello");
    expect(extractTextPart({ value: "" })).toBe("");
  });

  it("returns text for the legacy bare-string shape", () => {
    expect(extractTextPart("legacy chunk")).toBe("legacy chunk");
  });

  it("returns null for tool-call parts (which also have .value-like fields)", () => {
    // `name` is the disambiguator -- text parts don't have it.
    expect(extractTextPart({ name: "read_file", input: { path: "x" } })).toBeNull();
  });

  it("returns null for unrecognized shapes", () => {
    expect(extractTextPart(null)).toBeNull();
    expect(extractTextPart(undefined)).toBeNull();
    expect(extractTextPart({})).toBeNull();
    expect(extractTextPart(42)).toBeNull();
  });
});

describe("extractToolCallPart", () => {
  it("renders {name, input: object} as a fenced tool block with JSON body", () => {
    // This is the exact shape Claude Code's provider was emitting
    // when sim-flow's prompts mention tools, which we previously
    // dropped on the floor (since we only iterated `request.text`).
    // Now we surface it as a fenced block so the orchestrator's
    // `extract_tool_calls` dispatcher picks it up.
    const block = extractToolCallPart({
      name: "read_file",
      callId: "call_1",
      input: { path: "src/lib.rs" },
    });
    expect(block).toMatch(/```tool:read_file\n\{"path":"src\/lib\.rs"\}\n```/);
  });

  it("passes through string inputs that already parse as JSON", () => {
    const block = extractToolCallPart({
      name: "edit_file",
      input: '{"path":"a.md","old_string":"x","new_string":"y"}',
    });
    expect(block).toContain('{"path":"a.md","old_string":"x","new_string":"y"}');
  });

  it("wraps non-JSON string inputs as {raw: ...} so the orchestrator gets *something*", () => {
    const block = extractToolCallPart({ name: "search", input: "ConnectivityPlan" });
    expect(block).toContain('{"raw":"ConnectivityPlan"}');
  });

  it("returns null for parts without a name (i.e. text parts)", () => {
    expect(extractToolCallPart({ value: "just text" })).toBeNull();
    expect(extractToolCallPart(null)).toBeNull();
    expect(extractToolCallPart("string")).toBeNull();
  });

  it("emits empty `{}` body when input is missing", () => {
    const block = extractToolCallPart({ name: "list_dir" });
    expect(block).toContain("```tool:list_dir\n{}\n```");
  });
});
