import { describe, expect, it } from "vitest";

import {
  appendAssistantChunk,
  appendAssistantPlaceholder,
  appendAssistantTurnEntry,
  appendNote,
  appendOrchestratorUserEntry,
  appendUserPrompt,
  clearConversationState,
  completeAssistantTurn,
  createConversationState,
  filterPresentationEntries,
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

  it("appendOrchestratorUserEntry tallies body tokens at append time", () => {
    const out = appendOrchestratorUserEntry(
      clearConversationState(),
      "abcdefgh", // 8 chars -> ceil(8/4) = 2 tokens
      "Tool result",
      "orchestrator-tool",
    );
    expect(out.state.transcript).toHaveLength(1);
    expect(out.state.transcript[0]).toMatchObject({
      kind: "user",
      title: "Tool result",
      body: "abcdefgh",
      meta: "orchestrator-tool",
      requestTokensEstimate: 2,
    });
  });

  it("appendAssistantTurnEntry yields a non-streaming entry with a body token estimate", () => {
    const out = appendAssistantTurnEntry(
      clearConversationState(),
      "complete turn body that runs longer than four chars",
      "orchestrator-assistant",
    );
    const entry = out.state.transcript[0];
    expect(entry).toMatchObject({
      kind: "assistant",
      meta: "orchestrator-assistant",
      streaming: false,
    });
    if (entry.kind === "assistant") {
      // Use Math.ceil(len/4) -- the helper uses estimateTextTokens.
      expect(entry.responseTokensEstimate).toBeGreaterThan(0);
    }
  });

  it("appendAssistantPlaceholder creates a streaming entry that chunks can attach to", () => {
    const out = appendAssistantPlaceholder(
      clearConversationState(),
      "Assistant",
      "openai",
      99,
    );
    expect(out.state.transcript[0]).toMatchObject({
      kind: "assistant",
      title: "Assistant",
      meta: "openai",
      requestTokensEstimate: 99,
      streaming: true,
      body: "",
    });
    // The id returned must point at that placeholder.
    expect(out.assistantId).toBe(out.state.transcript[0].id);
  });

  it("appendAssistantChunk is a no-op when text is empty", () => {
    const placeholder = appendAssistantPlaceholder(
      clearConversationState(),
      "Assistant",
      undefined,
    );
    const after = appendAssistantChunk(placeholder.state, placeholder.assistantId, "");
    // No identity guarantees -- but contents shouldn't change.
    expect(after.transcript).toEqual(placeholder.state.transcript);
  });

  it("completeAssistantTurn substitutes the fallback when the body is empty whitespace", () => {
    const placeholder = appendAssistantPlaceholder(
      clearConversationState(),
      "Assistant",
      undefined,
    );
    const blank = appendAssistantChunk(placeholder.state, placeholder.assistantId, "   ");
    const done = completeAssistantTurn(blank, placeholder.assistantId, "<<no body>>");
    expect(done.transcript[0]).toMatchObject({
      kind: "assistant",
      body: "<<no body>>",
      streaming: false,
    });
  });

  it("setEntryRequestTokensEstimate is a no-op when the id matches no entry", () => {
    const placeholder = appendAssistantPlaceholder(
      clearConversationState(),
      "Assistant",
      undefined,
    );
    const after = setEntryRequestTokensEstimate(placeholder.state, "entry-9999", 12);
    // Untouched.
    expect(after.transcript).toEqual(placeholder.state.transcript);
  });

  it("createConversationState infers the next id past the largest entry-N id in the stored transcript", () => {
    // entry-7 is the largest id; nextId should be 8 regardless of the
    // nextId field in the stored payload (we take the max).
    const restored = createConversationState({
      nextId: 2,
      transcript: [
        { id: "entry-3", kind: "user", title: "You", body: "earlier" },
        { id: "entry-7", kind: "user", title: "You", body: "later" },
        // A non-matching id ("user-1") triggers the regex-miss branch
        // in inferNextId and must NOT push nextId.
        { id: "user-1", kind: "user", title: "You", body: "weird id" },
      ],
    });
    expect(restored.nextId).toBe(8);
  });

  it("summarizeTokenEstimates dedupes orchestrator-emitted user entries by body so reuse doesn't double-count", () => {
    const out = appendOrchestratorUserEntry(
      clearConversationState(),
      "shared body",
      "Tool",
      "orchestrator-tool",
    );
    // Second identical orchestrator emission should NOT double-count
    // its input tokens (chat-panel audit #11).
    const out2 = appendOrchestratorUserEntry(out.state, "shared body", "Tool", "orchestrator-tool");
    const totals = summarizeTokenEstimates(out2.state.transcript);
    // Only the first emission should contribute.
    const single = summarizeTokenEstimates(out.state.transcript);
    expect(totals.input).toBe(single.input);
  });

  it("filterPresentationEntries removes orchestrator artifact-only assistant turns AND legacy notes", () => {
    const filtered = filterPresentationEntries([
      { id: "entry-1", kind: "note", title: "Tool activity", body: "...", tone: "info" },
      { id: "entry-2", kind: "note", title: "Real note", body: "...", tone: "info" },
      {
        id: "entry-3",
        kind: "assistant",
        title: "sim-flow",
        body: ["```docs/spec.md", "# Spec", "```"].join("\n"),
        meta: "orchestrator",
      },
      { id: "entry-4", kind: "user", title: "You", body: "visible" },
    ]);
    // Legacy Tool activity note dropped; orchestrator artifact-only
    // assistant dropped; real note + user kept.
    expect(filtered.map((e) => e.id)).toEqual(["entry-2", "entry-4"]);
  });

  it("toStoredConversation strips streaming + protocol fences from orchestrator-meta assistants", () => {
    const stored = toStoredConversation({
      nextId: 5,
      transcript: [
        {
          id: "entry-1",
          kind: "assistant",
          title: "sim-flow",
          body: ["visible", "", "```tool:read_file", "{}", "```"].join("\n"),
          meta: "orchestrator",
          streaming: true,
        },
        {
          // Non-orchestrator assistant: streaming false but body preserved.
          id: "entry-2",
          kind: "assistant",
          title: "Assistant",
          body: ["visible", "", "```tool:read_file", "{}", "```"].join("\n"),
          streaming: true,
        },
      ],
    });
    const t = stored.transcript ?? [];
    expect(t).toHaveLength(2);
    // Orchestrator entry: protocol fences stripped, streaming false.
    expect(t[0]).toMatchObject({ id: "entry-1", streaming: false, body: "visible" });
    // Non-orchestrator entry: body left alone, streaming flipped off.
    expect(t[1]).toMatchObject({
      id: "entry-2",
      streaming: false,
    });
    expect((t[1] as { body: string }).body).toContain("```tool:read_file");
  });
});
