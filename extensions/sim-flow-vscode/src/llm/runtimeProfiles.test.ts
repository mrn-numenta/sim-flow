import { describe, expect, it } from "vitest";

import {
  ANTHROPIC_MESSAGES_RUNTIME,
  KNOWN_RUNTIME_PROFILE_IDS,
  mergeLeadingSystemMessages,
  OPENAI_COMPAT_GENERIC_RUNTIME,
  prepareAnthropicMessages,
  PROCESSOR_LOCAL_RUNTIME,
  resolveRuntimeProfile,
  VSCODE_LM_RUNTIME,
} from "./runtimeProfiles";
import type { LlmMessage } from "./types";

function system(content: string): LlmMessage {
  return { role: "system", content };
}

function user(content: string): LlmMessage {
  return { role: "user", content };
}

describe("OPENAI_COMPAT_GENERIC_RUNTIME", () => {
  it("collapses leading system messages through prepareInput", () => {
    const prepared = OPENAI_COMPAT_GENERIC_RUNTIME.prepareInput?.([
      system("a"),
      system("b"),
      user("hello"),
    ]);

    expect(prepared).toEqual({
      messages: [
        { role: "system", content: "a\n\nb" },
        { role: "user", content: "hello" },
      ],
    });
  });
});

describe("mergeLeadingSystemMessages", () => {
  it("preserves merged system attachments", () => {
    const a = { mime: "image/png", data: "AAA", source: "foo.png" };
    const b = { mime: "image/png", data: "BBB", source: "bar.png" };
    expect(
      mergeLeadingSystemMessages([
        { role: "system", content: "first", attachments: [a] },
        { role: "system", content: "second", attachments: [b] },
        user("hi"),
      ]),
    ).toEqual([
      { role: "system", content: "first\n\nsecond", attachments: [a, b] },
      { role: "user", content: "hi" },
    ]);
  });
});

describe("ANTHROPIC_MESSAGES_RUNTIME", () => {
  it("moves system messages into the dedicated system field", () => {
    const prepared = prepareAnthropicMessages([system("sys-a"), system("sys-b"), user("hi")]);

    expect(prepared).toEqual({
      system: "sys-a\n\nsys-b",
      messages: [{ role: "user", content: "hi" }],
    });
  });

  it("advertises runtime capabilities explicitly", () => {
    expect(ANTHROPIC_MESSAGES_RUNTIME.requestFormat).toBe("anthropic_messages");
    expect(ANTHROPIC_MESSAGES_RUNTIME.systemPromptMode).toBe("dedicated-field");
    expect(ANTHROPIC_MESSAGES_RUNTIME.supportsSharedCredentialChain).toBe(true);
  });
});

describe("PROCESSOR_LOCAL_RUNTIME", () => {
  it("keeps the placeholder runtime available for processor-centric backends", () => {
    expect(PROCESSOR_LOCAL_RUNTIME.id).toBe("processor_local");
    expect(PROCESSOR_LOCAL_RUNTIME.requestFormat).toBe("processor_local");
  });
});

describe("VSCODE_LM_RUNTIME", () => {
  it("names the host-managed VS Code LM runtime explicitly", () => {
    expect(VSCODE_LM_RUNTIME.id).toBe("vscode_language_model");
    expect(VSCODE_LM_RUNTIME.requestFormat).toBe("vscode_language_model");
  });
});

describe("resolveRuntimeProfile", () => {
  it("returns the fallback when no explicit id is supplied", () => {
    expect(resolveRuntimeProfile(undefined, OPENAI_COMPAT_GENERIC_RUNTIME)).toBe(
      OPENAI_COMPAT_GENERIC_RUNTIME,
    );
  });

  it("honors a compatible explicit override", () => {
    expect(
      resolveRuntimeProfile("anthropic_messages", OPENAI_COMPAT_GENERIC_RUNTIME, [
        "anthropic_messages",
      ]),
    ).toBe(ANTHROPIC_MESSAGES_RUNTIME);
  });

  it("throws on unknown ids", () => {
    expect(() => resolveRuntimeProfile("bogus", OPENAI_COMPAT_GENERIC_RUNTIME)).toThrow(
      "Unknown runtime capability profile",
    );
    expect(KNOWN_RUNTIME_PROFILE_IDS).toContain("openai_compat_generic");
  });
});
