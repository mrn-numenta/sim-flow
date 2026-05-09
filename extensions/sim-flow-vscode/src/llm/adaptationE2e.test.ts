import { describe, expect, it } from "vitest";

import { AnthropicBackend } from "./anthropic";
import { LMStudioBackend } from "./lmstudio";
import { sseSingleResponse } from "./sse-test-helpers";
import type { LlmChunkKind, SecretStorage } from "./types";

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

function stubSecrets(map: Record<string, string>): SecretStorage {
  return {
    get: async (k: string) => map[k],
  };
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

  it("preserves Claude thinking and tool-use blocks through the Anthropic path", async () => {
    const backend = new AnthropicBackend({
      model: "claude-sonnet-4-6",
      secrets: stubSecrets({ "sim-flow.anthropic.apiKey": "k" }),
      fetchImpl: (async () =>
        new Response(
          JSON.stringify({
            content: [
              { type: "thinking", thinking: "plan" },
              { type: "tool_use", name: "read_file", input: { path: "spec.md" } },
              { type: "text", text: "done" },
            ],
          }),
          { status: 200 },
        )) as typeof fetch,
    });

    await expect(
      collectNormalizedKinds(backend, [{ role: "user", content: "hi" }]),
    ).resolves.toEqual([
      { text: "plan", kind: "reasoning" },
      { text: '\n\n```tool:read_file\n{"path":"spec.md"}\n```\n', kind: "tool_call" },
      { text: "done", kind: "content" },
    ]);
  });
});
