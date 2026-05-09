import { describe, expect, it } from "vitest";

import { LMSTUDIO_DEFAULT_BASE_URL, LMStudioBackend } from "./lmstudio";
import {
  contentDelta,
  finishReason,
  sseResponse,
  sseResponseWithoutDone,
  sseSingleResponse,
} from "./sse-test-helpers";
import { LlmError } from "./types";

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

describe("LMStudioBackend", () => {
  it("posts to the default local endpoint without an Authorization header", async () => {
    let seenUrl: string | undefined;
    let seenHeaders: Record<string, string> | undefined;
    const fakeFetch = (async (url: string, init: RequestInit) => {
      seenUrl = url;
      seenHeaders = init.headers as Record<string, string>;
      return sseSingleResponse("pong");
    }) as unknown as typeof fetch;

    const backend = new LMStudioBackend({ fetchImpl: fakeFetch });
    const chunks: string[] = [];
    for await (const c of backend.stream([{ role: "user", content: "ping" }], noCancel())) {
      chunks.push(c.text);
    }
    expect(chunks.join("")).toBe("pong");
    expect(seenUrl).toBe(`${LMSTUDIO_DEFAULT_BASE_URL}/chat/completions`);
    expect(seenHeaders!.authorization).toBeUndefined();
  });

  it("passes the caller-supplied model through", async () => {
    let seenBody: unknown;
    const fakeFetch = (async (_url: string, init: RequestInit) => {
      seenBody = JSON.parse(init.body as string);
      return sseSingleResponse("");
    }) as unknown as typeof fetch;

    const backend = new LMStudioBackend({
      fetchImpl: fakeFetch,
      model: "qwen2.5-coder-7b-instruct",
    });
    for await (const _ of backend.stream([{ role: "user", content: "x" }], noCancel())) {
      // drain
    }
    expect((seenBody as { model: string }).model).toBe("qwen2.5-coder-7b-instruct");
    expect((seenBody as { stream: boolean }).stream).toBe(true);
  });

  it("wraps non-2xx responses in an http LlmError", async () => {
    const fakeFetch = (async () =>
      new Response("server down", { status: 500, statusText: "err" })) as unknown as typeof fetch;

    const backend = new LMStudioBackend({ fetchImpl: fakeFetch });
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

  it("forwards the tool catalog as OpenAI function tools when supplied", async () => {
    let seenBody: { tools?: unknown } | undefined;
    const fakeFetch = (async (_url: string, init: RequestInit) => {
      seenBody = JSON.parse(init.body as string);
      return sseSingleResponse("");
    }) as unknown as typeof fetch;

    const backend = new LMStudioBackend({ fetchImpl: fakeFetch });
    for await (const _ of backend.stream(
      [{ role: "user", content: "x" }],
      noCancel(),
      [
        {
          name: "read_file",
          description: "Read a file.",
          args_schema: { type: "object", properties: { path: { type: "string" } } },
        },
      ],
    )) {
      // drain
    }
    const tools = seenBody?.tools as Array<{
      type: string;
      function: { name: string; parameters: unknown };
    }>;
    expect(tools).toHaveLength(1);
    expect(tools[0].type).toBe("function");
    expect(tools[0].function.name).toBe("read_file");
    expect(tools[0].function.parameters).toEqual({
      type: "object",
      properties: { path: { type: "string" } },
    });
  });

  it("synthesizes fenced tool: blocks when streamed deltas carry tool_calls", async () => {
    // Mirrors how OpenAI / LM Studio actually deliver tool calls in
    // `stream: true` mode: the function name comes in one fragment,
    // arguments arrive in subsequent fragments, both keyed by
    // `index`. The backend must concatenate args by index and emit a
    // single fenced block per call when the stream ends.
    const fakeFetch = (async () =>
      sseResponse([
        contentDelta("I'll read it."),
        {
          choices: [
            {
              index: 0,
              delta: {
                tool_calls: [
                  {
                    index: 0,
                    id: "call_1",
                    type: "function",
                    function: { name: "read_file", arguments: "" },
                  },
                ],
              },
            },
          ],
        },
        {
          choices: [
            {
              index: 0,
              delta: {
                tool_calls: [{ index: 0, function: { arguments: '{"path":' } }],
              },
            },
          ],
        },
        {
          choices: [
            {
              index: 0,
              delta: {
                tool_calls: [{ index: 0, function: { arguments: '"src/lib.rs"}' } }],
              },
            },
          ],
        },
        finishReason("tool_calls"),
      ])) as unknown as typeof fetch;

    const backend = new LMStudioBackend({ fetchImpl: fakeFetch });
    const chunks: string[] = [];
    for await (const c of backend.stream([{ role: "user", content: "x" }], noCancel())) {
      chunks.push(c.text);
    }
    const joined = chunks.join("");
    expect(joined).toContain("I'll read it.");
    expect(joined).toMatch(/```tool:read_file\n\{"path":"src\/lib\.rs"\}\n```/);
  });

  it("yields reasoning_content deltas as kind:reasoning so the host can render them collapsibly", async () => {
    // Qwen3-Coder, DeepSeek-R1, and o-series emit chain-of-thought
    // via `delta.reasoning_content` (Qwen / DS) or `delta.reasoning`
    // (OpenAI). We surface both as `kind: "reasoning"` so the chat
    // pane can wrap them in a collapsed `<details>` block while the
    // orchestrator-facing path stays content-only.
    const fakeFetch = (async () =>
      sseResponse([
        { choices: [{ index: 0, delta: { reasoning_content: "Let me think..." } }] },
        { choices: [{ index: 0, delta: { reasoning_content: " step one." } }] },
        contentDelta("Final answer."),
        finishReason("stop"),
      ])) as unknown as typeof fetch;

    const backend = new LMStudioBackend({ fetchImpl: fakeFetch });
    const chunks: Array<{ text: string; kind?: string }> = [];
    for await (const c of backend.stream([{ role: "user", content: "x" }], noCancel())) {
      chunks.push({ text: c.text, kind: c.kind });
    }
    expect(chunks).toEqual([
      { text: "Let me think...", kind: "reasoning" },
      { text: " step one.", kind: "reasoning" },
      { text: "Final answer.", kind: "content" },
    ]);
  });

  it("yields content deltas as they arrive (multi-chunk stream)", async () => {
    // The whole point of streaming: tokens come back in pieces and
    // each piece is yielded as a separate chunk so the chat pane can
    // render in real time and the body-timeout never fires.
    const fakeFetch = (async () =>
      sseResponse([
        contentDelta("Hel"),
        contentDelta("lo, "),
        contentDelta("world"),
        contentDelta("!"),
        finishReason("stop"),
      ])) as unknown as typeof fetch;

    const backend = new LMStudioBackend({ fetchImpl: fakeFetch });
    const chunks: string[] = [];
    for await (const c of backend.stream([{ role: "user", content: "hi" }], noCancel())) {
      chunks.push(c.text);
    }
    expect(chunks).toEqual(["Hel", "lo, ", "world", "!"]);
    expect(chunks.join("")).toBe("Hello, world!");
  });

  it("stops on finish_reason even when the server never sends [DONE]", async () => {
    const fakeFetch = (async () =>
      sseResponseWithoutDone([
        contentDelta("done"),
        finishReason("stop"),
      ])) as unknown as typeof fetch;

    const backend = new LMStudioBackend({ fetchImpl: fakeFetch });
    const chunks: string[] = [];
    for await (const c of backend.stream([{ role: "user", content: "hi" }], noCancel())) {
      chunks.push(c.text);
    }
    expect(chunks).toEqual(["done"]);
  });

  it("fails fast when the stream never produces any response bytes", async () => {
    const fakeFetch = (async (_url: string, init: RequestInit) => {
      const signal = init.signal as AbortSignal;
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          signal.addEventListener(
            "abort",
            () => {
              controller.error(Object.assign(new Error("aborted"), { name: "AbortError" }));
            },
            { once: true },
          );
        },
      });
      return new Response(body, {
        status: 200,
        headers: { "content-type": "text/event-stream" },
      });
    }) as unknown as typeof fetch;

    const backend = new LMStudioBackend({
      fetchImpl: fakeFetch,
      streamIdleTimeoutMs: 20,
    });
    try {
      for await (const _ of backend.stream([{ role: "user", content: "hi" }], noCancel())) {
        // drain
      }
      throw new Error("expected timeout");
    } catch (err) {
      expect(err).toBeInstanceOf(LlmError);
      expect((err as LlmError).kind).toBe("http");
      expect((err as LlmError).message).toContain("timed out");
    }
  });

  it("includes LlmError.detail body text when available", async () => {
    const fakeFetch = (async () =>
      new Response(
        '{"error":"The number of tokens to keep from the initial prompt is greater than the context length"}',
        { status: 400, statusText: "Bad Request" },
      )) as unknown as typeof fetch;

    const backend = new LMStudioBackend({ fetchImpl: fakeFetch });
    try {
      for await (const _ of backend.stream([{ role: "user", content: "x" }], noCancel())) {
        // drain
      }
      throw new Error("expected throw");
    } catch (err) {
      expect(err).toBeInstanceOf(LlmError);
      expect((err as LlmError).detail).toContain("context length");
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

    const backend = new LMStudioBackend({ fetchImpl: fakeFetch });
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
