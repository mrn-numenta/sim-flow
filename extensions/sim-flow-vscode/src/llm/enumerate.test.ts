import { describe, expect, it, vi } from "vitest";

vi.mock("vscode", () => ({
  lm: {
    selectChatModels: async () => [],
  },
}));

const { enumerateModels } = await import("./enumerate");

function jsonResponse(body: unknown, init: ResponseInit = { status: 200 }): Response {
  return new Response(JSON.stringify(body), {
    status: init.status ?? 200,
    headers: { "content-type": "application/json" },
  });
}

describe("enumerateModels", () => {
  it("vscode source returns vendor-qualified families so duplicates can be picked apart", async () => {
    // Both Copilot and Claude Code publish `claude-sonnet-4.6`; the
    // dropdown has to surface both as distinct picks or the user
    // can't choose which provider's quota they want to spend.
    const fakeLm = {
      selectChatModels: async () => [
        { id: "1", family: "claude-sonnet-4.6", vendor: "copilot", name: "" } as never,
        { id: "2", family: "claude-sonnet-4.6", vendor: "claude-code", name: "" } as never,
        { id: "3", family: "claude-opus-4.7", vendor: "claude-code", name: "" } as never,
        { id: "4", family: "gpt-4o", vendor: "copilot", name: "" } as never,
      ],
    };
    const result = await enumerateModels({ source: "vscode", vscodeLm: fakeLm });
    expect(result.error).toBeUndefined();
    expect(result.emptyReason).toBeUndefined();
    expect(result.models).toEqual([
      "claude-code/claude-opus-4.7",
      "claude-code/claude-sonnet-4.6",
      "copilot/claude-sonnet-4.6",
      "copilot/gpt-4o",
    ]);
  });

  it("vscode source flags empty registries", async () => {
    const result = await enumerateModels({
      source: "vscode",
      vscodeLm: { selectChatModels: async () => [] },
    });
    expect(result.models).toEqual([]);
    expect(result.emptyReason).toContain("No chat-model providers");
  });

  it("lmstudio source GETs /models from the configured base URL", async () => {
    let seenUrl: string | undefined;
    const fakeFetch = (async (url: string) => {
      seenUrl = url;
      return jsonResponse({
        data: [
          { id: "qwen/qwen3-coder-next" },
          { id: "text-embedding-nomic-embed-text-v1.5" },
        ],
      });
    }) as unknown as typeof fetch;
    const result = await enumerateModels({
      source: "lmstudio",
      fetchImpl: fakeFetch,
    });
    expect(seenUrl).toBe("http://localhost:1234/v1/models");
    expect(result.models).toEqual([
      "qwen/qwen3-coder-next",
      "text-embedding-nomic-embed-text-v1.5",
    ]);
  });

  it("lmstudio source surfaces a non-2xx response as error", async () => {
    const fakeFetch = (async () =>
      new Response("nope", { status: 503, statusText: "down" })) as unknown as typeof fetch;
    const result = await enumerateModels({ source: "lmstudio", fetchImpl: fakeFetch });
    expect(result.models).toEqual([]);
    expect(result.error).toContain("503");
  });

  it("ollama source uses the configured base URL", async () => {
    let seenUrl: string | undefined;
    const fakeFetch = (async (url: string) => {
      seenUrl = url;
      return jsonResponse({ data: [{ id: "llama3.1:8b" }] });
    }) as unknown as typeof fetch;
    await enumerateModels({
      source: "ollama",
      ollamaBaseUrl: "http://ollama.internal:11434/v1",
      fetchImpl: fakeFetch,
    });
    expect(seenUrl).toBe("http://ollama.internal:11434/v1/models");
  });

  it("openai source returns a hardcoded list", async () => {
    const result = await enumerateModels({ source: "openai" });
    expect(result.error).toBeUndefined();
    expect(result.models).toContain("gpt-4o");
    expect(result.models.length).toBeGreaterThan(0);
  });

  it("anthropic source returns a hardcoded list", async () => {
    const result = await enumerateModels({ source: "anthropic" });
    expect(result.error).toBeUndefined();
    expect(result.models).toContain("claude-opus-4-7");
  });

  it("CLI sources return their alias lists", async () => {
    const claude = await enumerateModels({ source: "claude-cli" });
    expect(claude.models).toEqual(["sonnet", "opus", "haiku"]);
    const codex = await enumerateModels({ source: "codex-cli" });
    expect(codex.models.length).toBeGreaterThan(0);
    const gh = await enumerateModels({ source: "gh-copilot-cli" });
    expect(gh.models).toEqual(["(default)"]);
  });

  it("server-reachable-but-empty surfaces emptyReason, not error", async () => {
    const fakeFetch = (async () => jsonResponse({ data: [] })) as unknown as typeof fetch;
    const result = await enumerateModels({ source: "lmstudio", fetchImpl: fakeFetch });
    expect(result.error).toBeUndefined();
    expect(result.models).toEqual([]);
    expect(result.emptyReason).toContain("no models");
  });

  it("payload without a `.data` array is rejected as an unexpected shape", async () => {
    const fakeFetch = (async () => jsonResponse({ items: [] })) as unknown as typeof fetch;
    const result = await enumerateModels({ source: "lmstudio", fetchImpl: fakeFetch });
    expect(result.models).toEqual([]);
    expect(result.error).toContain("unexpected payload");
  });

  it("a network/fetch exception surfaces as `error` instead of throwing", async () => {
    const fakeFetch = (async () => {
      throw new Error("ECONNRESET");
    }) as unknown as typeof fetch;
    const result = await enumerateModels({ source: "lmstudio", fetchImpl: fakeFetch });
    expect(result.models).toEqual([]);
    expect(result.error).toContain("ECONNRESET");
  });

});
