import { describe, expect, it } from "vitest";

import { OLLAMA_DEFAULT_BASE_URL, OLLAMA_KEY_ID, OllamaBackend } from "./ollama";
import { sseSingleResponse } from "./sse-test-helpers";
import { LlmError, type SecretStorage } from "./types";

function stubSecrets(map: Record<string, string>): SecretStorage {
  return {
    get: async (k: string) => map[k],
  };
}

function noCancel() {
  return { isCancellationRequested: false };
}

describe("OllamaBackend", () => {
  it("posts to the default local endpoint with no Authorization header when no key is set", async () => {
    let seenUrl: string | undefined;
    let seenHeaders: Record<string, string> | undefined;
    let seenBody: unknown;
    const fakeFetch = (async (url: string, init: RequestInit) => {
      seenUrl = url;
      seenHeaders = init.headers as Record<string, string>;
      seenBody = JSON.parse(init.body as string);
      return sseSingleResponse("hi");
    }) as unknown as typeof fetch;

    const backend = new OllamaBackend({
      fetchImpl: fakeFetch,
      model: "llama3.1:8b",
    });

    const chunks: string[] = [];
    for await (const c of backend.stream([{ role: "user", content: "ping" }], noCancel())) {
      chunks.push(c.text);
    }
    expect(chunks.join("")).toBe("hi");

    expect(seenUrl).toBe(`${OLLAMA_DEFAULT_BASE_URL}/chat/completions`);
    expect(seenHeaders!["content-type"]).toBe("application/json");
    expect(seenHeaders!.authorization).toBeUndefined();
    const body = seenBody as { model: string; messages: Array<{ role: string }> };
    expect(body.model).toBe("llama3.1:8b");
    expect(body.messages).toEqual([{ role: "user", content: "ping" }]);
  });

  it("sends Authorization when a key is stored in SecretStorage", async () => {
    let seenHeaders: Record<string, string> | undefined;
    const fakeFetch = (async (_url: string, init: RequestInit) => {
      seenHeaders = init.headers as Record<string, string>;
      return sseSingleResponse("ok");
    }) as unknown as typeof fetch;

    const backend = new OllamaBackend({
      fetchImpl: fakeFetch,
      secrets: stubSecrets({ [OLLAMA_KEY_ID]: "token-123" }),
    });

    for await (const _ of backend.stream([{ role: "user", content: "hi" }], noCancel())) {
      // drain
    }
    expect(seenHeaders!.authorization).toBe("Bearer token-123");
  });

  it("honors a custom baseUrl", async () => {
    let seenUrl: string | undefined;
    const fakeFetch = (async (url: string) => {
      seenUrl = url;
      return sseSingleResponse("");
    }) as unknown as typeof fetch;

    const backend = new OllamaBackend({
      fetchImpl: fakeFetch,
      baseUrl: "http://ollama.internal:11434/v1/",
    });
    for await (const _ of backend.stream([{ role: "user", content: "x" }], noCancel())) {
      // drain
    }
    expect(seenUrl).toBe("http://ollama.internal:11434/v1/chat/completions");
  });

  it("wraps non-2xx responses in an http LlmError", async () => {
    const fakeFetch = (async () =>
      new Response("no model", {
        status: 404,
        statusText: "not found",
      })) as unknown as typeof fetch;

    const backend = new OllamaBackend({ fetchImpl: fakeFetch });
    try {
      for await (const _ of backend.stream([{ role: "user", content: "x" }], noCancel())) {
        // drain
      }
      throw new Error("expected throw");
    } catch (err) {
      expect(err).toBeInstanceOf(LlmError);
      expect((err as LlmError).kind).toBe("http");
    }
  });
});
