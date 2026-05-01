import { describe, expect, it } from "vitest";

import { extractOpenAiText, OpenAiBackend } from "./openai";
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

describe("extractOpenAiText", () => {
  it("returns the first choice's message content", () => {
    const payload = {
      choices: [{ message: { content: "hello" } }, { message: { content: "ignored" } }],
    };
    expect(extractOpenAiText(payload)).toBe("hello");
  });

  it("returns empty string when the shape is wrong", () => {
    expect(extractOpenAiText(null)).toBe("");
    expect(extractOpenAiText({})).toBe("");
    expect(extractOpenAiText({ choices: [] })).toBe("");
    expect(extractOpenAiText({ choices: [{ message: {} }] })).toBe("");
    expect(extractOpenAiText({ choices: [{ message: { content: 42 } }] })).toBe("");
  });
});

describe("OpenAiBackend", () => {
  it("throws missing-api-key when secrets are absent", async () => {
    const backend = new OpenAiBackend({});
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

  it("forwards messages as-is and yields the extracted text", async () => {
    let seenBody: unknown;
    let seenAuth: string | undefined;
    const fakeFetch = (async (_url: string, init: RequestInit) => {
      seenBody = JSON.parse(init.body as string);
      seenAuth = (init.headers as Record<string, string>)["authorization"];
      return sseSingleResponse("pong");
    }) as unknown as typeof fetch;

    const backend = new OpenAiBackend({
      secrets: stubSecrets({ "sim-flow.openai.apiKey": "sk-test" }),
      fetchImpl: fakeFetch,
      model: "gpt-x",
    });

    const chunks: string[] = [];
    for await (const c of backend.stream(
      [
        { role: "system", content: "be nice" },
        { role: "user", content: "ping" },
      ],
      noCancel(),
    )) {
      chunks.push(c.text);
    }
    expect(chunks.join("")).toBe("pong");

    expect(seenAuth).toBe("Bearer sk-test");
    const body = seenBody as {
      model: string;
      messages: Array<{ role: string; content: string }>;
    };
    expect(body.model).toBe("gpt-x");
    expect(body.messages).toEqual([
      { role: "system", content: "be nice" },
      { role: "user", content: "ping" },
    ]);
  });

  it("wraps non-2xx responses in an http LlmError", async () => {
    const fakeFetch = (async () =>
      new Response("unauthorized", { status: 401, statusText: "no" })) as unknown as typeof fetch;

    const backend = new OpenAiBackend({
      secrets: stubSecrets({ "sim-flow.openai.apiKey": "sk" }),
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
    }
  });
});
