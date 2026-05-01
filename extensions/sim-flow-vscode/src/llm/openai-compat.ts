// Shared implementation for OpenAI Chat Completions-compatible
// backends (OpenAI, Ollama, LM Studio). Subclasses fix the provider
// name, default base URL, default model, and key policy; the wire
// format is identical.

import {
  type CancellationLike,
  type LlmBackend,
  LlmError,
  type LlmMessage,
  type LlmStreamChunk,
  type LlmTool,
  type SecretStorage,
} from "./types";

export interface OpenAiCompatibleBackendOptions {
  /** Human-readable backend name (shown in the chat header). */
  name: string;
  /** Base URL, e.g. "https://api.openai.com/v1". No trailing slash. */
  baseUrl: string;
  /** Default model id when the caller doesn't supply one. */
  defaultModel: string;
  /** Optional API key identifier in SecretStorage. */
  keyId?: string;
  /** When true, the backend throws `missing-api-key` if the key is absent. */
  requireKey: boolean;
  /** Override for the request path; defaults to "/chat/completions". */
  path?: string;

  /** Caller-supplied overrides. */
  model?: string;
  secrets?: SecretStorage;
  /** Absolute override URL. Takes precedence over baseUrl + path. */
  apiUrl?: string;
  fetchImpl?: typeof fetch;
}

export class OpenAiCompatibleBackend implements LlmBackend {
  readonly name: string;

  constructor(protected readonly options: OpenAiCompatibleBackendOptions) {
    this.name = options.name;
  }

  async *stream(
    messages: LlmMessage[],
    token: CancellationLike,
    tools?: LlmTool[],
  ): AsyncIterable<LlmStreamChunk> {
    const apiKey = await this.resolveApiKey();
    if (token.isCancellationRequested) {
      throw new LlmError("cancelled", `${this.options.name} request cancelled.`);
    }
    const url =
      this.options.apiUrl ??
      `${trimTrailingSlash(this.options.baseUrl)}${this.options.path ?? "/chat/completions"}`;
    const model = this.options.model ?? this.options.defaultModel;

    // Always request SSE streaming. For local OpenAI-compatible
    // servers (LM Studio, Ollama) a single non-streaming POST can
    // sit open for many minutes while the model generates; Node's
    // global fetch has a 300 s body timeout that aborts long
    // generations as "fetch failed". With `stream: true` each delta
    // resets the timer, the user sees tokens in real time, and the
    // timeout stops being a problem.
    const body: {
      model: string;
      messages: Array<{ role: string; content: OpenAiContent }>;
      tools?: OpenAiTool[];
      stream: boolean;
    } = {
      model,
      messages: messages.map((m) => ({ role: m.role, content: openAiContent(m) })),
      stream: true,
    };
    if (tools && tools.length > 0) {
      body.tools = tools.map(toOpenAiTool);
    }

    const doFetch = this.options.fetchImpl ?? globalThis.fetch;
    if (typeof doFetch !== "function") {
      throw new LlmError(
        "unsupported",
        `\`fetch\` is not available; Node 18+ or a polyfill is required for the ${this.options.name} backend.`,
      );
    }

    const headers: Record<string, string> = {
      "content-type": "application/json",
      accept: "text/event-stream",
    };
    if (apiKey) {
      headers.authorization = `Bearer ${apiKey}`;
    }

    const controller = new AbortController();
    const res = await doFetch(url, {
      method: "POST",
      headers,
      body: JSON.stringify(body),
      signal: controller.signal,
    });
    if (!res.ok) {
      const detail = await safeText(res);
      throw new LlmError(
        "http",
        `${this.options.name} API returned ${res.status} ${res.statusText}`,
        detail,
      );
    }
    if (!res.body) {
      throw new LlmError(
        "parse",
        `${this.options.name} returned no response body for streaming request`,
      );
    }

    // Per-call accumulator for tool_calls. Streamed deltas arrive as
    // fragments keyed by `index`; we concatenate `function.arguments`
    // pieces and remember the first-seen `function.name`. On stream
    // end, we synthesize one fenced `tool:<name>` block per index so
    // the orchestrator's existing `extract_tool_calls` parser picks
    // them up unchanged.
    const accumulator = new Map<number, { name: string; args: string }>();
    let buffer = "";
    const decoder = new TextDecoder("utf-8");

    try {
      for await (const chunk of res.body as unknown as AsyncIterable<Uint8Array>) {
        if (token.isCancellationRequested) {
          controller.abort();
          throw new LlmError("cancelled", `${this.options.name} stream cancelled.`);
        }
        buffer += decoder.decode(chunk, { stream: true });

        let eventEnd: number;
        while ((eventEnd = buffer.indexOf("\n\n")) >= 0) {
          const eventText = buffer.slice(0, eventEnd);
          buffer = buffer.slice(eventEnd + 2);
          const dataLines = eventText
            .split("\n")
            .filter((l) => l.startsWith("data: "))
            .map((l) => l.slice(6));
          if (dataLines.length === 0) {
            continue;
          }
          const payload = dataLines.join("\n");
          if (payload === "[DONE]") {
            for (const block of synthesizeAccumulatedTools(accumulator)) {
              yield { text: block };
            }
            return;
          }
          let parsed: unknown;
          try {
            parsed = JSON.parse(payload);
          } catch {
            // Skip malformed events; servers occasionally emit
            // keep-alive comments or partial fragments.
            continue;
          }
          const delta = readDelta(parsed);
          if (delta?.reasoning && delta.reasoning.length > 0) {
            yield { text: delta.reasoning, kind: "reasoning" };
          }
          if (delta?.content && delta.content.length > 0) {
            yield { text: delta.content, kind: "content" };
          }
          if (delta?.tool_calls) {
            absorbToolCallDeltas(accumulator, delta.tool_calls);
          }
        }
      }
      // Body ended without an explicit `[DONE]`. Emit any
      // tool-call blocks we accumulated.
      for (const block of synthesizeAccumulatedTools(accumulator)) {
        yield { text: block };
      }
    } catch (err) {
      if (err instanceof LlmError) {
        throw err;
      }
      throw new LlmError(
        "http",
        `${this.options.name} stream error: ${(err as Error).message ?? String(err)}`,
        undefined,
        err,
      );
    }
  }

  protected async resolveApiKey(): Promise<string | undefined> {
    if (!this.options.keyId) {
      return undefined;
    }
    const key = this.options.secrets
      ? await this.options.secrets.get(this.options.keyId)
      : undefined;
    if (!key || key.length === 0) {
      if (this.options.requireKey) {
        throw new LlmError(
          "missing-api-key",
          `${this.options.name} API key is not set. Run the command "sim-flow: Set LLM API Key" and paste your ${this.options.keyId}.`,
        );
      }
      return undefined;
    }
    return key;
  }
}

export function extractOpenAiText(json: unknown): string {
  if (!json || typeof json !== "object") {
    return "";
  }
  const body = json as { choices?: unknown };
  const choices = body.choices;
  if (!Array.isArray(choices) || choices.length === 0) {
    return "";
  }
  const first = choices[0] as { message?: { content?: unknown } };
  const content = first.message?.content;
  return typeof content === "string" ? content : "";
}

/**
 * Render every native `tool_calls` entry in the response into a
 * sim-flow fenced `tool:<name>` block whose body is the call's
 * arguments JSON. Returns one string per block (caller yields them
 * as separate chunks). Empty when there are no tool calls.
 *
 * Wrapping with `\n\n` on both sides ensures the orchestrator's
 * line-based extractor sees a clean fence even when the model also
 * produced text content immediately before / after.
 */
export function extractOpenAiToolCalls(json: unknown): string[] {
  if (!json || typeof json !== "object") {
    return [];
  }
  const body = json as { choices?: unknown };
  const choices = body.choices;
  if (!Array.isArray(choices) || choices.length === 0) {
    return [];
  }
  const first = choices[0] as {
    message?: { tool_calls?: unknown };
  };
  const toolCalls = first.message?.tool_calls;
  if (!Array.isArray(toolCalls)) {
    return [];
  }
  const out: string[] = [];
  for (const raw of toolCalls) {
    const tc = raw as {
      type?: unknown;
      function?: { name?: unknown; arguments?: unknown };
    };
    if (tc?.type !== "function") {
      continue;
    }
    const name = typeof tc.function?.name === "string" ? tc.function.name : "";
    if (name.length === 0) {
      continue;
    }
    const args = tc.function?.arguments;
    let argsBody: string;
    if (typeof args === "string") {
      // Provider returned arguments as a JSON-encoded string. Pass
      // through verbatim if it parses; otherwise wrap as `{"raw": ...}`
      // so the orchestrator gets *something* and the failure surfaces
      // as a per-tool error rather than a parse crash.
      try {
        JSON.parse(args);
        argsBody = args;
      } catch {
        argsBody = JSON.stringify({ raw: args });
      }
    } else if (args && typeof args === "object") {
      argsBody = JSON.stringify(args);
    } else {
      argsBody = "{}";
    }
    out.push(`\n\n\`\`\`tool:${name}\n${argsBody}\n\`\`\`\n`);
  }
  return out;
}

interface OpenAiTool {
  type: "function";
  function: {
    name: string;
    description: string;
    parameters: Record<string, unknown>;
  };
}

function toOpenAiTool(tool: LlmTool): OpenAiTool {
  return {
    type: "function",
    function: {
      name: tool.name,
      description: tool.description,
      parameters: tool.args_schema,
    },
  };
}

function trimTrailingSlash(s: string): string {
  return s.endsWith("/") ? s.slice(0, -1) : s;
}

type OpenAiContent =
  | string
  | Array<
      | { type: "text"; text: string }
      | { type: "image_url"; image_url: { url: string } }
    >;

/**
 * OpenAI-compatible APIs (OpenAI, Ollama, LM Studio) accept either a
 * plain string or a content array of typed parts. Images go through
 * as `image_url` blocks with a base64 data URI; that shape is also
 * what GPT-4o vision and Llava expect when using the openai-compat
 * surface.
 */
function openAiContent(m: LlmMessage): OpenAiContent {
  if (!m.attachments || m.attachments.length === 0) {
    return m.content;
  }
  const parts: Exclude<OpenAiContent, string> = [];
  if (m.content.length > 0) {
    parts.push({ type: "text", text: m.content });
  }
  for (const att of m.attachments) {
    parts.push({
      type: "image_url",
      image_url: { url: `data:${att.mime};base64,${att.data}` },
    });
  }
  return parts;
}

async function safeText(res: Response): Promise<string | undefined> {
  try {
    return await res.text();
  } catch {
    return undefined;
  }
}

/**
 * Shape of `choices[0].delta` we care about. `reasoning_content` is
 * a Qwen / DeepSeek-R1 extension; `reasoning` is the OpenAI o-series
 * naming. We surface both as `reasoning` to the caller, who routes
 * it to a collapsible chat pane block instead of forwarding to the
 * orchestrator.
 */
interface SseDelta {
  content?: string;
  reasoning?: string;
  tool_calls?: SseToolCallFragment[];
}

interface SseToolCallFragment {
  index?: number;
  function?: { name?: string; arguments?: string };
}

function readDelta(parsed: unknown): SseDelta | undefined {
  if (!parsed || typeof parsed !== "object") {
    return undefined;
  }
  const choices = (parsed as { choices?: unknown }).choices;
  if (!Array.isArray(choices) || choices.length === 0) {
    return undefined;
  }
  const first = choices[0] as { delta?: unknown };
  const delta = first?.delta;
  if (!delta || typeof delta !== "object") {
    return undefined;
  }
  const out: SseDelta = {};
  const d = delta as Record<string, unknown>;
  if (typeof d.content === "string") {
    out.content = d.content;
  }
  if (typeof d.reasoning_content === "string") {
    out.reasoning = d.reasoning_content;
  } else if (typeof d.reasoning === "string") {
    out.reasoning = d.reasoning;
  }
  if (Array.isArray(d.tool_calls)) {
    out.tool_calls = d.tool_calls as SseToolCallFragment[];
  }
  return out;
}

function absorbToolCallDeltas(
  accumulator: Map<number, { name: string; args: string }>,
  fragments: SseToolCallFragment[],
): void {
  for (const f of fragments) {
    const idx = typeof f.index === "number" ? f.index : 0;
    const existing = accumulator.get(idx) ?? { name: "", args: "" };
    if (typeof f.function?.name === "string" && f.function.name.length > 0) {
      existing.name = f.function.name;
    }
    if (typeof f.function?.arguments === "string") {
      existing.args += f.function.arguments;
    }
    accumulator.set(idx, existing);
  }
}

function synthesizeAccumulatedTools(
  accumulator: Map<number, { name: string; args: string }>,
): string[] {
  const out: string[] = [];
  // Iterate in index order so the orchestrator sees tool calls in
  // the same order the model emitted them.
  const indices = [...accumulator.keys()].sort((a, b) => a - b);
  for (const idx of indices) {
    const tc = accumulator.get(idx);
    if (!tc || tc.name.length === 0) {
      continue;
    }
    // Drop tool_calls the model committed to but never filled in.
    // Qwen3-Coder occasionally emits a tool_call event with name set
    // but `function.arguments` empty -- if we synthesized `{}` and
    // forwarded that, the orchestrator would invoke a tool with no
    // args, fail with "missing arg", and we'd waste a whole turn on
    // model noise. Better to silently drop and let the model try
    // again next turn (or move on to producing an artifact). We log
    // the drop on stderr so the rate is visible without polluting
    // the chat.
    if (tc.args.trim().length === 0) {
      console.warn(
        `openai-compat: dropping empty tool_call for ${tc.name} (model emitted name without arguments)`,
      );
      continue;
    }
    out.push(synthesizeToolBlock(tc.name, tc.args));
  }
  return out;
}

function synthesizeToolBlock(name: string, argsRaw: string): string {
  const trimmed = argsRaw.trim();
  let body: string;
  try {
    JSON.parse(trimmed);
    body = trimmed;
  } catch {
    body = JSON.stringify({ raw: trimmed });
  }
  return `\n\n\`\`\`tool:${name}\n${body}\n\`\`\`\n`;
}
