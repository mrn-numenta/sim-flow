import { describe, expect, it } from "vitest";

import {
  appendAssistantChunk,
  appendNote,
  appendUserPrompt,
  clearConversationState,
  completeAssistantTurn,
  createConversationState,
  setEntryRequestTokensEstimate,
  stripProtocolFences,
  stripToolCallFences,
  stripToolCallFencesForDisplay,
  stripToolCallFencesForStreaming,
  summarizeTokenEstimates,
  toLlmMessages,
  toStoredConversation,
} from "./state";

describe("chatPanel/state", () => {
  it("appends a user turn and streams assistant chunks into the placeholder entry", () => {
    const initial = clearConversationState();
    const started = appendUserPrompt(
      initial,
      "Explain the current build failure.",
      "demo-project",
      "LM Studio • qwen",
    );
    let state = started.state;
    state = appendAssistantChunk(state, started.assistantId, "The build is failing");
    state = appendAssistantChunk(state, started.assistantId, " because Cargo cannot find a feature.");
    state = completeAssistantTurn(state, started.assistantId);

    expect(state.transcript).toHaveLength(2);
    expect(state.transcript[0]).toMatchObject({
      kind: "user",
      body: "Explain the current build failure.",
    });
    expect(state.transcript[1]).toMatchObject({
      kind: "assistant",
      body: "The build is failing because Cargo cannot find a feature.",
      streaming: false,
    });
  });

  it("omits notes and empty streaming placeholders when converting to backend messages", () => {
    let state = clearConversationState();
    state = appendNote(state, "Info", "Panel ready.");
    const started = appendUserPrompt(state, "Summarize the current step.", undefined, "LM Studio");

    const messages = toLlmMessages(started.state.transcript, "System prompt.");
    expect(messages).toEqual([
      { role: "system", content: "System prompt." },
      { role: "user", content: "Summarize the current step." },
    ]);
  });

  it("strips transient streaming state before persistence and restores next ids", () => {
    const started = appendUserPrompt(
      clearConversationState(),
      "Ping",
      undefined,
      "LM Studio",
    );
    const stored = toStoredConversation(started.state);
    const restored = createConversationState(stored);

    expect(stored.transcript?.[1]).toMatchObject({
      kind: "assistant",
      streaming: false,
    });
    expect(restored.nextId).toBe(3);
  });

  it("drops legacy inline header-status notes when restoring stored conversations", () => {
    const restored = createConversationState({
      nextId: 4,
      transcript: [
        {
          id: "entry-1",
          kind: "note",
          title: "Tool activity",
          body: "_Tool `read_file` -> ok (12 ms)._",
          tone: "info",
        },
        {
          id: "entry-2",
          kind: "note",
          title: "Artifact written",
          body: "_Wrote `docs/report.md` (512 bytes)._",
          tone: "info",
        },
        {
          id: "entry-3",
          kind: "user",
          title: "You",
          body: "Continue.",
        },
      ],
    });

    expect(restored.transcript).toHaveLength(1);
    expect(restored.transcript[0]).toMatchObject({
      kind: "user",
      body: "Continue.",
    });
  });

  it("tracks estimated request and response tokens across the transcript", () => {
    const started = appendUserPrompt(
      clearConversationState(),
      "Explain the current build failure in one sentence.",
      "demo-project",
      "LM Studio • qwen",
      42,
    );
    let state = appendAssistantChunk(
      started.state,
      started.assistantId,
      "Cargo is failing because the selected feature is not defined in the workspace manifest.",
    );
    state = completeAssistantTurn(state, started.assistantId);

    expect(state.transcript[0]).toMatchObject({
      kind: "user",
      requestTokensEstimate: 42,
    });
    expect(state.transcript[1]).toMatchObject({
      kind: "assistant",
      responseTokensEstimate: expect.any(Number),
    });
    expect(summarizeTokenEstimates(state.transcript)).toEqual({
      input: 42,
      output: expect.any(Number),
    });
  });

  it("can attach a request estimate to an existing entry when the pump reports it later", () => {
    const started = appendUserPrompt(
      clearConversationState(),
      "Continue.",
      undefined,
      "sim-flow",
    );
    const state = setEntryRequestTokensEstimate(started.state, started.userId, 88);

    expect(state.transcript[0]).toMatchObject({
      kind: "user",
      requestTokensEstimate: 88,
    });
  });

  it("removes tool fences from assistant messages before reuse or display", () => {
    const text = [
      "Here is the result.",
      "",
      "```tool:read_file",
      "{\"path\":\"src/lib.rs\"}",
      "```",
      "",
      "And here is the visible summary.",
    ].join("\n");

    expect(stripToolCallFences(text)).toBe(
      ["Here is the result.", "", "And here is the visible summary."].join("\n"),
    );
  });

  it("hides unterminated tool fences while a response is still streaming", () => {
    const partial = ["Visible intro.", "", "```tool:read_file", "{\"path\":\"src/lib.rs\"}"].join("\n");
    expect(stripToolCallFencesForDisplay(partial)).toBe("Visible intro.");
  });

  it("hides orchestrator artifact-write blocks from display and transcript reuse", () => {
    const text = [
      "Short visible preface.",
      "",
      "```docs/spec.md",
      "# Spec",
      "generic generated content",
      "```",
      "",
      "Visible tail.",
    ].join("\n");

    expect(stripProtocolFences(text)).toBe(
      ["Short visible preface.", "", "Visible tail."].join("\n"),
    );
  });

  it("drops artifact-only orchestrator assistant entries when restoring persisted conversations", () => {
    const restored = createConversationState({
      nextId: 3,
      transcript: [
        {
          id: "entry-1",
          kind: "assistant",
          title: "sim-flow",
          body: ["```docs/spec.md", "# Spec", "```"].join("\n"),
          meta: "orchestrator",
          streaming: false,
        },
        {
          id: "entry-2",
          kind: "user",
          title: "You",
          body: "Continue.",
        },
      ],
    });

    expect(restored.transcript).toHaveLength(1);
    expect(restored.transcript[0]).toMatchObject({
      kind: "user",
      body: "Continue.",
    });
  });

  it("strips orchestrator artifact blocks before reusing assistant turns as LLM context", () => {
    const messages = toLlmMessages(
      [
        {
          id: "entry-1",
          kind: "assistant",
          title: "sim-flow",
          body: [
            "Visible summary.",
            "",
            "```docs/spec.md",
            "# Spec",
            "```",
          ].join("\n"),
          meta: "orchestrator",
        },
      ],
      undefined,
    );

    expect(messages).toEqual([{ role: "assistant", content: "Visible summary." }]);
  });

  it("preserves leading whitespace for streamed assistant chunks", () => {
    expect(stripToolCallFencesForStreaming(" begin by reading")).toBe(" begin by reading");
    expect(
      stripToolCallFencesForStreaming(" visible\n```tool:read_file\n{\"path\":\"x\"}"),
    ).toBe(" visible");
  });
});
