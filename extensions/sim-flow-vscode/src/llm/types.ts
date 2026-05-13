// Language-model abstraction. Backends convert the vendor-neutral
// LlmMessage array into their provider's request shape, stream the
// response, and signal completion. Phase 9 M5 made this module's
// only consumer the SessionPump in `src/session/pump.ts`; the chat
// participant no longer assembles messages itself.

/** Vendor-neutral LLM message. Each backend converts to its own shape. */
export interface LlmMessage {
  role: "system" | "user" | "assistant" | "tool";
  content: string;
  /**
   * Optional binary attachments (e.g. image bytes from a `read_file`
   * call against an image file). Backends that support multimodal
   * input (vscode.lm via Copilot, Anthropic, OpenAI vision, etc.)
   * should convert these into the appropriate provider-specific
   * inline-data form. Backends that don't support images drop them.
   */
  attachments?: LlmAttachment[];
  /**
   * On role=tool messages: the call id this message is replying to.
   * Pairs with the assistant turn's `tool_calls[i].id` so OpenAI-
   * compatible backends can route the result back through the
   * function-calling pipeline.
   */
  tool_call_id?: string;
  /**
   * On role=assistant messages: the tool calls this turn emitted.
   * Backends echo them back on subsequent requests so the model
   * sees its prior calls in history.
   */
  tool_calls?: Array<{
    id?: string;
    name: string;
    arguments_json: string;
  }>;
}

export interface LlmAttachment {
  /** MIME type, e.g. "image/jpeg" or "image/png". */
  mime: string;
  /** Base64-encoded payload. The orchestrator already encodes. */
  data: string;
  /** Optional source path for tracing in logs. */
  source?: string;
}

/**
 * Source selector mirrors the `sim-flow.llm.source` setting. Must
 * stay in sync with `LlmSourceTag` in `webview/messages.ts` (the
 * two were intentionally split when the webview module became
 * orchestrator-facing, but both list the same values).
 *
 * The `*-cli` variants are terminal-only -- they're surfaced here
 * so the chat-pane factory can throw a clear "use the dashboard
 * Run/Resume button" error if the picker is on a CLI agent when a
 * chat-pane dispatch fires.
 */
export type LlmSource =
  | "vscode"
  | "anthropic"
  | "openai"
  | "ollama"
  | "lmstudio"
  | "vllm"
  | "openai-compat"
  | "claude-cli"
  | "codex-cli"
  | "gh-copilot-cli";

export interface LlmBackendOptions {
  /** Model identifier (vendor-specific). Empty means use the backend default. */
  model?: string;
  /** Where the backend reads API keys when it needs them. */
  secrets?: SecretStorage;
  /** Sim-foundation project dir; CLI fallback needs it. */
  projectDir?: string;
  /** Sim-flow CLI binary path; CLI fallback needs it. */
  binary?: string;
}

/** Subset of vscode.SecretStorage we actually use. Lets tests mock it. */
export interface SecretStorage {
  get(key: string): PromiseLike<string | undefined>;
}

/**
 * Normalized chunk kinds the session-driving layer understands.
 *
 * The normalized vocabulary intentionally includes `tool_call` even
 * though current backends still synthesize fenced `tool:<name>` text
 * instead of emitting first-class tool-call chunks. Keeping the
 * future-facing kind in the shared type set now gives both the TS and
 * Rust paths one stable target as the structured normalizer work
 * lands in later milestones.
 */
export type LlmChunkKind = "content" | "reasoning" | "tool_call";

/**
 * Normalized response chunk after defaulting / adaptation cleanup.
 * `text` remains the shared payload for all current kinds so existing
 * session drivers do not need a structural migration before runtime
 * and model-family profiles land.
 */
export interface NormalizedLlmChunk {
  text: string;
  kind: LlmChunkKind;
}

/** Runtime-owned prompt shape after request preparation. */
export interface RuntimePreparedInput {
  /**
   * Dedicated system prompt field for runtimes like Anthropic's
   * Messages API. Omitted when the runtime keeps system prompts in
   * the regular message array.
   */
  system?: string;
  /** Provider-neutral messages after runtime-level reshaping. */
  messages: LlmMessage[];
}

export interface LlmStreamChunk {
  /** Incremental text to append to the response. */
  text: string;
  /**
   * What flavor of chunk this is. `content` is normal assistant text
   * the orchestrator sees. `reasoning` is chain-of-thought text some
   * models (Qwen3-Coder, DeepSeek-R1, o-series) emit alongside their
   * actual answer; the host renders it collapsibly in the chat pane
   * but does NOT forward it to the orchestrator (it would otherwise
   * pollute artifact / tool-call extraction). Defaults to `content`
   * when omitted, so existing call sites stay unchanged.
   */
  kind?: LlmChunkKind;
  /**
   * Structured native tool calls the backend emitted on this chunk.
   * Optional and additive: the backend may also synthesize fenced
   * `tool:<name>` blocks in `text` for transcript visibility and
   * the orchestrator's fenced-mode fallback parser; when this field
   * is present the pump collects each entry and ships them in
   * `LlmEnd.tool_calls` so the orchestrator's native-mode path can
   * dispatch the calls directly without re-parsing fences. Empty /
   * undefined means "no native tool calls on this chunk."
   */
  toolCalls?: StreamToolCall[];
  /**
   * Exact token counts reported by the model server. Populated by
   * backends whose SSE stream surfaces a `usage` payload (typically
   * the final SSE chunk when the request was sent with
   * `stream_options.include_usage: true` on OpenAI-compat servers).
   * The pump aggregates the last non-null `usage` from the stream
   * and ships it on `LlmEnd.usage`; the orchestrator overrides
   * its byte-based token estimate in `llm-metrics.jsonl` with
   * these values and flips `tokens_exact: true` on the row.
   */
  usage?: {
    promptTokens: number;
    completionTokens: number;
  };
}

export interface StreamToolCall {
  /**
   * Backend-supplied call id (e.g. OpenAI's `tool_calls[i].id`).
   * Optional; pumps synthesize a deterministic placeholder when the
   * backend doesn't carry one. Threaded through to
   * `LlmToolCall.id` on the wire so the next request's tool-result
   * message can pair its `tool_call_id` correctly.
   */
  id?: string;
  /** Tool name as advertised in the request's tool catalog. */
  name: string;
  /**
   * Raw JSON-encoded argument blob the model emitted. Pumps pass
   * this through verbatim as `LlmToolCall.arguments_json`; the
   * orchestrator parses it via `serde_json::Value` at dispatch.
   */
  argumentsJson: string;
}

/**
 * Normalize a streamed chunk into the explicit session-facing shape.
 * This keeps the legacy "kind omitted means content" behavior local
 * to the LLM layer rather than spreading that default across every
 * consumer.
 */
export function normalizeLlmChunk(chunk: LlmStreamChunk): NormalizedLlmChunk {
  return {
    text: chunk.text,
    kind: chunk.kind ?? "content",
  };
}

/**
 * Runtime capability profile: behavior imposed by the serving stack
 * or integration surface rather than by the model family itself.
 * Kept intentionally small for the first landing; concrete profiles
 * can extend this as Phase 10 proceeds.
 */
export interface RuntimeCapabilityProfile {
  id: string;
  /** Request family the transport/runtime expects on the wire. */
  requestFormat?:
    | "openai_chat_completions"
    | "anthropic_messages"
    | "processor_local"
    | "vscode_language_model";
  /** Where credential lookup policy is owned for this runtime. */
  credentialPolicy?: "shared-provider-chain" | "host-managed" | "none";
  /** How the runtime expects system prompts to be carried. */
  systemPromptMode?: "message-array" | "collapsed-leading-message" | "dedicated-field";
  /**
   * True when the runtime requires or prefers multiple leading
   * system messages to be collapsed before serialization.
   */
  collapseLeadingSystemMessages?: boolean;
  /** True when the runtime can expose reasoning separately from text. */
  supportsStructuredReasoning?: boolean;
  /** True when the runtime can expose native structured tool calls. */
  supportsStructuredToolCalls?: boolean;
  /** True when the runtime supports a shared cross-host credential chain. */
  supportsSharedCredentialChain?: boolean;
  /**
   * Runtime-level prompt shaping that happens before model-family
   * policy and transport serialization. This is where serving-stack
   * quirks like "collapse leading system messages" or "split system
   * into a dedicated request field" belong.
   */
  prepareInput?(messages: LlmMessage[]): RuntimePreparedInput;
}

/**
 * Model-family profile: prompt / sampling / output semantics that
 * stay stable across multiple runtimes serving the same family.
 */
export interface ModelFamilyProfile {
  id: string;
  /** Hint that the family may emit raw-text thought markers. */
  thoughtMarkerStyle?:
    | "none"
    | "qwen-think-tag"
    | "kimi-think-tag"
    | "gemma-think-tag"
    | "anthropic-thinking-blocks"
    | "custom";
  /** Family-level preference for whether multimodal inputs place media first. */
  prefersMediaBeforeText?: boolean;
  /** True when the family has first-class reasoning controls. */
  supportsThinkingControls?: boolean;
  /** How the family exposes thinking controls when they exist. */
  thinkingControlMode?: "none" | "prompt-tag" | "runtime-flag";
  /** Prompt token/tag used by prompt-controlled families like Gemma 4. */
  thinkingControlToken?: string;
  /** Guidance for whether prior reasoning should stay in history. */
  reasoningHistoryPolicy?: "preserve-all" | "drop-prior-reasoning" | "runtime-controlled";
  /** Family-level generation defaults. */
  defaultSampling?: {
    temperature?: number;
    topP?: number;
    topK?: number;
  };
}

/**
 * Response normalizer: converts provider/runtime/model-family output
 * into the compact internal chunk stream the session-driving layer
 * consumes.
 */
export interface ResponseNormalizer {
  id: string;
  normalizeChunk(chunk: LlmStreamChunk): NormalizedLlmChunk[];
  flush?(): NormalizedLlmChunk[];
}

/** Optional adaptation metadata a backend may expose for diagnostics. */
export interface LlmAdaptationProfile {
  runtime: RuntimeCapabilityProfile;
  modelFamily: ModelFamilyProfile;
  responseNormalizer: ResponseNormalizer;
}

/** Compact, user-facing summary of the active adaptation path. */
export interface LlmAdaptationSummary {
  backend: string;
  runtimeId: string;
  modelFamilyId: string;
  requestFormat?: RuntimeCapabilityProfile["requestFormat"];
  systemPromptMode?: RuntimeCapabilityProfile["systemPromptMode"];
  credentialPolicy?: RuntimeCapabilityProfile["credentialPolicy"];
  supportsStructuredReasoning: boolean;
  supportsStructuredToolCalls: boolean;
  supportsThinkingControls: boolean;
}

/**
 * Vendor-neutral tool descriptor mirroring `protocol-types.LlmTool`.
 * Backends that support native tool-use (OpenAI / LM Studio function
 * calling, Anthropic tool_use blocks) translate this into their
 * provider's shape and synthesize sim-flow's fenced `tool:<name>`
 * blocks back into the streamed text on the response side. Backends
 * that don't support native tool-use ignore this field; the agent
 * falls back to emitting fenced blocks itself, parsed by the
 * orchestrator's `extract_tool_calls`.
 */
export interface LlmTool {
  name: string;
  description: string;
  /** JSON Schema for the tool's argument object. */
  args_schema: Record<string, unknown>;
}

/** Abstract cancellation token compatible with vscode.CancellationToken. */
export interface CancellationLike {
  isCancellationRequested: boolean;
  onCancellationRequested?(listener: () => void): { dispose(): void } | void;
}

export interface LlmBackend {
  /** Human-readable backend name (for status lines in the chat). */
  readonly name: string;
  /**
   * Optional adaptation metadata for diagnostics and future profile-
   * driven request shaping. Backends may omit this until they are
   * migrated onto explicit Phase 10 profiles.
   */
  readonly adaptation?: LlmAdaptationProfile;
  /**
   * Stream a response for the supplied messages. Chunks are yielded
   * as soon as they arrive. The consumer is responsible for stopping
   * iteration when the token is cancelled; well-behaved backends also
   * honor cancellation internally.
   *
   * `tools` is the orchestrator's tool catalog. Backends that support
   * native tool-use translate it into the provider's request shape;
   * backends that don't may safely ignore it.
   */
  stream(
    messages: LlmMessage[],
    token: CancellationLike,
    tools?: LlmTool[],
  ): AsyncIterable<LlmStreamChunk>;
}

export class LlmError extends Error {
  readonly kind: LlmErrorKind;
  readonly detail?: string;

  constructor(kind: LlmErrorKind, message: string, detail?: string, cause?: unknown) {
    super(message, { cause });
    this.name = "LlmError";
    this.kind = kind;
    this.detail = detail;
  }
}

export type LlmErrorKind =
  | "no-model"
  | "missing-api-key"
  | "http"
  | "parse"
  | "unsupported"
  | "cancelled";
