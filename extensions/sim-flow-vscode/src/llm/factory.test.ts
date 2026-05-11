import { describe, expect, it, vi } from "vitest";

vi.mock("vscode", () => ({
  workspace: {},
  window: {},
}));

const { createBackend } = await import("./factory");
const { LMStudioBackend } = await import("./lmstudio");
const { OllamaBackend } = await import("./ollama");
const { OpenAiBackend } = await import("./openai");
const { LlmError } = await import("./types");
const { VSCodeLmBackend } = await import("./vscode");

describe("createBackend", () => {
  it("returns VSCodeLmBackend for source=vscode", () => {
    const backend = createBackend({ source: "vscode" });
    expect(backend).toBeInstanceOf(VSCodeLmBackend);
    expect(backend.name).toBe("vscode.lm");
  });

  it("returns OpenAiBackend for source=openai", () => {
    const backend = createBackend({
      source: "openai",
      secrets: { get: async () => undefined },
    });
    expect(backend).toBeInstanceOf(OpenAiBackend);
    expect(backend.name).toBe("openai");
    expect(backend.adaptation?.runtime.id).toBe("openai_compat_generic");
  });

  it("infers the model family from the configured model id", () => {
    const backend = createBackend({
      source: "lmstudio",
      model: "Qwen/Qwen3.6-35B-A3B",
    });
    expect(backend.adaptation?.modelFamily.id).toBe("qwen3_6");
  });

  it("honors an explicit model-family override", () => {
    const backend = createBackend({
      source: "lmstudio",
      model: "moonshotai/Kimi-VL-A3B-Thinking-2506",
      modelFamilyId: "gemma4",
    });
    expect(backend.adaptation?.modelFamily.id).toBe("gemma4");
  });

  it("returns OllamaBackend for source=ollama (no secrets required)", () => {
    const backend = createBackend({ source: "ollama" });
    expect(backend).toBeInstanceOf(OllamaBackend);
    expect(backend.name).toBe("ollama");
    expect(backend.adaptation?.runtime.id).toBe("openai_compat_generic");
  });

  it("returns LMStudioBackend for source=lmstudio (no secrets required)", () => {
    const backend = createBackend({ source: "lmstudio" });
    expect(backend).toBeInstanceOf(LMStudioBackend);
    expect(backend.name).toBe("lmstudio");
    expect(backend.adaptation?.runtime.id).toBe("openai_compat_generic");
  });

  it("throws unsupported for an unknown source", () => {
    try {
      createBackend({ source: "bogus" as unknown as "vscode" });
      throw new Error("expected throw");
    } catch (err) {
      expect(err).toBeInstanceOf(LlmError);
      expect((err as InstanceType<typeof LlmError>).kind).toBe("unsupported");
    }
  });

  it.each(["anthropic", "claude-cli", "codex-cli", "gh-copilot-cli"] as const)(
    "throws unsupported with an orchestrator-route hint for source=%s",
    (source) => {
      // Sources that don't have a chat-pane HTTP backend in the
      // extension. Anthropic dispatch lives in the sim-flow Rust
      // orchestrator (per the architecture doc: HTTP backends live
      // in `sim-flow` Rust). CLI agents run via `sim-flow auto
      // --llm-backend <name>` in a terminal. Either way the
      // chat-pane factory rejects them with a clear message so a
      // user who left the picker on `anthropic` and then triggered
      // a /step session sees why nothing's happening (instead of a
      // silent no-op or a misleading "no model" error).
      try {
        createBackend({ source });
        throw new Error("expected throw");
      } catch (err) {
        expect(err).toBeInstanceOf(LlmError);
        const llmErr = err as InstanceType<typeof LlmError>;
        expect(llmErr.kind).toBe("unsupported");
        expect(llmErr.message).toContain(source);
        expect(llmErr.message).toContain("orchestrator");
      }
    },
  );
});
