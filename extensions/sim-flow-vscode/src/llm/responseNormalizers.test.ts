import { describe, expect, it } from "vitest";

import {
  createResponseNormalizerForFamily,
  DEFAULT_RESPONSE_NORMALIZER,
} from "./responseNormalizers";
import {
  GENERIC_CHAT_MODEL_FAMILY,
  KIMI_VL_THINKING_MODEL_FAMILY,
  QWEN3_6_MODEL_FAMILY,
} from "./modelFamilies";

describe("DEFAULT_RESPONSE_NORMALIZER", () => {
  it("passes through plain chunks unchanged", () => {
    expect(DEFAULT_RESPONSE_NORMALIZER.normalizeChunk({ text: "hello" })).toEqual([
      { text: "hello", kind: "content" },
    ]);
  });
});

describe("createResponseNormalizerForFamily", () => {
  it("splits Qwen think tags into reasoning and content chunks", () => {
    const normalizer = createResponseNormalizerForFamily(QWEN3_6_MODEL_FAMILY);
    expect(normalizer.normalizeChunk({ text: "<think>plan</think>answer" })).toEqual([
      { kind: "reasoning", text: "plan" },
      { kind: "content", text: "answer" },
    ]);
  });

  it("handles split Qwen think tags across multiple chunks", () => {
    const normalizer = createResponseNormalizerForFamily(QWEN3_6_MODEL_FAMILY);
    expect(normalizer.normalizeChunk({ text: "<thi" })).toEqual([]);
    expect(normalizer.normalizeChunk({ text: "nk>plan</th" })).toEqual([
      { kind: "reasoning", text: "plan" },
    ]);
    expect(normalizer.normalizeChunk({ text: "ink>answer" })).toEqual([
      { kind: "content", text: "answer" },
    ]);
    expect(normalizer.flush?.()).toEqual([]);
  });

  it("splits Kimi think tags into reasoning and content chunks", () => {
    const normalizer = createResponseNormalizerForFamily(KIMI_VL_THINKING_MODEL_FAMILY);
    expect(normalizer.normalizeChunk({ text: "◁think▷plan◁/think▷answer" })).toEqual([
      { kind: "reasoning", text: "plan" },
      { kind: "content", text: "answer" },
    ]);
  });

  it("falls back to default normalization for generic families", () => {
    const normalizer = createResponseNormalizerForFamily(GENERIC_CHAT_MODEL_FAMILY);
    expect(normalizer.normalizeChunk({ text: "hello" })).toEqual([
      { kind: "content", text: "hello" },
    ]);
  });
});
