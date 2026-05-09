import { describe, expect, it } from "vitest";

import {
  normalizeLlmChunk,
  type LlmAdaptationProfile,
  type LlmStreamChunk,
} from "./types";

describe("normalizeLlmChunk", () => {
  it("defaults an omitted kind to content", () => {
    const chunk: LlmStreamChunk = { text: "hello" };
    expect(normalizeLlmChunk(chunk)).toEqual({
      text: "hello",
      kind: "content",
    });
  });

  it("preserves an explicit reasoning kind", () => {
    const chunk: LlmStreamChunk = { text: "thinking", kind: "reasoning" };
    expect(normalizeLlmChunk(chunk)).toEqual({
      text: "thinking",
      kind: "reasoning",
    });
  });

  it("preserves an explicit tool_call kind", () => {
    const chunk: LlmStreamChunk = { text: "tool:get_status", kind: "tool_call" };
    expect(normalizeLlmChunk(chunk)).toEqual({
      text: "tool:get_status",
      kind: "tool_call",
    });
  });
});

describe("LlmAdaptationProfile shape", () => {
  it("supports minimal runtime/model/normalizer metadata", () => {
    const profile: LlmAdaptationProfile = {
      runtime: {
        id: "openai_compat_generic",
        collapseLeadingSystemMessages: true,
        supportsStructuredReasoning: true,
      },
      modelFamily: {
        id: "qwen3_6",
        thoughtMarkerStyle: "qwen-think-tag",
        supportsThinkingControls: true,
      },
      responseNormalizer: {
        id: "default",
        normalizeChunk: (chunk) => [normalizeLlmChunk(chunk)],
      },
    };

    expect(profile.runtime.id).toBe("openai_compat_generic");
    expect(profile.modelFamily.id).toBe("qwen3_6");
    expect(profile.responseNormalizer.normalizeChunk({ text: "x" })).toEqual([
      {
        text: "x",
        kind: "content",
      },
    ]);
  });
});
