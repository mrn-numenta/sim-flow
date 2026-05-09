// Anthropic Messages API backend. v1 uses a single POST (not
// streaming) to keep the implementation small; a streaming upgrade is
// tracked as a polish pass in the Phase 8 plan. The key is resolved
// via the shared key chain (env var → `<config>/sim-flow/credentials.toml`
// → VS Code SecretStorage); see `keyResolver.ts`.

import { envVarFor, resolveApiKey, secretIdFor } from "./keyResolver";
import {
  extractToolFences,
  makeToolCallId,
  parseToolResultsEnvelope,
  type ParsedToolCall,
} from "./tool-translation";
import {
  ANTHROPIC_MESSAGES_RUNTIME,
  prepareAnthropicMessages,
  resolveRuntimeProfile,
} from "./runtimeProfiles";
import {
  applyModelFamilyPromptPolicy,
  applyReasoningHistoryPolicy,
  GENERIC_CHAT_MODEL_FAMILY,
  orderAttachmentsByFamily,
  resolveModelFamily,
} from "./modelFamilies";
import { createResponseNormalizerForFamily } from "./responseNormalizers";
import {
  type LlmAdaptationProfile,
  type CancellationLike,
  type LlmBackend,
  LlmError,
  type LlmMessage,
  type LlmStreamChunk,
  type LlmTool,
  type SecretStorage,
} from "./types";

/**
 * SecretStorage key id for the Anthropic API key. Re-exported for
 * back-compat with callers (`apiKey.ts`, the migration test) that
 * referenced this constant before `keyResolver.ts` centralized the
 * naming. New code should call `secretIdFor("anthropic")` instead.
 */
export const ANTHROPIC_KEY_ID = secretIdFor("anthropic");

export interface AnthropicBackendOptions {
  model?: string;
  modelFamilyId?: string;
  runtimeProfileId?: string;
  secrets?: SecretStorage;
  apiUrl?: string;
  /** Max tokens for the response. Anthropic requires this field. */
  maxTokens?: number;
  /** Injectable fetch for tests. */
  fetchImpl?: typeof fetch;
}

export class AnthropicBackend implements LlmBackend {
  readonly name = "anthropic";
  readonly adaptation: LlmAdaptationProfile;

  constructor(private readonly options: AnthropicBackendOptions = {}) {
    const modelFamily = resolveModelFamily(this.options.modelFamilyId, this.options.model);
    let runtime = ANTHROPIC_MESSAGES_RUNTIME;
    try {
      runtime = resolveRuntimeProfile(this.options.runtimeProfileId, ANTHROPIC_MESSAGES_RUNTIME, [
        ANTHROPIC_MESSAGES_RUNTIME.id,
      ]);
    } catch (err) {
      throw new LlmError("unsupported", (err as Error).message);
    }
    this.adaptation = {
      runtime,
      modelFamily,
      responseNormalizer: createResponseNormalizerForFamily(modelFamily),
    };
  }

  async *stream(
    messages: LlmMessage[],
    token: CancellationLike,
    tools?: LlmTool[],
  ): AsyncIterable<LlmStreamChunk> {
    const apiKey = await this.readApiKey();
    if (token.isCancellationRequested) {
      throw new LlmError("cancelled", "Anthropic request cancelled.");
    }
    const url = this.options.apiUrl ?? "https://api.anthropic.com/v1/messages";
    const model = this.options.model ?? "claude-sonnet-4-6";

    const modelFamily = this.adaptation.modelFamily;
    const prepared = this.adaptation.runtime.prepareInput
      ? this.adaptation.runtime.prepareInput(messages)
      : { messages };
    const familyInput = applyModelFamilyPromptPolicy(
      {
        ...prepared,
        messages: applyReasoningHistoryPolicy(prepared.messages, modelFamily),
      },
      modelFamily,
    );
    const body: {
      model: string;
      max_tokens: number;
      system?: string;
      messages: Array<{ role: "user" | "assistant"; content: AnthropicContent }>;
      tools?: AnthropicTool[];
    } = {
      model,
      max_tokens: this.options.maxTokens ?? 4096,
      system: familyInput.system,
      messages: convertMessagesForAnthropic(familyInput.messages, modelFamily),
    };
    if (tools && tools.length > 0) {
      body.tools = tools.map(toAnthropicTool);
    }

    const doFetch = this.options.fetchImpl ?? globalThis.fetch;
    if (typeof doFetch !== "function") {
      throw new LlmError(
        "unsupported",
        "`fetch` is not available; Node 18+ or a polyfill is required for the Anthropic backend.",
      );
    }

    const controller = new AbortController();
    const cancelSubscription = token.onCancellationRequested?.(() => {
      controller.abort();
    });
    let res: Response;
    try {
      res = await doFetch(url, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-api-key": apiKey,
          "anthropic-version": "2023-06-01",
        },
        body: JSON.stringify(body),
        signal: controller.signal,
      });
    } catch (err) {
      cancelSubscription?.dispose();
      if (token.isCancellationRequested || isAbortError(err)) {
        throw new LlmError("cancelled", "Anthropic request cancelled.");
      }
      throw new LlmError(
        "http",
        `Anthropic request failed: ${(err as Error).message ?? String(err)}`,
        undefined,
        err,
      );
    }
    if (!res.ok) {
      cancelSubscription?.dispose();
      const detail = await safeText(res);
      throw new LlmError("http", `Anthropic API returned ${res.status} ${res.statusText}`, detail);
    }
    const json = (await res.json()) as unknown;
    cancelSubscription?.dispose();
    for (const chunk of extractAnthropicChunks(json)) {
      yield chunk;
    }
  }

  private async readApiKey(): Promise<string> {
    // Resolution chain: env var → shared credentials.toml → VS Code
    // SecretStorage. The first two work outside VS Code (the CLI's
    // Rust resolver shares the same on-disk file), so a user who
    // runs `sim-flow auto` from a terminal picks up the same key
    // the extension does.
    const resolved = await resolveApiKey("anthropic", this.options.secrets);
    if (resolved) {
      return resolved.key;
    }
    throw new LlmError(
      "missing-api-key",
      this.options.secrets
        ? `Anthropic API key not found. Set ${envVarFor("anthropic")} in your shell, run \`sim-flow keys set anthropic\`, or use the "sim-flow: Set LLM API Key" command and pick "VS Code keychain".`
        : `Anthropic API key not found. Set ${envVarFor("anthropic")} in your shell or run \`sim-flow keys set anthropic\`.`,
    );
  }
}

/** Strip system messages and map remaining roles to Anthropic native shapes. */
function convertMessagesForAnthropic(
  messages: LlmMessage[],
  modelFamily = GENERIC_CHAT_MODEL_FAMILY,
): Array<{ role: "user" | "assistant"; content: AnthropicContent }> {
  const out: Array<{ role: "user" | "assistant"; content: AnthropicContent }> = [];
  let pendingToolUses: AnthropicToolUseBlock[] = [];

  for (let i = 0; i < messages.length; i += 1) {
    const message = messages[i];
    if (message.role === "system") {
      continue;
    }

    if (message.role === "assistant") {
      if (message.attachments && message.attachments.length > 0) {
        out.push({ role: "assistant", content: anthropicContent(message, modelFamily) });
        pendingToolUses = [];
        continue;
      }
      const { content, toolCalls } = extractToolFences(message.content);
      if (toolCalls.length === 0) {
        out.push({ role: "assistant", content: anthropicContent(message, modelFamily) });
        pendingToolUses = [];
        continue;
      }
      const blocks: Exclude<AnthropicContent, string> = [];
      if (content.length > 0) {
        blocks.push({ type: "text", text: content });
      }
      const toolUses = toolCalls.map((tc: ParsedToolCall, idx: number) =>
        toAnthropicToolUseBlock(tc, i, idx),
      );
      blocks.push(...toolUses);
      out.push({ role: "assistant", content: blocks });
      pendingToolUses = toolUses;
      continue;
    }

    if (pendingToolUses.length > 0 && (!message.attachments || message.attachments.length === 0)) {
      const sections = parseToolResultsEnvelope(message.content);
      if (sections !== null) {
        const blocks: Exclude<AnthropicContent, string> = [];
        const paired = Math.min(sections.length, pendingToolUses.length);
        for (let idx = 0; idx < paired; idx += 1) {
          blocks.push({
            type: "tool_result",
            tool_use_id: pendingToolUses[idx].id,
            content: sections[idx],
          });
        }
        if (sections.length > paired) {
          for (const extra of sections.slice(paired)) {
            blocks.push({ type: "text", text: extra });
          }
        }
        out.push({ role: "user", content: blocks });
        pendingToolUses = [];
        continue;
      }
    }

    pendingToolUses = [];
    out.push({ role: "user", content: anthropicContent(message, modelFamily) });
  }

  return out;
}

type AnthropicContent =
  | string
  | Array<
      | { type: "text"; text: string }
      | {
          type: "image";
          source: { type: "base64"; media_type: string; data: string };
        }
      | AnthropicToolUseBlock
      | {
          type: "tool_result";
          tool_use_id: string;
          content: string;
        }
    >;

type AnthropicToolUseBlock = {
  type: "tool_use";
  id: string;
  name: string;
  input: Record<string, unknown>;
};

type AnthropicTool = {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
};

/**
 * Anthropic supports multimodal via `content` arrays of typed blocks.
 * When the message has no attachments we keep the simpler string
 * form (which Anthropic happily accepts); attachments produce a
 * mixed text+image block array.
 */
function anthropicContent(
  m: LlmMessage,
  modelFamily = GENERIC_CHAT_MODEL_FAMILY,
): AnthropicContent {
  if (!m.attachments || m.attachments.length === 0) {
    return m.content;
  }
  const parts: Exclude<AnthropicContent, string> = [];
  for (const part of orderAttachmentsByFamily(modelFamily, m.content, m.attachments)) {
    if (part.kind === "text") {
      parts.push({ type: "text", text: part.text });
      continue;
    }
    parts.push({
      type: "image",
      source: {
        type: "base64",
        media_type: part.attachment.mime,
        data: part.attachment.data,
      },
    });
  }
  return parts;
}

export function extractAnthropicText(json: unknown): string {
  return extractAnthropicChunks(json)
    .filter((chunk) => chunk.kind === undefined || chunk.kind === "content")
    .map((chunk) => chunk.text)
    .join("");
}

export { prepareAnthropicMessages };

export function extractAnthropicChunks(json: unknown): LlmStreamChunk[] {
  if (!json || typeof json !== "object") {
    return [];
  }
  const body = json as { content?: unknown };
  const content = body.content;
  if (!Array.isArray(content)) {
    return [];
  }

  const out: LlmStreamChunk[] = [];
  for (const block of content) {
    if (!block || typeof block !== "object") {
      continue;
    }
    const typed = block as {
      type?: unknown;
      text?: unknown;
      thinking?: unknown;
      name?: unknown;
      input?: unknown;
    };
    if (typed.type === "text" && typeof typed.text === "string") {
      out.push({ text: typed.text, kind: "content" });
      continue;
    }
    if (typed.type === "thinking") {
      const thinkingText =
        typeof typed.thinking === "string"
          ? typed.thinking
          : typeof typed.text === "string"
            ? typed.text
            : "";
      if (thinkingText.length > 0) {
        out.push({ text: thinkingText, kind: "reasoning" });
      }
      continue;
    }
    if (typed.type === "tool_use" && typeof typed.name === "string" && typed.name.length > 0) {
      let inputBody = "{}";
      if (typeof typed.input === "string") {
        inputBody = typed.input;
      } else if (typed.input && typeof typed.input === "object") {
        inputBody = JSON.stringify(typed.input);
      }
      out.push({
        text: `\n\n\`\`\`tool:${typed.name}\n${inputBody}\n\`\`\`\n`,
        kind: "tool_call",
      });
    }
  }
  return out;
}

function toAnthropicTool(tool: LlmTool): AnthropicTool {
  return {
    name: tool.name,
    description: tool.description,
    input_schema: tool.args_schema,
  };
}

function toAnthropicToolUseBlock(
  toolCall: ParsedToolCall,
  messageIndex: number,
  callIndex: number,
): AnthropicToolUseBlock {
  return {
    type: "tool_use",
    id: makeToolCallId(messageIndex, callIndex),
    name: toolCall.name,
    input: parseAnthropicToolInput(toolCall.args),
  };
}

function parseAnthropicToolInput(args: string): Record<string, unknown> {
  const trimmed = args.trim();
  if (trimmed.length === 0) {
    return {};
  }
  try {
    const parsed = JSON.parse(trimmed);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      return parsed as Record<string, unknown>;
    }
    return { raw: parsed };
  } catch {
    return { raw: trimmed };
  }
}

async function safeText(res: Response): Promise<string | undefined> {
  try {
    return await res.text();
  } catch {
    return undefined;
  }
}

function isAbortError(err: unknown): boolean {
  return !!err && typeof err === "object" && (err as { name?: unknown }).name === "AbortError";
}
