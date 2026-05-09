// Anthropic Messages API backend. v1 uses a single POST (not
// streaming) to keep the implementation small; a streaming upgrade is
// tracked as a polish pass in the Phase 8 plan. The key is resolved
// via the shared key chain (env var → `<config>/sim-flow/credentials.toml`
// → VS Code SecretStorage); see `keyResolver.ts`.

import { envVarFor, resolveApiKey, secretIdFor } from "./keyResolver";
import {
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
  secrets?: SecretStorage;
  apiUrl?: string;
  /** Max tokens for the response. Anthropic requires this field. */
  maxTokens?: number;
  /** Injectable fetch for tests. */
  fetchImpl?: typeof fetch;
}

export class AnthropicBackend implements LlmBackend {
  readonly name = "anthropic";

  constructor(private readonly options: AnthropicBackendOptions = {}) {}

  async *stream(
    messages: LlmMessage[],
    token: CancellationLike,
    _tools?: LlmTool[],
  ): AsyncIterable<LlmStreamChunk> {
    // Anthropic native tool-use is on the v2 roadmap; v1 stays with
    // the fenced-block fallback driven by sim-flow's system prompt.
    void _tools;
    const apiKey = await this.readApiKey();
    if (token.isCancellationRequested) {
      throw new LlmError("cancelled", "Anthropic request cancelled.");
    }
    const url = this.options.apiUrl ?? "https://api.anthropic.com/v1/messages";
    const model = this.options.model ?? "claude-sonnet-4-6";

    const body = {
      model,
      max_tokens: this.options.maxTokens ?? 4096,
      system: extractSystem(messages),
      messages: convertMessages(messages),
    };

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
    const text = extractAnthropicText(json);
    if (text.length > 0) {
      yield { text };
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

/**
 * Anthropic takes the system prompt out of the messages array and into
 * a dedicated `system` field. Collapse consecutive system messages.
 */
function extractSystem(messages: LlmMessage[]): string | undefined {
  const systems = messages.filter((m) => m.role === "system").map((m) => m.content);
  if (systems.length === 0) {
    return undefined;
  }
  return systems.join("\n\n");
}

/** Strip system messages and map remaining roles to Anthropic shapes. */
function convertMessages(
  messages: LlmMessage[],
): Array<{ role: "user" | "assistant"; content: AnthropicContent }> {
  return messages
    .filter((m) => m.role !== "system")
    .map((m) => ({
      role: m.role as "user" | "assistant",
      content: anthropicContent(m),
    }));
}

type AnthropicContent =
  | string
  | Array<
      | { type: "text"; text: string }
      | {
          type: "image";
          source: { type: "base64"; media_type: string; data: string };
        }
    >;

/**
 * Anthropic supports multimodal via `content` arrays of typed blocks.
 * When the message has no attachments we keep the simpler string
 * form (which Anthropic happily accepts); attachments produce a
 * mixed text+image block array.
 */
function anthropicContent(m: LlmMessage): AnthropicContent {
  if (!m.attachments || m.attachments.length === 0) {
    return m.content;
  }
  const parts: Exclude<AnthropicContent, string> = [];
  if (m.content.length > 0) {
    parts.push({ type: "text", text: m.content });
  }
  for (const att of m.attachments) {
    parts.push({
      type: "image",
      source: { type: "base64", media_type: att.mime, data: att.data },
    });
  }
  return parts;
}

export function extractAnthropicText(json: unknown): string {
  if (!json || typeof json !== "object") {
    return "";
  }
  const body = json as { content?: unknown };
  const content = body.content;
  if (!Array.isArray(content)) {
    return "";
  }
  return content
    .filter(
      (c): c is { type: "text"; text: string } =>
        !!c && typeof c === "object" && (c as { type: unknown }).type === "text",
    )
    .map((c) => c.text)
    .join("");
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
