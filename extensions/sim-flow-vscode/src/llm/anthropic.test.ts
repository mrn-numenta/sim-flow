import { describe, expect, it } from "vitest";

import { AnthropicBackend, extractAnthropicChunks, extractAnthropicText } from "./anthropic";
import { LlmError, type SecretStorage } from "./types";

function stubSecrets(map: Record<string, string>): SecretStorage {
  return {
    get: async (k: string) => map[k],
  };
}

function noCancel() {
  return { isCancellationRequested: false };
}

function cancellableToken() {
  const listeners = new Set<() => void>();
  return {
    token: {
      isCancellationRequested: false,
      onCancellationRequested(listener: () => void) {
        listeners.add(listener);
        return {
          dispose() {
            listeners.delete(listener);
          },
        };
      },
    },
    cancel() {
      this.token.isCancellationRequested = true;
      for (const listener of Array.from(listeners)) {
        listener();
      }
    },
  };
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

describe("extractAnthropicChunks", () => {
  it("preserves text, thinking, and tool_use blocks distinctly", () => {
    const payload = {
      content: [
        { type: "thinking", thinking: "plan" },
        { type: "text", text: "answer" },
        { type: "tool_use", name: "read_file", input: { path: "spec.md" } },
      ],
    };

    expect(extractAnthropicChunks(payload)).toEqual([
      { text: "plan", kind: "reasoning" },
      { text: "answer", kind: "content" },
      { text: "\n\n```tool:read_file\n{\"path\":\"spec.md\"}\n```\n", kind: "tool_call" },
    ]);
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

  it("translates fenced tool calls and tool results into Anthropic native blocks", async () => {
    let seenBody: unknown;
    const fakeFetch = (async (_url: string, init: RequestInit) => {
      seenBody = JSON.parse(init.body as string);
      return new Response(JSON.stringify({ content: [{ type: "text", text: "ok" }] }), {
        status: 200,
      });
    }) as unknown as typeof fetch;

    const backend = new AnthropicBackend({
      model: "claude-sonnet-4-6",
      secrets: stubSecrets({ "sim-flow.anthropic.apiKey": "k" }),
      fetchImpl: fakeFetch,
    });

    for await (const _ of backend.stream(
      [
        { role: "user", content: "inspect the spec" },
        { role: "assistant", content: "```tool:read_file\n{\"path\":\"docs/spec.md\"}\n```\n" },
        { role: "user", content: "Tool results:\n\n[read_file `docs/spec.md`]\n\n# Spec\n" },
      ],
      noCancel(),
      [
        {
          name: "read_file",
          description: "Read a file",
          args_schema: {
            type: "object",
            properties: {
              path: { type: "string" },
            },
          },
        },
      ],
    )) {
      // drain
    }

    expect(seenBody).toEqual({
      model: "claude-sonnet-4-6",
      max_tokens: 4096,
      system: undefined,
      messages: [
        { role: "user", content: "inspect the spec" },
        {
          role: "assistant",
          content: [
            {
              type: "tool_use",
              id: "call_1_0",
              name: "read_file",
              input: { path: "docs/spec.md" },
            },
          ],
        },
        {
          role: "user",
          content: [
            {
              type: "tool_result",
              tool_use_id: "call_1_0",
              content: "[read_file `docs/spec.md`]\n\n# Spec",
            },
          ],
        },
      ],
      tools: [
        {
          name: "read_file",
          description: "Read a file",
          input_schema: {
            type: "object",
            properties: {
              path: { type: "string" },
            },
          },
        },
      ],
    });
  });

  it("returns thinking and tool_use blocks without flattening them to plain text", async () => {
    const fakeFetch = (async () =>
      new Response(
        JSON.stringify({
          content: [
            { type: "thinking", thinking: "plan" },
            { type: "tool_use", name: "read_file", input: { path: "spec.md" } },
            { type: "text", text: "done" },
          ],
        }),
        { status: 200 },
      )) as unknown as typeof fetch;

    const backend = new AnthropicBackend({
      model: "claude-sonnet-4-6",
      secrets: stubSecrets({ "sim-flow.anthropic.apiKey": "k" }),
      fetchImpl: fakeFetch,
    });

    const chunks = [];
    for await (const chunk of backend.stream([{ role: "user", content: "hi" }], noCancel())) {
      chunks.push(chunk);
    }

    expect(chunks).toEqual([
      { text: "plan", kind: "reasoning" },
      { text: "\n\n```tool:read_file\n{\"path\":\"spec.md\"}\n```\n", kind: "tool_call" },
      { text: "done", kind: "content" },
    ]);
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

  it("aborts an in-flight fetch when cancelled", async () => {
    const fakeFetch = (async (_url: string, init: RequestInit) =>
      await new Promise<Response>((_resolve, reject) => {
        const signal = init.signal as AbortSignal;
        signal.addEventListener("abort", () => {
          reject(Object.assign(new Error("aborted"), { name: "AbortError" }));
        }, { once: true });
      })) as unknown as typeof fetch;

    const backend = new AnthropicBackend({
      secrets: stubSecrets({ "sim-flow.anthropic.apiKey": "k" }),
      fetchImpl: fakeFetch,
    });
    const cancellation = cancellableToken();
    const pending = (async () => {
      for await (const _ of backend.stream([{ role: "user", content: "hi" }], cancellation.token)) {
        // drain
      }
    })();
    await Promise.resolve();
    cancellation.cancel();

    await expect(pending).rejects.toMatchObject({
      kind: "cancelled",
    });
  });
});
