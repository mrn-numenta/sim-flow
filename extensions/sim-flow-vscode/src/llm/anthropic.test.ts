import { describe, expect, it } from "vitest";

import { AnthropicBackend, extractAnthropicText } from "./anthropic";
import { LlmError, type SecretStorage } from "./types";

function stubSecrets(map: Record<string, string>): SecretStorage {
  return {
    get: async (k: string) => map[k],
  };
}

function noCancel() {
  return { isCancellationRequested: false };
}

describe("extractAnthropicText", () => {
  it("joins all text parts and ignores non-text blocks", () => {
    const payload = {
      content: [
        { type: "text", text: "hello" },
        { type: "tool_use", id: "x" },
        { type: "text", text: " world" },
      ],
    };
    expect(extractAnthropicText(payload)).toBe("hello world");
  });

  it("returns empty string when the response is malformed", () => {
    expect(extractAnthropicText(null)).toBe("");
    expect(extractAnthropicText({})).toBe("");
    expect(extractAnthropicText({ content: "nope" })).toBe("");
    expect(extractAnthropicText({ content: [{ type: "text" }] })).toBe("");
  });
});

describe("AnthropicBackend", () => {
  it("throws missing-api-key when SecretStorage is absent", async () => {
    const backend = new AnthropicBackend({});
    await expect(async () => {
      for await (const _ of backend.stream([], noCancel())) {
        // drain
      }
    }).rejects.toBeInstanceOf(LlmError);
  });

  it("throws missing-api-key when the secret is empty", async () => {
    const backend = new AnthropicBackend({ secrets: stubSecrets({}) });
    try {
      for await (const _ of backend.stream([], noCancel())) {
        // drain
      }
      throw new Error("expected throw");
    } catch (err) {
      expect(err).toBeInstanceOf(LlmError);
      expect((err as LlmError).kind).toBe("missing-api-key");
    }
  });

  it("sends system messages as the dedicated system field and returns text", async () => {
    let seenBody: unknown;
    const fakeFetch = (async (_url: string, init: RequestInit) => {
      seenBody = JSON.parse(init.body as string);
      return new Response(JSON.stringify({ content: [{ type: "text", text: "ok" }] }), {
        status: 200,
      });
    }) as unknown as typeof fetch;

    const backend = new AnthropicBackend({
      secrets: stubSecrets({ "sim-flow.anthropic.apiKey": "k" }),
      fetchImpl: fakeFetch,
    });

    const chunks: string[] = [];
    for await (const c of backend.stream(
      [
        { role: "system", content: "sys-a" },
        { role: "system", content: "sys-b" },
        { role: "user", content: "hi" },
      ],
      noCancel(),
    )) {
      chunks.push(c.text);
    }
    expect(chunks.join("")).toBe("ok");

    const body = seenBody as {
      system: string;
      messages: Array<{ role: string; content: string }>;
    };
    expect(body.system).toBe("sys-a\n\nsys-b");
    expect(body.messages).toEqual([{ role: "user", content: "hi" }]);
  });

  it("wraps non-2xx responses in an http LlmError", async () => {
    const fakeFetch = (async () =>
      new Response("boom", { status: 500, statusText: "server error" })) as unknown as typeof fetch;

    const backend = new AnthropicBackend({
      secrets: stubSecrets({ "sim-flow.anthropic.apiKey": "k" }),
      fetchImpl: fakeFetch,
    });

    try {
      for await (const _ of backend.stream([{ role: "user", content: "x" }], noCancel())) {
        // drain
      }
      throw new Error("expected throw");
    } catch (err) {
      expect(err).toBeInstanceOf(LlmError);
      expect((err as LlmError).kind).toBe("http");
      expect((err as LlmError).detail).toBe("boom");
    }
  });
});
