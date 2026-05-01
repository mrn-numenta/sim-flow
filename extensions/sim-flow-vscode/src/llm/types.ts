// Language-model abstraction. Backends convert the vendor-neutral
// LlmMessage array into their provider's request shape, stream the
// response, and signal completion. Phase 9 M5 made this module's
// only consumer the SessionPump in `src/session/pump.ts`; the chat
// participant no longer assembles messages itself.

/** Vendor-neutral LLM message. Each backend converts to its own shape. */
export interface LlmMessage {
  role: "system" | "user" | "assistant";
  content: string;
  /**
   * Optional binary attachments (e.g. image bytes from a `read_file`
   * call against an image file). Backends that support multimodal
   * input (vscode.lm via Copilot, Anthropic, OpenAI vision, etc.)
   * should convert these into the appropriate provider-specific
   * inline-data form. Backends that don't support images drop them.
   */
  attachments?: LlmAttachment[];
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
  kind?: "content" | "reasoning";
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
