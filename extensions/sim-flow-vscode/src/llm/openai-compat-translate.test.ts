import { describe, expect, it } from "vitest";

import { transformMessagesForOpenAi } from "./openai-compat";
import type { LlmMessage } from "./types";

function user(content: string, attachments?: LlmMessage["attachments"]): LlmMessage {
  return { role: "user", content, attachments };
}
function assistant(content: string, attachments?: LlmMessage["attachments"]): LlmMessage {
  return { role: "assistant", content, attachments };
}
function system(content: string): LlmMessage {
  return { role: "system", content };
}

describe("transformMessagesForOpenAi", () => {
  it("passes plain conversations through unchanged", () => {
    const out = transformMessagesForOpenAi([
      system("You are an assistant."),
      user("Hi."),
      assistant("Hello!"),
      user("What's 2+2?"),
    ]);
    expect(out).toEqual([
      { role: "system", content: "You are an assistant." },
      { role: "user", content: "Hi." },
      { role: "assistant", content: "Hello!" },
      { role: "user", content: "What's 2+2?" },
    ]);
  });

  it("converts a single fenced tool call into native tool_calls", () => {
    const out = transformMessagesForOpenAi([
      user("read the spec"),
      assistant(
        "Let me check.\n\n```tool:read_file\n{\"path\":\"docs/spec.md\"}\n```\n",
      ),
    ]);
    expect(out).toEqual([
      { role: "user", content: "read the spec" },
      {
        role: "assistant",
        content: "Let me check.",
        tool_calls: [
          {
            id: "call_1_0",
            type: "function",
            function: {
              name: "read_file",
              arguments: '{"path":"docs/spec.md"}',
            },
          },
        ],
      },
    ]);
  });

  it("emits content: null for a tool-call-only assistant message", () => {
    const out = transformMessagesForOpenAi([
      user("read the spec"),
      assistant("```tool:read_file\n{\"path\":\"docs/spec.md\"}\n```\n"),
    ]);
    expect(out[1]).toMatchObject({
      role: "assistant",
      content: null,
      tool_calls: [
        {
          type: "function",
          function: { name: "read_file", arguments: '{"path":"docs/spec.md"}' },
        },
      ],
    });
  });

  it("pairs Tool-results user message into role: tool entries by index", () => {
    const conv: LlmMessage[] = [
      user("read the spec"),
      assistant("```tool:read_file\n{\"path\":\"docs/spec.md\"}\n```\n"),
      user(
        "Tool results:\n\n[read_file `docs/spec.md`]\n\n# Spec\nClock: 2 GHz\n\n---\n\n",
      ),
    ];
    const out = transformMessagesForOpenAi(conv);
    expect(out).toHaveLength(3);
    const assistantMsg = out[1] as Extract<(typeof out)[number], { role: "assistant" }>;
    const toolMsg = out[2] as Extract<(typeof out)[number], { role: "tool" }>;
    expect(toolMsg.role).toBe("tool");
    expect(toolMsg.tool_call_id).toBe(assistantMsg.tool_calls?.[0].id);
    expect(toolMsg.content).toContain("[read_file `docs/spec.md`]");
    expect(toolMsg.content).toContain("Clock: 2 GHz");
  });

  it("pairs multiple tool calls with multiple results", () => {
    const conv: LlmMessage[] = [
      user("explore"),
      assistant(
        "```tool:list_dir\nsrc/\n```\n\n```tool:read_file\n{\"path\":\"src/lib.rs\"}\n```\n",
      ),
      user(
        "Tool results:\n\n[list_dir `src/`]\n\n- lib.rs\n\n---\n\n[read_file `src/lib.rs`]\n\nfn main() {}\n",
      ),
    ];
    const out = transformMessagesForOpenAi(conv);
    expect(out).toHaveLength(4); // user, assistant, tool, tool
    const a = out[1] as Extract<(typeof out)[number], { role: "assistant" }>;
    expect(a.tool_calls).toHaveLength(2);
    const t1 = out[2] as Extract<(typeof out)[number], { role: "tool" }>;
    const t2 = out[3] as Extract<(typeof out)[number], { role: "tool" }>;
    expect(t1.tool_call_id).toBe(a.tool_calls?.[0].id);
    expect(t1.content).toContain("[list_dir `src/`]");
    expect(t2.tool_call_id).toBe(a.tool_calls?.[1].id);
    expect(t2.content).toContain("[read_file `src/lib.rs`]");
  });

  it("treats a non-Tool-results user message as plain content", () => {
    const conv: LlmMessage[] = [
      user("explore"),
      assistant("```tool:read_file\n{\"path\":\"x\"}\n```\n"),
      user("Actually never mind, do something else."),
    ];
    const out = transformMessagesForOpenAi(conv);
    // assistant emitted a tool_call but the next user msg isn't
    // tool-results — pass it through as a normal user message. The
    // ergonomic loss: OpenAI may reject the unmatched tool_call. We
    // surface that as an HTTP error rather than silently fabricating
    // a result; in practice the orchestrator only emits user
    // messages of the tool-results shape after assistant tool calls,
    // so this branch only fires under upstream protocol violations
    // or deliberate user injections.
    expect(out).toHaveLength(3);
    expect(out[2]).toEqual({
      role: "user",
      content: "Actually never mind, do something else.",
    });
  });

  it("falls back to pass-through for assistant messages with attachments", () => {
    // Defensive: tool fences only appear in plain text content; if
    // somebody attached an image to an assistant message, don't try
    // to extract.
    const conv: LlmMessage[] = [
      assistant("```tool:read_file\n{}\n```\n", [
        { mime: "image/png", data: "Zm9v" },
      ]),
    ];
    const out = transformMessagesForOpenAi(conv);
    expect(out).toHaveLength(1);
    expect((out[0] as { role: string }).role).toBe("assistant");
    expect((out[0] as { tool_calls?: unknown }).tool_calls).toBeUndefined();
  });

  it("normalizes a non-JSON args body to {raw: ...}", () => {
    const out = transformMessagesForOpenAi([
      assistant("```tool:read_file\nsrc/lib.rs\n```\n"),
    ]);
    const a = out[0] as Extract<(typeof out)[number], { role: "assistant" }>;
    const args = a.tool_calls?.[0].function.arguments;
    expect(args).toBe(JSON.stringify({ raw: "src/lib.rs" }));
  });

  it("clears pending tool_call ids after pairing so a later assistant turn starts fresh", () => {
    const conv: LlmMessage[] = [
      assistant("```tool:read_file\n{\"path\":\"a\"}\n```\n"),
      user("Tool results:\n\n[read_file `a`]\n\nA\n"),
      user("Now do the other one."),
    ];
    const out = transformMessagesForOpenAi(conv);
    // The third message should be a plain user message, not a stray tool result.
    expect(out[2]).toEqual({ role: "user", content: "Now do the other one." });
  });

  it("emits a residual user message when there are more results than calls", () => {
    const conv: LlmMessage[] = [
      assistant("```tool:read_file\n{\"path\":\"a\"}\n```\n"),
      user(
        "Tool results:\n\n[read_file `a`]\n\nA\n\n---\n\n[read_file `b`]\n\nB\n",
      ),
    ];
    const out = transformMessagesForOpenAi(conv);
    expect(out).toHaveLength(3);
    expect(out[1]).toMatchObject({ role: "tool" });
    // Surplus section becomes a regular user message so the model
    // still sees the data.
    expect(out[2]).toMatchObject({
      role: "user",
      content: expect.stringContaining("[read_file `b`]"),
    });
  });
});
