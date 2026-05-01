// Language-model backend that uses VS Code's built-in Language Model
// API. Copilot (and any other extension providing lm chat models) is
// surfaced here; no API key handling required from us.

import * as vscode from "vscode";

import {
  type CancellationLike,
  type LlmBackend,
  LlmError,
  type LlmMessage,
  type LlmStreamChunk,
  type LlmTool,
} from "./types";

export interface VSCodeBackendOptions {
  /** Optional model family/id filter, e.g. "gpt-4o" or "claude-sonnet-4.5". */
  model?: string;
}

export class VSCodeLmBackend implements LlmBackend {
  readonly name = "vscode.lm";

  constructor(private readonly options: VSCodeBackendOptions = {}) {}

  async *stream(
    messages: LlmMessage[],
    token: CancellationLike,
    _tools?: LlmTool[],
  ): AsyncIterable<LlmStreamChunk> {
    // vscode.lm has its own tool-use surface that's tied to chat
    // participants, not vendor-neutral. Sim-flow stays on the
    // fenced-block fallback for the vscode.lm path.
    void _tools;
    const models = await selectModel(this.options.model);
    if (!models) {
      throw new LlmError(
        "no-model",
        "VS Code Language Model API returned no usable models.",
        "Install Copilot (or another provider that implements the Language Model API), then retry.",
      );
    }

    const vmsgs = toVscodeMessages(messages);
    // `justification` gives VS Code the user-facing reason for the
    // consent prompt some providers gate behind. Without it, certain
    // chat-model providers silently return empty without throwing.
    const request = await models.sendRequest(
      vmsgs,
      { justification: "sim-flow: orchestrator-driven model implementation step" },
      cancellationAsVscode(token),
    );

    // Iterate the full `stream` rather than `text`-only. Some
    // providers (e.g. the Claude Code extension when it has
    // built-in tools enabled) emit `LanguageModelToolCallPart`
    // parts in response to prompts that mention tools, and those
    // never appear on the text-only stream. We surface them as
    // synthesized fenced `tool:<name>` blocks so the orchestrator's
    // existing `extract_tool_calls` dispatcher picks them up the
    // same way it does for the openai-compat backend.
    type StreamPart = unknown;
    const responseStream = (
      request as unknown as { stream?: AsyncIterable<StreamPart> }
    ).stream;
    let sawAnything = false;
    const unrecognized: string[] = [];
    try {
      if (responseStream) {
        for await (const part of responseStream) {
          if (token.isCancellationRequested) {
            break;
          }
          const text = extractTextPart(part);
          if (text !== null && text.length > 0) {
            sawAnything = true;
            yield { text };
            continue;
          }
          const toolBlock = extractToolCallPart(part);
          if (toolBlock !== null) {
            sawAnything = true;
            yield { text: toolBlock };
            continue;
          }
          // Unknown part shape -- collect a description so the
          // empty-stream error message can include it; helps diagnose
          // a future VS Code build adding a part type we should handle.
          unrecognized.push(describePart(part));
        }
      } else {
        // Older VS Code builds may only expose `.text`. Fall back so
        // we don't break the existing path.
        for await (const fragment of request.text) {
          if (token.isCancellationRequested) {
            break;
          }
          if (fragment.length > 0) {
            sawAnything = true;
            yield { text: fragment };
          }
        }
      }
    } catch (err) {
      if (token.isCancellationRequested) {
        throw new LlmError("cancelled", "LLM stream cancelled.");
      }
      throw new LlmError(
        "http",
        `vscode.lm sendRequest failed: ${(err as Error).message ?? String(err)}`,
        undefined,
        err,
      );
    }
    if (!sawAnything && !token.isCancellationRequested) {
      // Provider returned an empty stream WITHOUT throwing. Common
      // causes: extension consent not granted; content filter
      // rejection; the model emitted only an unrecognized part
      // type. Surface the failure to the chat pane (instead of
      // letting the orchestrator's empty-response retry burn turns)
      // so the user sees something actionable.
      const detail =
        unrecognized.length > 0
          ? `Stream contained ${unrecognized.length} unrecognized part(s) (${unrecognized
              .slice(0, 3)
              .join(", ")}${unrecognized.length > 3 ? ", ..." : ""}). The model may have emitted a content type sim-flow doesn't decode.`
          : "The provider returned no text and no tool calls. Common causes: extension consent not granted (look for an Allow dialog), content-filter rejection, or a network issue between VS Code and the provider.";
      throw new LlmError("http", "vscode.lm returned an empty response", detail);
    }
  }
}

/**
 * Pull text out of a stream part. VS Code exposes
 * `LanguageModelTextPart` with a `.value: string`. Older builds
 * sometimes yield bare strings (the legacy `.text` shape) — we
 * accept either.
 */
export function extractTextPart(part: unknown): string | null {
  if (typeof part === "string") {
    return part;
  }
  if (
    part &&
    typeof part === "object" &&
    typeof (part as { value?: unknown }).value === "string" &&
    !("name" in part) // disambiguate from tool-call parts
  ) {
    return (part as { value: string }).value;
  }
  return null;
}

/**
 * Render a `LanguageModelToolCallPart` as a sim-flow fenced
 * `tool:<name>` block whose body is the call's arguments JSON.
 * VS Code's part shape is `{ name, callId, input: any }` -- we
 * stringify `input` as JSON and let the orchestrator dispatch.
 * Returns null when the part isn't recognizable as a tool call.
 */
export function extractToolCallPart(part: unknown): string | null {
  if (!part || typeof part !== "object") {
    return null;
  }
  const p = part as { name?: unknown; input?: unknown };
  if (typeof p.name !== "string" || p.name.length === 0) {
    return null;
  }
  let body: string;
  if (p.input === undefined || p.input === null) {
    body = "{}";
  } else if (typeof p.input === "string") {
    try {
      JSON.parse(p.input);
      body = p.input;
    } catch {
      body = JSON.stringify({ raw: p.input });
    }
  } else {
    try {
      body = JSON.stringify(p.input);
    } catch {
      body = "{}";
    }
  }
  return `\n\n\`\`\`tool:${p.name}\n${body}\n\`\`\`\n`;
}

function describePart(part: unknown): string {
  if (part === null) {return "null";}
  if (typeof part !== "object") {return typeof part;}
  const ctor = (part as { constructor?: { name?: string } }).constructor?.name;
  return ctor ?? "object";
}

async function selectModel(
  prefer: string | undefined,
): Promise<vscode.LanguageModelChat | undefined> {
  // The configured value can be one of:
  //   - "" / undefined -> any available model
  //   - "<family>"     -> any vendor that offers that family
  //   - "<vendor>/<family>" -> exact match on both axes
  // The vendor-scoped form is what the dashboard's model dropdown
  // emits when multiple registered providers offer overlapping
  // families (e.g. Copilot AND Claude Code both publish a
  // `claude-sonnet-4.6`); without it `selectChatModels({family})`
  // returns whatever vendor was registered first, which gave us
  // the "you've exhausted Copilot's quota" surprise.
  const { vendor, family } = parseModelHint(prefer);
  const selector: vscode.LanguageModelChatSelector = {};
  if (vendor) {selector.vendor = vendor;}
  if (family) {selector.family = family;}
  let models = await vscode.lm.selectChatModels(selector);
  if (models.length > 0) {
    return models[0];
  }
  // Fall back: drop the most-restrictive scope and retry. Saves the
  // user from a hard fail when the configured vendor/family pair
  // doesn't match anything that's currently registered (e.g. they
  // uninstalled Claude Code but the setting still names it).
  if (vendor && family) {
    models = await vscode.lm.selectChatModels({ family });
    if (models.length > 0) {
      return models[0];
    }
  }
  if (prefer) {
    const any = await vscode.lm.selectChatModels({});
    return any[0];
  }
  return undefined;
}

/**
 * Split a `<vendor>/<family>` hint on the first `/`. Plain strings
 * (no `/`) are treated as bare family names so the legacy single-
 * field setting keeps working.
 */
export function parseModelHint(value: string | undefined): {
  vendor?: string;
  family?: string;
} {
  if (!value) {return {};}
  const idx = value.indexOf("/");
  if (idx === -1) {
    return { family: value };
  }
  const vendor = value.slice(0, idx).trim();
  const family = value.slice(idx + 1).trim();
  return {
    vendor: vendor.length > 0 ? vendor : undefined,
    family: family.length > 0 ? family : undefined,
  };
}

function toVscodeMessages(messages: LlmMessage[]): vscode.LanguageModelChatMessage[] {
  return messages.map((m) => {
    const text = m.role === "system" ? `[system] ${m.content}` : m.content;
    const parts = buildContentParts(text, m);
    switch (m.role) {
      case "system":
      case "user":
        // vscode.lm has no system role; we already prepended `[system]`.
        return vscode.LanguageModelChatMessage.User(parts);
      case "assistant":
        return vscode.LanguageModelChatMessage.Assistant(parts);
    }
  });
}

type LmPart = vscode.LanguageModelTextPart | vscode.LanguageModelDataPart;

/**
 * Build the message content parts for a vscode.lm chat message. When
 * the message has no attachments we fall back to a plain string so
 * older vscode.lm builds (which only accept `string`) keep working.
 * When attachments are present we emit a text part followed by one
 * data part per attachment; vscode.lm forwards the data parts to
 * any underlying multimodal model that accepts them.
 */
function buildContentParts(text: string, m: LlmMessage): string | LmPart[] {
  if (!m.attachments || m.attachments.length === 0) {
    return text;
  }
  const parts: LmPart[] = [];
  if (text.length > 0) {
    parts.push(new vscode.LanguageModelTextPart(text));
  }
  for (const att of m.attachments) {
    try {
      const bytes = Buffer.from(att.data, "base64");
      parts.push(vscode.LanguageModelDataPart.image(bytes, att.mime));
    } catch (err) {
      // Skip a malformed attachment but keep the rest of the message
      // intact -- losing the image is preferable to dropping the
      // whole turn.
      console.warn(
        `sim-flow: skipping malformed attachment (${att.source ?? "<unknown>"}): ${
          (err as Error).message ?? String(err)
        }`,
      );
    }
  }
  return parts;
}

function cancellationAsVscode(token: CancellationLike): vscode.CancellationToken {
  // If we already received a real vscode token, pass it through
  // (duck-typed); otherwise wrap in a synthetic token.
  const maybe = token as unknown as { onCancellationRequested?: unknown };
  if (typeof maybe.onCancellationRequested === "function") {
    return token as unknown as vscode.CancellationToken;
  }
  const src = new vscode.CancellationTokenSource();
  if (token.isCancellationRequested) {
    src.cancel();
  }
  return src.token;
}
