// Long-lived wrapper around `sim-flow session <step>.<kind> --jsonl`.
//
// VS Code's chat participant model invokes the handler once per user
// turn, but the orchestrator subprocess needs to live across turns
// to preserve session state (history, phase, file watchers, etc.).
// `SessionPump` is the bridge: one pump per active chat session,
// kept alive by the SessionRegistry until the session ends or the
// chat tab is closed.
//
// Per turn, the chat handler:
//   1. Looks up (or creates) the pump for this session.
//   2. Awaits the pump's settle promise so we know the orchestrator
//      is parked at a `RequestUserInput` (or has already exited).
//   3. Sends the user's reply via `sendUserMessage`.
//   4. Awaits settle again, draining all events emitted in the
//      meantime to the chat stream.
//
// LLM dispatch: when the orchestrator emits `RequestLlmResponse`,
// the pump invokes the configured `LlmBackend` and streams chunks
// back via stdin. Streaming runs concurrently with chat-event
// rendering since the orchestrator may emit further events while the
// LLM is still producing.

import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { randomUUID } from "node:crypto";
import { EventEmitter, once } from "node:events";

import * as vscode from "vscode";

import { bundledFrameworkDocsRoot, bundledPdfiumLibPath } from "../cli";
import {
  type LlmBackend,
  type LlmMessage as BackendLlmMessage,
  type SecretStorage,
  createBackend,
  LlmError,
  type LlmSource,
} from "../llm";
import { estimateMessagesTokens } from "../llm/tokenEstimate";
import type {
  Event as ProtocolEvent,
  HostEvent,
  HostInfo,
  LlmMessage as ProtocolLlmMessage,
  SessionKindOut,
  SessionTag,
  StepDescriptorOut,
  StepMode,
} from "./protocol-types";
import { DebugLog } from "./debug-log";
import { removePidRecord, writePidRecord } from "./processRegistry";
import { type LlmServerEntry, resolveLlmSource } from "../webview/messages";

/** Configuration the registry hands to a freshly-spawned pump. */
export interface SessionPumpOptions {
  binary: string;
  args: string[];
  cwd: string;
  /**
   * Inherited environment for the subprocess. Currently unused; the
   * orchestrator inherits the parent process's env so things like
   * `ANTHROPIC_API_KEY` flow through if the orchestrator ever needs
   * them itself.
   */
  env?: NodeJS.ProcessEnv;
}

/** Resolved settings the pump uses to dispatch RequestLlmResponse. */
export interface PumpLlmConfig {
  source: LlmSource;
  model?: string;
  /**
   * Generic base-URL override for OpenAI-compat backends. Set when
   * the user picks a custom server (`server:<name>` in the source
   * picker). Wins over `ollamaBaseUrl` / `lmstudioBaseUrl` and
   * gets passed through as `--llm-base-url` to the spawned
   * `sim-flow auto`.
   */
  baseUrl?: string;
  ollamaBaseUrl?: string;
  lmstudioBaseUrl?: string;
  secrets?: SecretStorage;
  /** Project dir + binary used by the CLI fallback backend (M9 dropped that backend; here for completeness). */
  projectDir: string;
  binary: string;
  /**
   * Comma-joined debug categories from `sim-flow.debug` (or the
   * `SIM_FOUNDATION_DEBUG` env var when the setting is empty). Empty
   * string disables logging on both sides. Forwarded to the
   * subprocess so the orchestrator's DebugLog sees the same value.
   */
  debugTokens: string;
}

/** Snapshot of state every chat turn finishes with. */
export interface PumpSettleResult {
  status: "awaiting-input" | "ended";
  endReason?: string;
  endMessage?: string;
}

/**
 * Renderer the pump pushes events into. The chat-handler turn
 * supplies a `vscode.ChatResponseStream`; tests use a recording
 * stub.
 */
export interface PumpRenderer {
  markdown(text: string): void;
  reference?(uri: vscode.Uri, label?: string): void;
  requestTokensEstimate?(tokens: number): void;
}

export interface LiveSessionTransport {
  readonly session: SessionTag | null;
  readonly descriptor: StepDescriptorOut | null;
  settle(renderer: PumpRenderer): Promise<PumpSettleResult>;
  sendUserMessage(text: string): void;
  cancel(): void;
  dispose(): void;
  /**
   * Manual step-mode capabilities. Only the JSONL transport
   * (`SocketSessionPump`) implements these today; the PTY transport
   * uses a different control socket and the per-step protocol there
   * is a follow-up. Call sites use optional chaining and fall back to
   * the legacy chat-tab path when these are absent.
   */
  readonly stepMode?: StepMode | null;
  onStepModeChanged?(listener: (mode: StepMode) => void): () => void;
  setStepMode?(mode: StepMode): void;
  runStep?(step: string, kind: SessionKindOut): void;
  runCritique?(step: string): void;
  runGate?(step: string): void;
  advance?(step: string): void;
  reset?(step: string): void;
  shutdown?(): void;
  /**
   * Graceful-then-forceful disconnect. Sends `shutdown`, waits for
   * the orchestrator child to exit cleanly, escalates to SIGTERM,
   * then SIGKILL. Returns the path that ended the child. Only the
   * JSONL transport implements this; PTY / mock transports omit it
   * and callers fall back to the control-socket `/exit` injection.
   */
  disconnectWithEscalation?(
    cleanTimeoutMs?: number,
    termTimeoutMs?: number,
  ): Promise<"clean" | "sigterm" | "sigkill" | "already-gone">;
  /**
   * Sub-session bracketing surface. `inSubSession` is true while the
   * orchestrator is inside a Work / Critique sub-session and false
   * while parked. `onSubSessionChanged` notifies on every transition
   * so the dashboard can refresh and re-evaluate per-step button
   * gating. Same JSONL-only optionality as the manual-mode commands
   * above.
   */
  readonly inSubSession?: boolean;
  onSubSessionChanged?(listener: (inSubSession: boolean) => void): () => void;
}

/**
 * System message slipped into LLM requests when `sim-flow.llm.verbose`
 * is OFF. Kept short and explicit because long brevity directives
 * paradoxically make models more verbose.
 */
export const BREVITY_DIRECTIVE = [
  "Brevity directive (overrides any earlier prose-style guidance):",
  "- Be concise. State results directly.",
  "- Skip preamble, recaps, and summaries of what you're about to do.",
  "- Don't restate the task; just do it.",
  "- Prefer bullet lists and short sentences over prose.",
  "- Avoid hedging language ('I think...', 'It might be...', 'It's worth noting...').",
  "- Code, file paths, and tool calls over commentary.",
].join("\n");

/** Bind a `vscode.ChatResponseStream` as a `PumpRenderer`. */
export function rendererFromChatStream(stream: vscode.ChatResponseStream): PumpRenderer {
  return {
    markdown(text) {
      stream.markdown(text);
    },
  };
}

// Internal protocol events we emit on the pump bus.
type PumpBusEvent =
  | { type: "settled"; result: PumpSettleResult }
  | { type: "event"; event: ProtocolEvent };

/**
 * Owns one orchestrator subprocess + the JSONL pump that drives it.
 * Construction kicks off the handshake; await `firstSettle()` before
 * the first user reply.
 */
export class SessionPump {
  private process: ChildProcessWithoutNullStreams;
  private bus = new EventEmitter();
  private debugLog: DebugLog;
  private stdoutBuffer = "";
  private stderrBuffer = "";
  private currentRenderer: PumpRenderer | null = null;
  private currentRequestId = 0;
  private terminated = false;
  private terminationReason: PumpSettleResult | null = null;
  private sessionTag: SessionTag | null = null;
  private stepDescriptor: StepDescriptorOut | null = null;
  /**
   * Last LLM source actually used by `dispatchLlm`. Lets us announce
   * a banner when the user toggles `sim-flow.llm.source` mid-run.
   */
  private lastUsedSource: LlmSource | null = null;
  /**
   * Stable id for this pump's pid record. Generated internally
   * because callers don't supply one (unlike `SocketSessionPump`).
   * Used as the basename of `<project>/.sim-flow/pids/<id>.json` so
   * the next extension activate can reap orphans.
   */
  private readonly sessionId: string = randomUUID();
  private pidRecordCleared = false;

  constructor(
    options: SessionPumpOptions,
    private readonly llm: PumpLlmConfig,
  ) {
    this.lastUsedSource = llm.source;
    this.debugLog = DebugLog.fromTokens(llm.debugTokens, llm.projectDir);
    const pdfiumLib = bundledPdfiumLibPath();
    const frameworkDocsRoot = bundledFrameworkDocsRoot();
    const env: NodeJS.ProcessEnv = {
      ...(options.env ?? process.env),
      SIM_FOUNDATION_DEBUG: llm.debugTokens,
    };
    if (pdfiumLib) {
      env.SIM_FLOW_PDFIUM_LIB_PATH = pdfiumLib;
    }
    if (frameworkDocsRoot) {
      env.SIM_FLOW_FRAMEWORK_DOCS_ROOT = frameworkDocsRoot;
    }
    this.process = spawn(options.binary, options.args, {
      cwd: options.cwd,
      env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.debugLog.logProcessSpawn(options.binary, options.args, this.process.pid);
    if (typeof this.process.pid === "number") {
      try {
        writePidRecord(llm.projectDir, {
          pid: this.process.pid,
          sessionId: this.sessionId,
          binary: options.binary,
          label: options.args.slice(0, 2).join(" "),
          spawnedAtMs: Date.now(),
        });
      } catch (err) {
        this.debugLog.logSpawnError(
          `pid registry write failed: ${(err as Error).message}`,
        );
      }
    }
    this.process.stdout.setEncoding("utf8");
    this.process.stderr.setEncoding("utf8");
    this.process.stdout.on("data", (chunk: string) => this.onStdoutChunk(chunk));
    this.process.stderr.on("data", (chunk: string) => {
      this.stderrBuffer += chunk;
    });
    this.process.on("error", (err) => {
      this.debugLog.logSpawnError(err.message);
      this.clearPidRecord();
      this.markTerminated({
        status: "ended",
        endReason: "spawn-error",
        endMessage: `${err.message}`,
      });
    });
    this.process.on("exit", (code, signal) => {
      this.debugLog.logProcessExit(code, signal, this.stderrBuffer);
      this.clearPidRecord();
      this.markTerminated({
        status: "ended",
        endReason: code === 0 ? "completed" : "process-exit",
        endMessage:
          code === 0 && !this.stderrBuffer
            ? undefined
            : `process exited with code ${code}${signal ? ` (signal ${signal})` : ""}; stderr tail: ${this.stderrBuffer.slice(-512)}`,
      });
    });

    // Kick off the handshake immediately. The orchestrator blocks on
    // its first `read()` waiting for our Hello.
    this.sendHostEvent({
      event: "hello",
      protocol_version: "1",
      host: {
        name: "sim-flow-vscode",
        version: "0.2.0",
      } as HostInfo,
      capabilities: ["text", "markdown", "user-input", "llm-request", "tool-notifications"],
    });
  }

  get session(): SessionTag | null {
    return this.sessionTag;
  }

  get descriptor(): StepDescriptorOut | null {
    return this.stepDescriptor;
  }

  /**
   * Wait for the orchestrator to either request user input or end.
   * The optional `renderer` receives every event that arrives during
   * this settle window. Resolves with the terminal status; subsequent
   * settle calls after `ended` resolve immediately with the cached
   * result.
   */
  async settle(renderer: PumpRenderer): Promise<PumpSettleResult> {
    if (this.terminated && this.terminationReason) {
      return this.terminationReason;
    }
    this.currentRenderer = renderer;
    return new Promise<PumpSettleResult>((resolve) => {
      const onSettled = (msg: PumpBusEvent) => {
        if (msg.type !== "settled") {
          return;
        }
        this.bus.off("msg", onSettled);
        this.currentRenderer = null;
        resolve(msg.result);
      };
      this.bus.on("msg", onSettled);
    });
  }

  /** Send a user reply. Caller awaits `settle` afterward. */
  sendUserMessage(text: string): void {
    if (this.terminated) {
      return;
    }
    this.sendHostEvent({ event: "user-message", text });
  }

  /** Tell the orchestrator the user cancelled this session. */
  cancel(): void {
    if (this.terminated) {
      return;
    }
    this.sendHostEvent({ event: "cancel" });
  }

  /** Force-kill the subprocess. Used on extension deactivate. */
  dispose(): void {
    if (!this.terminated) {
      try {
        this.process.kill("SIGTERM");
      } catch {
        // ignore
      }
    }
    this.terminated = true;
    // Defensive cleanup: if the exit handler hasn't fired yet (or
    // never will, e.g. tests), reap the pid record here.
    this.clearPidRecord();
    this.debugLog.dispose();
  }

  private clearPidRecord(): void {
    if (this.pidRecordCleared) {
      return;
    }
    this.pidRecordCleared = true;
    try {
      removePidRecord(this.llm.projectDir, this.sessionId);
    } catch {
      // ignored; removePidRecord already logs
    }
  }

  // ------------------------------------------------------------------
  // Internals
  // ------------------------------------------------------------------

  private onStdoutChunk(chunk: string): void {
    this.stdoutBuffer += chunk;
    let nl = this.stdoutBuffer.indexOf("\n");
    while (nl !== -1) {
      const raw = this.stdoutBuffer.slice(0, nl);
      this.stdoutBuffer = this.stdoutBuffer.slice(nl + 1);
      const trimmed = raw.trim();
      if (trimmed.length > 0) {
        this.handleProtocolLine(trimmed);
      }
      nl = this.stdoutBuffer.indexOf("\n");
    }
  }

  private handleProtocolLine(line: string): void {
    this.debugLog.logRawIn(line);
    let event: ProtocolEvent;
    try {
      event = JSON.parse(line) as ProtocolEvent;
    } catch (err) {
      this.renderDiagnostic("error", `protocol: bad JSON from sim-flow: ${(err as Error).message}`);
      return;
    }
    this.debugLog.logEventIn(event);
    this.handleEvent(event);
  }

  private handleEvent(event: ProtocolEvent): void {
    switch (event.event) {
      case "hello-ack":
        this.sessionTag = event.session;
        this.stepDescriptor = event.step_descriptor;
        this.renderHelloAck(event);
        break;
      case "assistant-text":
        if (event.text.length > 0) {
          this.currentRenderer?.markdown(event.text);
        }
        break;
      case "request-user-input":
        // Orchestrator parked; release the current settle promise.
        this.bus.emit("msg", {
          type: "settled",
          result: { status: "awaiting-input" },
        } as PumpBusEvent);
        break;
      case "request-llm-response":
        // Run the LLM call concurrently; further events from the
        // orchestrator continue to flow through this same pump.
        this.dispatchLlm(event).catch((err) => {
          this.renderDiagnostic(
            "error",
            `LLM dispatch threw: ${(err as Error).message ?? String(err)}`,
          );
        });
        break;
      case "artifact-written":
        this.currentRenderer?.markdown(`\n_Wrote \`${event.path}\` (${event.bytes} bytes)._\n`);
        break;
      case "tool-invoked":
        this.currentRenderer?.markdown(
          `\n_Tool \`${event.name}\` ${event.args_summary ? `(${event.args_summary}) ` : ""}-> ${event.status} (${event.duration_ms} ms)._\n`,
        );
        break;
      case "phase-changed":
        this.currentRenderer?.markdown(`\n**Phase:** \`${event.phase}\`\n`);
        break;
      case "build-output":
        this.currentRenderer?.markdown(
          `\n**\`${event.command}\`** exited with status \`${event.exit_code}\`.\n`,
        );
        break;
      case "gate-result":
        if (event.clean) {
          this.currentRenderer?.markdown(`\n**Gate \`${event.step}\`: clean.**\n`);
        } else {
          const lines = event.failures.map((f) => `- ${f.description}: ${f.reason}`).join("\n");
          this.currentRenderer?.markdown(
            `\n**Gate \`${event.step}\`: ${event.failures.length} failure(s).**\n\n${lines}\n`,
          );
        }
        break;
      case "state-advanced":
        this.currentRenderer?.markdown(
          `\n**Advanced past \`${event.from}\`${event.to ? `; current step is now \`${event.to}\`.` : ` (final step in this flow).`}**\n`,
        );
        break;
      case "followup":
        // Surface as plain text; M5 doesn't yet wire the chat
        // followup-provider for sub-session followups.
        this.currentRenderer?.markdown(
          `\n_Suggested next: ${event.label} (\`${event.action}\`)._\n`,
        );
        break;
      case "diagnostic":
        this.renderDiagnostic(event.level, event.message);
        break;
      case "session-end":
        this.markTerminated({
          status: "ended",
          endReason: event.reason,
          endMessage: event.message ?? undefined,
        });
        break;
      case "step-mode-changed":
        // Step-axis mode flipped (manual <-> auto). Dashboard wiring
        // for the toggle UI lands in the extension-side PR; for now
        // the debug log already records the event and consumers that
        // care can subscribe via the bus's `event` channel.
        break;
      case "sub-session-started":
      case "sub-session-ended":
        // Stdio pump doesn't track sub-session bracketing; the
        // socketPump handles it for the dashboard. The events
        // already round-trip through the debug log above.
        break;
      default: {
        const exhaustive: never = event;
        void exhaustive;
      }
    }
  }

  private renderHelloAck(event: ProtocolEvent & { event: "hello-ack" }): void {
    const banner = [
      `**Step \`${event.session.step}\` ${event.session.kind} session**`,
      event.session.candidate ? `(candidate \`${event.session.candidate}\`)` : null,
      `— sim-flow ${event.sim_flow_version}; protocol v${event.protocol_version}; backend \`${this.llm.source}\`${this.llm.model ? ` (\`${this.llm.model}\`)` : ""}.`,
    ]
      .filter(Boolean)
      .join(" ");
    this.currentRenderer?.markdown(`${banner}\n\n`);
    if (event.step_descriptor.phases.length > 0) {
      this.currentRenderer?.markdown(
        `_Phases:_ ${event.step_descriptor.phases.map((p) => `\`${p}\``).join(" -> ")}\n\n`,
      );
    }
  }

  private renderDiagnostic(level: string, message: string): void {
    const tag = level === "error" ? "**Error**" : level === "warning" ? "**Warning**" : "**Info**";
    this.currentRenderer?.markdown(`\n${tag}: ${message}\n`);
  }

  /**
   * Snapshot the LLM-side settings as of right now. Re-read on every
   * dispatch so the user can hot-swap source/model from the
   * dashboard while a chat is running -- e.g. switching from
   * Anthropic to Ollama mid-flow when API tokens are exhausted.
   * Falls back to the values captured at construction for fields
   * that aren't represented as VSCode settings (binary path, secrets
   * handle, etc.).
   */
  private readLiveLlmConfig(): {
    source: LlmSource;
    rawSource: string;
    model?: string;
    ollamaBaseUrl?: string;
    lmstudioBaseUrl?: string;
    /** See `socketPump.readLiveLlmConfig` for semantics. */
    serverBaseUrl: string | null | undefined;
    verbose: boolean;
  } {
    const config = vscode.workspace.getConfiguration("sim-flow");
    const rawSource = (config.get<string>("llm.source") ?? this.llm.source) as string;
    const model = (config.get<string>("llm.model") ?? "").trim() || this.llm.model;
    const ollamaBaseUrl =
      (config.get<string>("llm.ollama.baseUrl") ?? "").trim() || this.llm.ollamaBaseUrl;
    const lmstudioBaseUrl =
      (config.get<string>("llm.lmstudio.baseUrl") ?? "").trim() || this.llm.lmstudioBaseUrl;
    const verbose = config.get<boolean>("llm.verbose") ?? true;
    const servers = (config.get<unknown>("llm.servers") as LlmServerEntry[] | undefined) ?? [];
    const resolved = resolveLlmSource(rawSource, servers);
    if (resolved === null) {
      return {
        source: rawSource as LlmSource,
        rawSource,
        model,
        ollamaBaseUrl,
        lmstudioBaseUrl,
        serverBaseUrl: null,
        verbose,
      };
    }
    return {
      source: resolved.source as LlmSource,
      rawSource,
      model: resolved.model ?? model,
      ollamaBaseUrl,
      lmstudioBaseUrl,
      serverBaseUrl: resolved.baseUrl,
      verbose,
    };
  }

  private async dispatchLlm(
    event: ProtocolEvent & { event: "request-llm-response" },
  ): Promise<void> {
    this.currentRequestId++;
    const token = new vscode.CancellationTokenSource().token;
    // Re-read the live settings on every dispatch. Pumps live across
    // many turns and the user can switch LLM source mid-run via the
    // dashboard; the captured config from constructor time would
    // pin them to whatever was active when the chat tab opened.
    const live = this.readLiveLlmConfig();
    if (live.serverBaseUrl === null) {
      this.sendHostEvent({
        event: "llm-error",
        request_id: event.request_id,
        kind: "unsupported",
        message: `LLM source \`${live.rawSource}\` references a custom server that isn't defined in \`sim-flow.llm.servers\`. Add the entry in the dashboard's Settings tab, or pick a built-in source.`,
      });
      return;
    }
    if (live.source !== this.lastUsedSource) {
      this.currentRenderer?.markdown(
        `_LLM source switched: \`${this.lastUsedSource ?? "(initial)"}\` → \`${live.source}\`._\n\n`,
      );
      this.lastUsedSource = live.source;
    }
    let backend: LlmBackend;
    try {
      backend = createBackend({
        source: live.source,
        model: live.model ?? event.model ?? undefined,
        secrets: this.llm.secrets,
        projectDir: this.llm.projectDir,
        binary: this.llm.binary,
        ollamaBaseUrl: live.ollamaBaseUrl,
        lmstudioBaseUrl: live.lmstudioBaseUrl,
        baseUrl: live.serverBaseUrl ?? undefined,
        // The CLI fallback backend has been retired (Phase 9 M5); a
        // stub session keeps the type happy for the few branches
        // that still touch it.
        session: undefined,
      });
    } catch (err) {
      this.sendHostEvent({
        event: "llm-error",
        request_id: event.request_id,
        kind: err instanceof LlmError ? err.kind : "factory",
        message: (err as Error).message ?? String(err),
      });
      return;
    }
    const messages: BackendLlmMessage[] = (event.messages as ProtocolLlmMessage[]).map((m) => ({
      role: m.role,
      content: m.content,
      attachments: m.attachments?.map((a) => ({
        mime: a.mime,
        data: a.data,
        source: a.source ?? undefined,
      })),
    }));
    // When the user has unchecked "Verbose" in the dashboard, slip a
    // brevity directive in just before the user message. The
    // orchestrator's prompts always end with one user turn, so
    // splicing at length-1 lands us after every system message but
    // ahead of the actual ask. Skipped when verbose=true so models'
    // natural tone shows through.
    if (!live.verbose) {
      const insertAt = Math.max(0, messages.length - 1);
      messages.splice(insertAt, 0, {
        role: "system",
        content: BREVITY_DIRECTIVE,
      });
    }
    this.currentRenderer?.requestTokensEstimate?.(estimateMessagesTokens(messages));
    const tools = event.tools?.map((t) => ({
      name: t.name,
      description: t.description,
      args_schema: t.args_schema,
    }));
    this.debugLog.logLlmDispatch(messages);
    let chunkCount = 0;
    let totalChars = 0;
    // Reasoning chunks from Qwen3-Coder / DeepSeek-R1 / o-series go
    // into a collapsed `<details>` block in the chat pane and are
    // NOT forwarded to the orchestrator (they would otherwise
    // pollute artifact / tool-call extraction). We open the block
    // lazily on the first reasoning delta and close it the moment
    // real content arrives, when the stream ends, or on error.
    let reasoningOpen = false;
    const closeReasoning = () => {
      if (reasoningOpen) {
        this.currentRenderer?.markdown("\n\n</details>\n\n");
        reasoningOpen = false;
      }
    };
    try {
      for await (const chunk of backend.stream(messages, token, tools)) {
        if (chunk.text.length === 0) {
          continue;
        }
        if (chunk.kind === "reasoning") {
          if (!reasoningOpen) {
            this.currentRenderer?.markdown(
              "\n<details>\n<summary>Model reasoning (click to expand)</summary>\n\n",
            );
            reasoningOpen = true;
          }
          this.currentRenderer?.markdown(chunk.text);
          continue;
        }
        // Real content arrived; finalize any open reasoning block first
        // so the collapsible doesn't swallow the answer.
        closeReasoning();
        chunkCount++;
        totalChars += chunk.text.length;
        this.debugLog.logLlmChunk(chunk.text);
        this.sendHostEvent({
          event: "llm-chunk",
          request_id: event.request_id,
          text: chunk.text,
        });
      }
      closeReasoning();
      this.debugLog.logLlmEnd(totalChars, chunkCount);
      this.sendHostEvent({
        event: "llm-end",
        request_id: event.request_id,
        stop_reason: "stop",
      });
    } catch (err) {
      closeReasoning();
      this.debugLog.logLlmError(err);
      const baseMessage = (err as Error).message ?? String(err);
      const detail = err instanceof LlmError ? err.detail : undefined;
      const composed =
        detail && detail.length > 0
          ? `${baseMessage} -- response: ${detail.slice(0, 512)}`
          : baseMessage;
      this.sendHostEvent({
        event: "llm-error",
        request_id: event.request_id,
        kind: err instanceof LlmError ? err.kind : "stream",
        message: composed,
      });
    }
  }

  private sendHostEvent(event: HostEvent): void {
    if (this.terminated) {
      return;
    }
    this.debugLog.logEventOut(event);
    const line = `${JSON.stringify(event)}\n`;
    this.debugLog.logRawOut(line);
    try {
      this.process.stdin.write(line);
    } catch (err) {
      this.renderDiagnostic("error", `failed to send host event: ${(err as Error).message}`);
    }
  }

  private markTerminated(result: PumpSettleResult): void {
    if (this.terminated) {
      return;
    }
    this.terminated = true;
    this.terminationReason = result;
    this.bus.emit("msg", { type: "settled", result } as PumpBusEvent);
  }
}

/**
 * One-shot helper: spawn a session, send a single user message, and
 * collect the rendered output. Currently only used by tests; the
 * real chat path uses the registry.
 */
export async function runOneShot(
  options: SessionPumpOptions,
  llm: PumpLlmConfig,
  userMessage: string,
  renderer: PumpRenderer,
): Promise<PumpSettleResult> {
  const pump = new SessionPump(options, llm);
  let result = await pump.settle(renderer);
  if (result.status === "awaiting-input") {
    pump.sendUserMessage(userMessage);
    result = await pump.settle(renderer);
  }
  pump.dispose();
  return result;
}

// `once` is currently unused but kept available for the registry's
// teardown path in M5.5.
void once;
