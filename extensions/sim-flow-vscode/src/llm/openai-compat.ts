// Shared implementation for OpenAI Chat Completions-compatible
// backends (OpenAI, Ollama, LM Studio). Subclasses fix the provider
// name, default base URL, default model, and key policy; the wire
// format is identical.

import { envVarFor, type ProviderId, resolveApiKey as resolveProviderKey } from "./keyResolver";
import {
  extractToolFences,
  makeToolCallId,
  normalizeToolArgsForOpenAi,
  parseToolResultsEnvelope,
  type ParsedToolCall,
} from "./tool-translation";
import {
  mergeLeadingSystemMessages,
  OPENAI_COMPAT_GENERIC_RUNTIME,
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

export interface OpenAiCompatibleBackendOptions {
  /** Human-readable backend name (shown in the chat header). */
  name: string;
  /** Base URL, e.g. "https://api.openai.com/v1". No trailing slash. */
  baseUrl: string;
  /** Default model id when the caller doesn't supply one. */
  defaultModel: string;
  /**
   * Provider id (`openai` / `ollama` / `lmstudio`). Drives the
   * env-var name and config-file table the resolver consults.
   * When set, takes precedence over the legacy `keyId` path so the
   * backend benefits from the shared CLI / extension key chain.
   */
  provider?: ProviderId;
  /**
   * Legacy SecretStorage key id. Kept for back-compat with callers
   * that pre-date `provider`; new subclasses should set both.
   */
  keyId?: string;
  /** When true, the backend throws `missing-api-key` if the key is absent. */
  requireKey: boolean;
  /** Override for the request path; defaults to "/chat/completions". */
  path?: string;

  /** Caller-supplied overrides. */
  model?: string;
  /** Explicit model-family override; otherwise inferred from `model`. */
  modelFamilyId?: string;
  /** Explicit runtime-profile override; otherwise uses the backend default. */
  runtimeProfileId?: string;
  secrets?: SecretStorage;
  /** Absolute override URL. Takes precedence over baseUrl + path. */
  apiUrl?: string;
  fetchImpl?: typeof fetch;
  /** Abort stalled SSE streams after this many milliseconds of silence. */
  streamIdleTimeoutMs?: number;
}

export class OpenAiCompatibleBackend implements LlmBackend {
  readonly name: string;
  readonly adaptation: LlmAdaptationProfile;
  private static readonly DEFAULT_STREAM_IDLE_TIMEOUT_MS = 30_000;

  constructor(protected readonly options: OpenAiCompatibleBackendOptions) {
    this.name = options.name;
    const modelFamily = resolveModelFamily(this.options.modelFamilyId, this.options.model);
    let runtime = OPENAI_COMPAT_GENERIC_RUNTIME;
    try {
      runtime = resolveRuntimeProfile(
        this.options.runtimeProfileId,
        OPENAI_COMPAT_GENERIC_RUNTIME,
        [OPENAI_COMPAT_GENERIC_RUNTIME.id],
      );
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
    //
    // Translate sim-flow's fenced-block tool representation into
    // OpenAI's native `tool_calls` + `role: tool` shape. Without
    // this, the model sees its own previous tool calls as text in
    // the conversation history and tends to emit text-mode tool
    // calls in subsequent turns (or worse, near-misses on the fence
    // syntax — qwen3-coder slips to `tool=name` instead of
    // `tool:name`). Going native end-to-end keeps the model in its
    // own protocol.
    // Strict OpenAI-compat servers (vllm with the default chat
    // template, especially) reject requests with multiple system
    // messages: "System message must be at the beginning." The
    // orchestrator legitimately emits several (combined-system,
    // tool-notice, spec TOC, framework TOC, session-inputs
    // stable + volatile) so the prefix-cache split survives.
    // Collapse them into one before serializing -- LM Studio /
    // Ollama / OpenAI tolerate the merged form just fine, so this
    // is uniform behavior, not a vllm-special-case.
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
      messages: OpenAiMessage[];
      tools?: OpenAiTool[];
      stream: boolean;
    } = {
      model,
      messages: transformMessagesForOpenAi(familyInput.messages, modelFamily),
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
    const cancelSubscription = token.onCancellationRequested?.(() => {
      controller.abort();
    });
    let streamTimedOut = false;
    let idleTimer: ReturnType<typeof setTimeout> | undefined;
    const clearIdleTimer = () => {
      if (idleTimer !== undefined) {
        clearTimeout(idleTimer);
        idleTimer = undefined;
      }
    };
    const resetIdleTimer = () => {
      clearIdleTimer();
      const timeoutMs =
        this.options.streamIdleTimeoutMs ?? OpenAiCompatibleBackend.DEFAULT_STREAM_IDLE_TIMEOUT_MS;
      idleTimer = setTimeout(() => {
        streamTimedOut = true;
        controller.abort();
      }, timeoutMs);
    };
    let res: Response;
    try {
      res = await doFetch(url, {
        method: "POST",
        headers,
        body: JSON.stringify(body),
        signal: controller.signal,
      });
    } catch (err) {
      cancelSubscription?.dispose();
      if (token.isCancellationRequested || isAbortError(err)) {
        throw new LlmError("cancelled", `${this.options.name} request cancelled.`);
      }
      // Include the URL we tried so the user can immediately see
      // host / port / path when the connection itself fails. The
      // bare "fetch failed" Node hands us is otherwise useless for
      // diagnosing wrong-port / unreachable-host setups.
      throw new LlmError(
        "http",
        `${this.options.name} request to ${url} failed: ${(err as Error).message ?? String(err)}`,
        undefined,
        err,
      );
    }
    if (!res.ok) {
      cancelSubscription?.dispose();
      const detail = await safeText(res);
      throw new LlmError(
        "http",
        `${this.options.name} API at ${url} returned ${res.status} ${res.statusText}`,
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
      resetIdleTimer();
      for await (const chunk of res.body as unknown as AsyncIterable<Uint8Array>) {
        resetIdleTimer();
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
          if (delta?.finishReason && delta.finishReason !== "length") {
            for (const block of synthesizeAccumulatedTools(accumulator)) {
              yield { text: block };
            }
            return;
          }
          if (delta?.finishReason === "length") {
            throw new LlmError(
              "parse",
              `${this.options.name} response was truncated (finish_reason=length).`,
            );
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
      if (streamTimedOut) {
        const timeoutMs =
          this.options.streamIdleTimeoutMs ?? OpenAiCompatibleBackend.DEFAULT_STREAM_IDLE_TIMEOUT_MS;
        throw new LlmError(
          "http",
          `${this.options.name} stream timed out after ${timeoutMs} ms without any response activity.`,
        );
      }
      if (token.isCancellationRequested || isAbortError(err)) {
        throw new LlmError("cancelled", `${this.options.name} stream cancelled.`);
      }
      throw new LlmError(
        "http",
        `${this.options.name} stream error: ${(err as Error).message ?? String(err)}`,
        undefined,
        err,
      );
    } finally {
      clearIdleTimer();
      cancelSubscription?.dispose();
    }
  }

  protected async resolveApiKey(): Promise<string | undefined> {
    // Resolution chain (shared with the Rust CLI for env + config
    // file): env var → `<config>/sim-flow/credentials.toml` → VS
    // Code SecretStorage. The provider id drives the env var name
    // and config-file table. The legacy `keyId` is retained as the
    // SecretStorage id so already-stored keys keep working.
    if (this.options.provider) {
      const resolved = await resolveProviderKey(this.options.provider, this.options.secrets);
      if (resolved) {
        return resolved.key;
      }
      if (this.options.requireKey) {
        throw new LlmError(
          "missing-api-key",
          `${this.options.name} API key not found. Set ${envVarFor(this.options.provider)} in your shell, run \`sim-flow keys set ${this.options.provider}\`, or use the "sim-flow: Set LLM API Key" command.`,
        );
      }
      return undefined;
    }
    // Fallback for backends without a provider id (legacy callers
    // and tests) — keep the original SecretStorage-only path.
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

function isAbortError(err: unknown): boolean {
  return !!err && typeof err === "object" && (err as { name?: unknown }).name === "AbortError";
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

interface OpenAiToolCall {
  id: string;
  type: "function";
  function: { name: string; arguments: string };
}

/**
 * OpenAI Chat Completions message shape. Three variants:
 *
 * - `system` / `user`: plain `content` (string or multimodal array).
 * - `assistant`: optional text `content` plus optional `tool_calls`.
 *   At least one of the two must be non-empty; a tool-call-only
 *   assistant turn uses `content: null` per OpenAI's spec.
 * - `tool`: result of one prior `assistant.tool_calls[i]`. The
 *   `tool_call_id` MUST match the assistant message's id.
 */
type OpenAiMessage =
  | { role: "system" | "user"; content: OpenAiContent }
  | {
      role: "assistant";
      content: OpenAiContent | null;
      tool_calls?: OpenAiToolCall[];
    }
  | { role: "tool"; tool_call_id: string; content: string };

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

/**
 * Walk sim-flow's `LlmMessage[]` and produce OpenAI-native messages.
 *
 * The orchestrator's wire shape carries tool calls as fenced text in
 * assistant content and tool results as a "Tool results: …" user
 * message. This walks the conversation in order:
 *
 *   - For each assistant message, extract every `\`\`\`tool:<name>` fence
 *     into a structured `tool_calls` entry, mint deterministic ids,
 *     and keep any non-fence text as `content`. A tool-call-only
 *     assistant turn becomes `{ content: null, tool_calls: [...] }`.
 *   - When the next message is a "Tool results: …" user message,
 *     split it on the orchestrator's `\n\n---\n\n` separator and emit
 *     one `role: "tool"` message per section, paired by index with
 *     the previous assistant's tool_call ids.
 *   - Anything that doesn't match those patterns passes through
 *     unchanged.
 *
 * Outcome: the model sees a clean
 * `assistant{tool_calls} → tool → assistant{tool_calls} → …`
 * conversation, so it stays in native-tool-calling mode instead of
 * mimicking the fenced-text format we synthesized for it.
 */
/**
 * Collapse a run of leading `role: "system"` messages into one,
 * joining their bodies with a blank-line separator. Subsequent
 * non-leading system messages (rare; defensive) are left in place
 * so we don't mask higher-up bugs by silently rewriting them.
 *
 * Why we need this: the orchestrator emits up to five separate
 * system messages at the head of every prompt (combined-system,
 * tool-notice, spec TOC, framework-API TOC, session-inputs
 * stable + volatile) so vLLM's prefix cache can reuse the long
 * stable prefix across milestone advances and critique retries.
 * That split works on most servers, but the default vllm chat
 * template enforces "exactly one system message at the
 * beginning" and rejects the request with `BadRequestError:
 * System message must be at the beginning.`. Merging on the
 * wire keeps the prefix-cache benefit intact (the merged head
 * is still token-identical across dispatches that share their
 * stable inputs) while satisfying the strict template.
 */
export function transformMessagesForOpenAi(
  messages: LlmMessage[],
  modelFamily = GENERIC_CHAT_MODEL_FAMILY,
): OpenAiMessage[] {
  const out: OpenAiMessage[] = [];
  // Outstanding tool_call ids from the most recent assistant message.
  // The next user message is a candidate for translation into
  // `role: tool` results paired with these ids; cleared once paired
  // (or once a non-tool-results user message lands, since OpenAI
  // doesn't allow lingering tool calls without matching results).
  let pendingToolCallIds: string[] = [];

  for (let i = 0; i < messages.length; i++) {
    const m = messages[i];

    if (m.role === "assistant") {
      // Tool fences only live in plain text content; if the message
      // carries attachments we don't try to extract — fall back to
      // pass-through. (Today the orchestrator never attaches images
      // to assistant messages; the guard is defensive.)
      if (m.attachments && m.attachments.length > 0) {
        out.push({ role: "assistant", content: openAiContent(m, modelFamily) });
        pendingToolCallIds = [];
        continue;
      }
      const { content, toolCalls } = extractToolFences(m.content);
      if (toolCalls.length === 0) {
        out.push({ role: "assistant", content: m.content });
        pendingToolCallIds = [];
        continue;
      }
      const assistant: OpenAiToolCall[] = toolCalls.map((tc: ParsedToolCall, idx: number) => ({
        id: makeToolCallId(i, idx),
        type: "function" as const,
        function: {
          name: tc.name,
          arguments: normalizeToolArgsForOpenAi(tc.args),
        },
      }));
      out.push({
        role: "assistant",
        content: content.length > 0 ? content : null,
        tool_calls: assistant,
      });
      pendingToolCallIds = assistant.map((c) => c.id);
      continue;
    }

    if (m.role === "user" && pendingToolCallIds.length > 0) {
      // Attachments on a tool-results message would lose alignment
      // when split into per-section role: tool entries. Fall back to
      // pass-through if any section would need to carry an image —
      // the model still sees the data, just as a regular user
      // message.
      const sections =
        m.attachments && m.attachments.length > 0 ? null : parseToolResultsEnvelope(m.content);
      if (sections === null) {
        // Plain user message — clear pending ids so we don't
        // mis-pair later, and pass through.
        pendingToolCallIds = [];
        out.push({ role: "user", content: openAiContent(m, modelFamily) });
        continue;
      }
      // Pair each section with the corresponding pending id.
      const pairCount = Math.min(sections.length, pendingToolCallIds.length);
      for (let j = 0; j < pairCount; j++) {
        out.push({
          role: "tool",
          tool_call_id: pendingToolCallIds[j],
          content: sections[j],
        });
      }
      // If there are extra sections (more results than calls), the
      // surplus has nowhere to live as `role: tool` messages. Tack
      // them on as a regular user message so the model still sees
      // the data.
      if (sections.length > pairCount) {
        out.push({
          role: "user",
          content: sections.slice(pairCount).join("\n\n---\n\n"),
        });
      }
      // Note: if there are MORE pending ids than sections, OpenAI
      // will reject the request ("tool call without matching
      // result"). We don't fabricate results — this signals a real
      // protocol violation upstream that should surface as an HTTP
      // error rather than be silently masked.
      pendingToolCallIds = [];
      continue;
    }

    // Plain message (system / user without pending tool calls). Pass
    // through. Clear pending ids — any tool calls without matching
    // results before the next user turn would be a protocol error.
    pendingToolCallIds = [];
    if (m.role === "system" || m.role === "user") {
      out.push({ role: m.role, content: openAiContent(m, modelFamily) });
    } else {
      // Unknown role — shouldn't happen; pass through with role
      // coerced to user so the message at least reaches the model.
      out.push({ role: "user", content: openAiContent(m, modelFamily) });
    }
  }
  return out;
}

export { mergeLeadingSystemMessages };

function trimTrailingSlash(s: string): string {
  return s.endsWith("/") ? s.slice(0, -1) : s;
}

type OpenAiContent =
  | string
  | Array<{ type: "text"; text: string } | { type: "image_url"; image_url: { url: string } }>;

/**
 * OpenAI-compatible APIs (OpenAI, Ollama, LM Studio) accept either a
 * plain string or a content array of typed parts. Images go through
 * as `image_url` blocks with a base64 data URI; that shape is also
 * what GPT-4o vision and Llava expect when using the openai-compat
 * surface.
 */
function openAiContent(m: LlmMessage, modelFamily = GENERIC_CHAT_MODEL_FAMILY): OpenAiContent {
  if (!m.attachments || m.attachments.length === 0) {
    return m.content;
  }
  const parts: Exclude<OpenAiContent, string> = [];
  for (const part of orderAttachmentsByFamily(modelFamily, m.content, m.attachments)) {
    if (part.kind === "text") {
      parts.push({ type: "text", text: part.text });
      continue;
    }
    parts.push({
      type: "image_url",
      image_url: { url: `data:${part.attachment.mime};base64,${part.attachment.data}` },
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
  finishReason?: string;
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
  const first = choices[0] as { delta?: unknown; finish_reason?: unknown };
  const delta = first?.delta;
  const out: SseDelta = {};
  if (typeof first.finish_reason === "string" && first.finish_reason.length > 0) {
    out.finishReason = first.finish_reason;
  }
  if (delta && typeof delta === "object") {
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
  }
  return Object.keys(out).length > 0 ? out : undefined;
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
