import { describe, expect, it } from "vitest";

import { resolveLlmSource, type LlmServerEntry } from "./messages";

describe("resolveLlmSource", () => {
  it("passes through built-in sources unchanged", () => {
    expect(resolveLlmSource("vscode", [])).toEqual({ source: "vscode" });
  });

  it("resolves custom server overrides including model adaptation metadata", () => {
    const servers: LlmServerEntry[] = [
      {
        name: "kimi",
        kind: "openai-compat",
        host: "kimi.internal",
        port: 443,
        path: "/v1",
        model: "moonshot/kimi-k2",
        modelFamilyId: "kimi_vl_thinking",
        runtimeProfileId: "openai_compat_generic",
      },
    ];

    expect(resolveLlmSource("server:kimi", servers)).toEqual({
      source: "openai-compat",
      baseUrl: "http://kimi.internal:443/v1",
      model: "moonshot/kimi-k2",
      modelFamilyId: "kimi_vl_thinking",
      runtimeProfileId: "openai_compat_generic",
    });
  });

  it("returns null for unknown custom servers", () => {
    expect(resolveLlmSource("server:missing", [])).toBeNull();
  });
});
