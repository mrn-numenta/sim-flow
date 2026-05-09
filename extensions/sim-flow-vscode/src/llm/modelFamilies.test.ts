import { describe, expect, it } from "vitest";

import {
  applyModelFamilyPromptPolicy,
  GEMMA4_MODEL_FAMILY,
  inferModelFamily,
  KIMI_VL_THINKING_MODEL_FAMILY,
  orderAttachmentsByFamily,
  resolveModelFamily,
} from "./modelFamilies";

describe("inferModelFamily", () => {
  it("infers Gemma 4 from the model id", () => {
    expect(inferModelFamily("google/gemma-4-27b-it").id).toBe("gemma4");
  });

  it("infers Qwen 3.6 from the model id", () => {
    expect(inferModelFamily("Qwen/Qwen3.6-35B-A3B").id).toBe("qwen3_6");
  });

  it("infers Kimi-VL from the model id", () => {
    expect(inferModelFamily("moonshotai/Kimi-VL-A3B-Thinking-2506").id).toBe(
      "kimi_vl_thinking",
    );
  });

  it("falls back to generic_chat when the model id is unknown", () => {
    expect(inferModelFamily("gpt-4o-mini").id).toBe("generic_chat");
  });
});

describe("resolveModelFamily", () => {
  it("honors an explicit override over inference", () => {
    expect(resolveModelFamily("gemma4", "moonshotai/Kimi-VL-A3B-Thinking-2506").id).toBe(
      "gemma4",
    );
  });
});

describe("applyModelFamilyPromptPolicy", () => {
  it("injects the Gemma thinking token into a dedicated system field when enabled", () => {
    const prepared = applyModelFamilyPromptPolicy(
      {
        system: "You are helpful.",
        messages: [{ role: "user", content: "hi" }],
      },
      GEMMA4_MODEL_FAMILY,
      { enableThinking: true },
    );

    expect(prepared.system).toBe("<|think|>\nYou are helpful.");
  });

  it("does not mutate prompts when thinking is not enabled", () => {
    const prepared = applyModelFamilyPromptPolicy(
      {
        messages: [{ role: "user", content: "hi" }],
      },
      GEMMA4_MODEL_FAMILY,
    );

    expect(prepared).toEqual({
      messages: [{ role: "user", content: "hi" }],
    });
  });
});

describe("orderAttachmentsByFamily", () => {
  const image = { mime: "image/png", data: "AAA", source: "img.png" };

  it("keeps text before media for the generic family", () => {
    expect(orderAttachmentsByFamily(resolveModelFamily(undefined, "gpt-4o-mini"), "caption", [image]))
      .toEqual([
        { kind: "text", text: "caption" },
        { kind: "attachment", attachment: image },
      ]);
  });

  it("moves media ahead of text for Kimi-VL", () => {
    expect(orderAttachmentsByFamily(KIMI_VL_THINKING_MODEL_FAMILY, "caption", [image])).toEqual([
      { kind: "attachment", attachment: image },
      { kind: "text", text: "caption" },
    ]);
  });
});
