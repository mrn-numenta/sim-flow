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
// LLM dispatch: the orchestrator owns LLM dispatch end-to-end now.
// The pump used to receive `RequestLlmResponse` events and ship the
// streaming reply back as `LlmChunk` / `LlmEnd`, but those wire
// events were removed once the orchestrator absorbed the LLM
// client family. The pump's remaining job is to forward `UserMessage`
// / control events to the orchestrator and surface presentation
// events (`AssistantText`, `RequestUserInput`, `Diagnostic`, ...)
// back to the chat renderer.

import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { randomUUID } from "node:crypto";
import { EventEmitter, once } from "node:events";

import * as vscode from "vscode";

import { bundledFrameworkDocsRoot, bundledPdfiumLibPath } from "../cli";
import type { LlmSource, SecretStorage } from "../llm";
import type {
  Event as ProtocolEvent,
  HostEvent,
  HostInfo,
  SessionKindOut,
  SessionTag,
  StepDescriptorOut,
  StepMode,
} from "./protocol-types";
import { renderBuildOutput } from "./buildOutput";
import { DebugLog } from "./debug-log";
import { removePidRecord, writePidRecord } from "./processRegistry";

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

/**
 * LLM-related metadata the pump needs to keep around for header
 * rendering and pid-record tagging. The extension no longer
 * dispatches LLM calls; these fields are pure labels that the
 * Rust orchestrator already received via `--llm-backend` /
 * `--llm-model` argv.
 */
export interface PumpLlmConfig {
  source: LlmSource;
  model?: string;
  modelFamilyId?: string;
  runtimeProfileId?: string;
  /**
   * Generic base-URL override forwarded to the orchestrator via
   * `--llm-base-url`. Set when the user picks a custom server
   * (`server:<name>` in the source picker).
   */
  baseUrl?: string;
  ollamaBaseUrl?: string;
  lmstudioBaseUrl?: string;
  secrets?: SecretStorage;
  /** Sim-foundation project dir, used for pid records and debug-log paths. */
  projectDir: string;
  /** Sim-flow CLI binary path. */
  binary: string;
  /**
   * Comma-joined debug categories from `sim-flow.debug` (or the
   * `SIM_FOUNDATION_DEBUG` env var when the setting is empty). Empty
   * string disables logging on both sides. Forwarded to the
   * subprocess so the orchestrator's DebugLog sees the same value.
   */
  debugTokens: string;
  debugAdaptation?: boolean;
  /**
   * Forwarded to the orchestrator via `--llm-stream-idle-timeout-ms`
   * for callers that want to override the openai-compat backend
   * idle-timeout. The pump itself no longer streams.
   */
  streamIdleTimeoutMs?: number;
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
  /**
   * Optional, experimental: structured "prompt sent to LLM" event
   * carrying the latest non-Assistant message the orchestrator
   * appended to the prompt stack before dispatching. Lets hosts
   * render the running prompt+response transcript instead of just
   * the assistant prose. Renderers that don't implement this
   * simply never see the data; the chat panel falls back to
   * `markdown()` for everything else.
   */
  llmRequest?(args: {
    role: string;
    content: string;
    turnIndex: number;
    requestId: string;
    /** Stable orchestrator-side message id (e.g. `msg-12`). The
     *  chat panel stashes this on the bubble so later
     *  `ContextEvicted` events can mark the matching row. `null`
     *  on legacy emits / synthetic events that aren't tied to a
     *  prompt-stack slot. */
    messageId: string | null;
  }): void;
  /**
   * Optional, experimental: assistant turn carrying both the prose
   * `text` AND the native `tool_calls` the LLM emitted this turn.
   * When implemented, the pump routes the `assistant-text` event
   * here instead of through `markdown(text)` so the host gets the
   * full reply (including tool-only turns where `text` is empty).
   */
  assistantTurn?(args: {
    text: string;
    finalChunk: boolean;
    toolCalls: Array<{ id?: string; name: string; argumentsJson: string }>;
  }): void;
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
   * Continue-flow shortcut. Tells the orchestrator to run the next
   * logical manual-mode action (work → critique → advance →
   * work-on-next-step) without the host having to compute which
   * action that is. The orchestrator already knows from
   * state.toml + critique resolution + its own sub-session
   * history. Only the JSONL transport implements this; older
   * orchestrators reject the event with a Diagnostic.
   */
  continueFlow?(): void;
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
  /**
   * Subscribe to `request-user-input` events. Notifies with the
   * prompt + placeholder text the orchestrator embedded in the
   * event so the host can render a banner above its composer.
   * Either field can be null when the orchestrator parks without
   * an explicit question. Only the socket pump implements this;
   * the stdio pump leaves it undefined and the caller falls back
   * to a generic awaiting-input notice.
   */
  onRequestUserInput?(
    listener: (msg: { prompt: string | null; placeholder: string | null }) => void,
  ): () => void;
  /**
   * Subscribe to `Followup` events. Each notification carries a
   * label (button text) and action string (the literal message
   * that should be shipped back as a `UserMessage` on click). Hosts
   * that declared the `followups` capability render these as
   * clickable affordances; hosts that didn't can ignore the bus
   * (the orchestrator also still emits the legacy "Suggested next"
   * markdown line for plain renderers). Optional like the other
   * pump subscriptions; the stdio pump leaves it undefined.
   */
  onFollowup?(listener: (msg: { label: string; action: string }) => void): () => void;
  /**
   * Subscribe to `NextActionHint` events. The orchestrator emits
   * these every time it parks at `wait_for_command` (manual mode),
   * carrying a pre-rendered label like "Run critique on DM2d" or
   * "Advance past DM0". The chat panel surfaces this on its
   * Continue button so the user sees what the next click will do
   * without the chat panel having to duplicate the orchestrator's
   * state machine. `label === null` means the orchestrator has
   * nothing useful to suggest (e.g. no current step) -- render
   * the Continue button as disabled.
   */
  onNextActionHint?(
    listener: (msg: { label: string | null }) => void,
  ): () => void;
  /**
   * Subscribe to prompt-stack compaction events. Each call carries
   * the message ids the orchestrator just evicted plus the reason
   * (dedup / mutation-invalidation / phase-boundary / etc.). The
   * chat panel uses this to mark matching transcript rows with a
   * "no longer in context" indicator -- the transcript itself is
   * never modified. Absent on stdio / mock transports.
   */
  onContextEvicted?(
    listener: (msg: {
      ids: string[];
      reason: import("./protocol-types").ContextEvictionReason;
    }) => void,
  ): () => void;
  /**
   * True when this pump is attached as a read-only observer to a
   * `--watch-socket` tap. Dashboard / chat panel use this to
   * disable command surfaces (Run Step / Run Critique / Send /
   * Stop) since the user can't drive a run that something else
   * owns. Absent on stdio / mock transports.
   */
  readonly isViewer?: boolean;
  /**
   * Structured gate-result observation surface. The orchestrator
   * emits `Event::GateResult` over JSONL when a manual Run Gate
   * click runs; the pump exposes it here so the dashboard host can
   * post a `gate-result` HostMessage that updates the per-step gate
   * cache and clears the matching pending-action entry. JSONL-only
   * transport optional; absent on stdio / mock.
   */
  onGateResult?(
    listener: (result: {
      step: string;
      clean: boolean;
      failures: { description: string; reason: string }[];
    }) => void,
  ): () => void;
}

/**
 * Optional brevity directive the chat panel used to splice into LLM
 * messages when `sim-flow.llm.verbose` was off. Kept as an exported
 * constant only because the mockFlowHarness test still imports it
 * to assert on the chat panel's settings plumbing. Pumps no longer
 * touch it — the orchestrator handles brevity directly via its own
 * `--llm-verbose` flag.
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
  private terminated = false;
  private terminationReason: PumpSettleResult | null = null;
  private sessionTag: SessionTag | null = null;
  private stepDescriptor: StepDescriptorOut | null = null;
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
        this.debugLog.logSpawnError(`pid registry write failed: ${(err as Error).message}`);
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
      capabilities: [
        "text",
        "markdown",
        "user-input",
        "tool-notifications",
        "followups",
      ],
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
      case "assistant-text": {
        const toolCalls = (event.tool_calls ?? []).map((c) => ({
          id: c.id ?? undefined,
          name: c.name,
          argumentsJson: c.arguments_json,
        }));
        if (this.currentRenderer?.assistantTurn) {
          this.currentRenderer.assistantTurn({
            text: event.text,
            finalChunk: event.final_chunk,
            toolCalls,
          });
        } else if (event.text.length > 0) {
          this.currentRenderer?.markdown(event.text);
        }
        break;
      }
      case "llm-request":
        this.currentRenderer?.llmRequest?.({
          role: event.role,
          content: event.content,
          turnIndex: event.turn_index,
          requestId: event.request_id,
          messageId: event.message_id ?? null,
        });
        break;
      case "request-user-input":
        // Orchestrator parked; release the current settle promise.
        this.bus.emit("msg", {
          type: "settled",
          result: { status: "awaiting-input" },
        } as PumpBusEvent);
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
        this.currentRenderer?.markdown(renderBuildOutput(event));
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
      case "next-action-hint":
        // Stdio pump doesn't drive the chat panel's Continue button;
        // the socketPump translates this into a bus event for the
        // chat panel. Debug log already records it above.
        break;
      case "context-evicted":
        // Chat-panel-only signal; the socketPump translates it
        // into a bus event so the experimental UI can mark the
        // matching transcript rows. Stdio has no transcript.
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
