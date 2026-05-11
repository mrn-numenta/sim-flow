import { describe, expect, it } from "vitest";

import { LMStudioBackend } from "./lmstudio";
import { sseSingleResponse } from "./sse-test-helpers";
import type { LlmChunkKind } from "./types";

function noCancel() {
  return { isCancellationRequested: false };
}

function collectNormalizedKinds(
  backend: {
    adaptation?: {
      responseNormalizer: {
        normalizeChunk(chunk: {
          text: string;
          kind?: LlmChunkKind;
        }): Array<{ text: string; kind: LlmChunkKind }>;
        flush?(): Array<{ text: string; kind: LlmChunkKind }>;
      };
    };
    stream(
      messages: Array<{ role: "system" | "user" | "assistant"; content: string }>,
      token: { isCancellationRequested: boolean },
    ): AsyncIterable<{ text: string; kind?: LlmChunkKind }>;
  },
  messages: Array<{ role: "system" | "user" | "assistant"; content: string }>,
): Promise<Array<{ text: string; kind: LlmChunkKind }>> {
  const normalizer = backend.adaptation!.responseNormalizer;
  const out: Array<{ text: string; kind: LlmChunkKind }> = [];
  return (async () => {
    for await (const raw of backend.stream(messages, noCancel())) {
      out.push(...normalizer.normalizeChunk(raw));
    }
    out.push(...(normalizer.flush?.() ?? []));
    return out;
  })();
}

describe("adaptation end-to-end validation", () => {
  it("normalizes Qwen raw think tags through the OpenAI-compatible path", async () => {
    const backend = new LMStudioBackend({
      model: "Qwen/Qwen3.6-35B-A3B",
      fetchImpl: (async () => sseSingleResponse("<think>plan</think>answer")) as typeof fetch,
    });

    await expect(
      collectNormalizedKinds(backend, [{ role: "user", content: "hi" }]),
    ).resolves.toEqual([
      { text: "plan", kind: "reasoning" },
      { text: "answer", kind: "content" },
    ]);
  });

  it("normalizes Kimi think tags through the OpenAI-compatible path", async () => {
    const backend = new LMStudioBackend({
      model: "moonshotai/Kimi-VL-A3B-Thinking-2506",
      fetchImpl: (async () => sseSingleResponse("◁think▷plan◁/think▷answer")) as typeof fetch,
    });

    await expect(
      collectNormalizedKinds(backend, [{ role: "user", content: "hi" }]),
    ).resolves.toEqual([
      { text: "plan", kind: "reasoning" },
      { text: "answer", kind: "content" },
    ]);
  });

  // The Anthropic-path adaptation case moved to the sim-flow Rust
  // orchestrator (AnthropicAgent) along with the rest of the HTTP
  // Anthropic backend. Equivalent coverage lives in the Rust agent
  // tests; the extension no longer hosts this code path.
});
