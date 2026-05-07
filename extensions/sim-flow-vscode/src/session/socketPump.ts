import { spawn, type ChildProcess } from "node:child_process";
import { EventEmitter } from "node:events";
import * as net from "node:net";

import * as vscode from "vscode";

import {
  type LlmBackend,
  type LlmMessage as BackendLlmMessage,
  createBackend,
  LlmError,
  type LlmSource,
} from "../llm";
import { estimateMessagesTokens } from "../llm/tokenEstimate";
import { DebugLog } from "./debug-log";
import { removePidRecord, writePidRecord } from "./processRegistry";
import { acquirePumpLock, type PumpLock } from "./pumpLock";
import {
  BREVITY_DIRECTIVE,
  type LiveSessionTransport,
  type PumpLlmConfig,
  type PumpRenderer,
  type PumpSettleResult,
} from "./pump";
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

type SocketPumpBusEvent =
  | { type: "settled"; result: PumpSettleResult }
  | { type: "step-mode"; mode: StepMode }
  | { type: "in-sub-session"; inSubSession: boolean };

export interface SocketSessionPumpOptions {
  sessionId: string;
  socketPath: string;
  attachTimeoutMs?: number;
  launch?: {
    binary: string;
    args: string[];
    cwd: string;
    env?: NodeJS.ProcessEnv;
  };
}

export class SocketSessionPump implements LiveSessionTransport {
  private readonly bus = new EventEmitter();
  private readonly debugLog: DebugLog;
  private readonly connectionReady: Promise<void>;
  private readonly attachTimeoutMs: number;
  private readonly queuedEvents: ProtocolEvent[] = [];
  private socket: net.Socket | undefined;
  private currentRenderer: PumpRenderer | null = null;
  private stdoutBuffer = "";
  private terminated = false;
  private terminationReason: PumpSettleResult | null = null;
  private sessionTag: SessionTag | null = null;
  private stepDescriptor: StepDescriptorOut | null = null;
  private lastUsedSource: LlmSource | null = null;
  /**
   * Spawned `sim-flow` child. Tracked so `dispose()` can SIGTERM it
   * (otherwise the child outlives the pump if its socket close
   * handler doesn't notice the disconnect quickly enough). `null`
   * when no child was spawned (attach-only mode for tests / future
   * reconnection).
   */
  private child: ChildProcess | null = null;
  /** True after we've reaped `<project>/.sim-flow/pids/<id>.json`. */
  private pidRecordCleared = false;
  /**
   * Per-project flock guarding against a second window racing this
   * pump for the same project. `null` in attach-only mode (no spawn,
   * lock is somebody else's responsibility) or when the spawn was
   * blocked by an existing live holder. `release()` runs from
   * `dispose()`.
   */
  private pumpLock: PumpLock | null = null;
  /**
   * Last `StepModeChanged` value the orchestrator emitted. The
   * orchestrator echoes the initial mode as soon as it's spawned and
   * re-emits on every flip (user toggle, cap exceeded, gate failure
   * on advance), so this is the truth the dashboard's toggle should
   * reflect. `null` until the first echo arrives.
   */
  private currentStepMode: StepMode | null = null;
  /**
   * True while the orchestrator is inside a sub-session (work or
   * critique). Set by `sub-session-started` and cleared by
   * `sub-session-ended`. The dashboard reads this to disable
   * per-step buttons while the orchestrator is busy — clicking
   * Run Gate / Run Step / Run Critique / Advance during this span
   * is rejected on the orchestrator side anyway, but disabling
   * here gives clearer feedback than a Diagnostic warning after
   * the click.
   */
  private inSubSessionFlag = false;

  constructor(
    private readonly options: SocketSessionPumpOptions,
    private readonly llm: PumpLlmConfig,
  ) {
    this.lastUsedSource = llm.source;
    this.debugLog = DebugLog.fromTokens(llm.debugTokens, llm.projectDir);
    this.attachTimeoutMs = options.attachTimeoutMs ?? 5000;
    if (options.launch) {
      // Acquire the per-project pump lock BEFORE spawning. A second
      // VS Code window with the same project open would otherwise
      // race us for the JSONL socket and `.sim-flow/state.toml`.
      const lockResult = acquirePumpLock(llm.projectDir, options.sessionId);
      if (!lockResult.ok) {
        this.markTerminated({
          status: "ended",
          endReason: "spawn-error",
          endMessage: lockResult.message,
        });
        this.connectionReady = Promise.reject(new Error(lockResult.message));
        // Swallow the unhandled rejection -- callers observe the
        // terminal state via `settle()`, which checks
        // `terminationReason` first.
        this.connectionReady.catch(() => undefined);
        return;
      }
      this.pumpLock = lockResult.lock;
      // Mirror SessionPump's env wiring so the spawned `sim-flow auto`
      // process sees the same `SIM_FOUNDATION_DEBUG` value that drives
      // the extension-side DebugLog. Without this the orchestrator's
      // own DebugLog::open sees no env var and never writes
      // `<project>/.sim-flow/logs/sim-flow-chat.log` even when the
      // user has `sim-flow.debug` set in settings.
      const baseEnv = options.launch.env ?? process.env;
      const env: NodeJS.ProcessEnv = {
        ...baseEnv,
        SIM_FOUNDATION_DEBUG: llm.debugTokens,
      };
      const child = spawn(options.launch.binary, options.launch.args, {
        cwd: options.launch.cwd,
        env,
        stdio: ["ignore", "ignore", "ignore"],
      });
      this.child = child;
      this.debugLog.logProcessSpawn(
        options.launch.binary,
        options.launch.args,
        child.pid,
      );
      // Stash a pid record under `<project>/.sim-flow/pids/<sessionId>.json`
      // so that on the next extension activate we can reap orphans
      // left behind by a host crash. The record is removed when the
      // child exits cleanly or `dispose()` runs.
      if (typeof child.pid === "number") {
        try {
          writePidRecord(llm.projectDir, {
            pid: child.pid,
            sessionId: options.sessionId,
            binary: options.launch.binary,
            label: options.launch.args.slice(0, 2).join(" "),
            spawnedAtMs: Date.now(),
          });
        } catch (err) {
          // Non-fatal: the child still runs; we just can't reap it
          // automatically on a future activate. Log via debug log.
          this.debugLog.logSpawnError(
            `pid registry write failed: ${(err as Error).message}`,
          );
        }
      }
      child.on("error", (err) => {
        this.debugLog.logSpawnError(err.message);
        this.clearPidRecord();
        this.markTerminated({
          status: "ended",
          endReason: "spawn-error",
          endMessage: err.message,
        });
      });
      child.on("exit", (code, signal) => {
        this.debugLog.logProcessExit(code, signal, "");
        this.clearPidRecord();
        if (!this.terminated) {
          this.markTerminated({
            status: "ended",
            endReason: code === 0 ? "completed" : "process-exit",
            endMessage:
              code === 0
                ? undefined
                : `process exited with code ${code}${signal ? ` (signal ${signal})` : ""}`,
          });
        }
      });
    }
    this.connectionReady = this.connect();
  }

  private clearPidRecord(): void {
    if (this.pidRecordCleared) {
      return;
    }
    this.pidRecordCleared = true;
    try {
      removePidRecord(this.llm.projectDir, this.options.sessionId);
    } catch {
      // already logged by removePidRecord; ignore here so we don't
      // throw out of an exit handler.
    }
  }

  get session(): SessionTag | null {
    return this.sessionTag;
  }

  get descriptor(): StepDescriptorOut | null {
    return this.stepDescriptor;
  }

  async ready(): Promise<void> {
    await this.connectionReady;
  }

  async settle(renderer: PumpRenderer): Promise<PumpSettleResult> {
    try {
      await this.connectionReady;
    } catch (err) {
      return this.terminationReason ?? {
        status: "ended",
        endReason: "attach-failed",
        endMessage: (err as Error).message ?? String(err),
      };
    }
    this.currentRenderer = renderer;
    return new Promise<PumpSettleResult>((resolve) => {
      const onSettled = (msg: SocketPumpBusEvent) => {
        if (msg.type !== "settled") {
          return; // step-mode notifications flow on the same bus
        }
        this.bus.off("msg", onSettled);
        this.currentRenderer = null;
        resolve(msg.result);
      };
      this.bus.on("msg", onSettled);
      if (this.terminated && this.terminationReason) {
        this.bus.emit("msg", {
          type: "settled",
          result: this.terminationReason,
        } as SocketPumpBusEvent);
        return;
      }
      this.flushQueuedEvents();
    });
  }

  sendUserMessage(text: string): void {
    void this.connectionReady
      .then(() => {
        this.sendHostEvent({ event: "user-message", text });
      })
      .catch((err) => {
        this.markTerminated({
          status: "ended",
          endReason: "attach-failed",
          endMessage: (err as Error).message ?? String(err),
        });
      });
  }

  cancel(): void {
    void this.connectionReady
      .then(() => {
        this.sendHostEvent({ event: "cancel" });
      })
      .catch(() => {
        // ignore; caller will observe the terminal state
      });
  }

  /**
   * Last step-mode the orchestrator confirmed via `StepModeChanged`.
   * `null` means the pump hasn't received the initial echo yet — the
   * dashboard reflects the persisted setting in that case.
   */
  get stepMode(): StepMode | null {
    return this.currentStepMode;
  }

  /**
   * Subscribe to `StepModeChanged` events from the orchestrator.
   * Returns a disposer. The dashboard registers one listener per
   * panel; the chat host can register another to refresh state.
   */
  onStepModeChanged(listener: (mode: StepMode) => void): () => void {
    const wrapped = (msg: SocketPumpBusEvent) => {
      if (msg.type === "step-mode") {
        listener(msg.mode);
      }
    };
    this.bus.on("msg", wrapped);
    return () => this.bus.off("msg", wrapped);
  }

  /**
   * Whether the orchestrator is currently inside a sub-session.
   * `true` between `sub-session-started` and `sub-session-ended`;
   * `false` while parked. The dashboard uses this to gate the
   * per-step buttons.
   */
  get inSubSession(): boolean {
    return this.inSubSessionFlag;
  }

  /**
   * Subscribe to sub-session bracket transitions. The listener
   * fires once on every sub-session boundary; the dashboard
   * triggers a refresh so the per-step buttons re-evaluate.
   */
  onSubSessionChanged(listener: (inSubSession: boolean) => void): () => void {
    const wrapped = (msg: SocketPumpBusEvent) => {
      if (msg.type === "in-sub-session") {
        listener(msg.inSubSession);
      }
    };
    this.bus.on("msg", wrapped);
    return () => this.bus.off("msg", wrapped);
  }

  /**
   * Manual-mode host commands. Each one fires-and-forgets — the
   * orchestrator emits `Diagnostic` if the command is rejected (auto
   * mode owns step execution, sub-session in flight, etc.) and that
   * surfaces to the user via the existing diagnostic renderer.
   */
  setStepMode(mode: StepMode): void {
    this.sendHostEventAfterReady({ event: "set-step-mode", mode });
  }

  runStep(step: string, kind: SessionKindOut): void {
    this.sendHostEventAfterReady({ event: "run-step", step, kind });
  }

  runCritique(step: string): void {
    this.sendHostEventAfterReady({ event: "run-critique", step });
  }

  runGate(step: string): void {
    this.sendHostEventAfterReady({ event: "run-gate", step });
  }

  advance(step: string): void {
    this.sendHostEventAfterReady({ event: "advance", step });
  }

  reset(step: string): void {
    this.sendHostEventAfterReady({ event: "reset", step });
  }

  shutdown(): void {
    this.sendHostEventAfterReady({ event: "shutdown" });
  }

  /**
   * Graceful disconnect with escalation. Sequence:
   *   1. Send `shutdown` over the socket so the orchestrator
   *      finishes its current turn and exits cleanly.
   *   2. Wait up to `cleanTimeoutMs` for the child to exit.
   *   3. If still alive, send SIGTERM (`dispose` already does
   *      this on its own; we wait again afterward).
   *   4. Wait up to `termTimeoutMs` for SIGTERM to take effect.
   *   5. If still alive, SIGKILL.
   *
   * Returns once the child is reaped or after the worst-case
   * deadline. Safe to call concurrently with `dispose`; the
   * second-arrival-loses race is handled by the underlying
   * `terminated` flag.
   */
  async disconnectWithEscalation(
    cleanTimeoutMs = 5_000,
    termTimeoutMs = 2_000,
  ): Promise<"clean" | "sigterm" | "sigkill" | "already-gone"> {
    if (!this.child || this.child.exitCode !== null || this.child.signalCode !== null) {
      this.dispose();
      return "already-gone";
    }
    this.shutdown();
    if (await this.waitForChildExit(cleanTimeoutMs)) {
      this.dispose();
      return "clean";
    }
    try {
      this.child?.kill("SIGTERM");
    } catch {
      // child already gone
    }
    if (await this.waitForChildExit(termTimeoutMs)) {
      this.dispose();
      return "sigterm";
    }
    try {
      this.child?.kill("SIGKILL");
    } catch {
      // child already gone
    }
    // Best-effort wait for the kill to land. Clean up regardless.
    await this.waitForChildExit(1_000);
    this.dispose();
    return "sigkill";
  }

  /** Resolve true when the child has exited within `ms`, false on timeout. */
  private waitForChildExit(ms: number): Promise<boolean> {
    return new Promise((resolve) => {
      if (!this.child || this.child.exitCode !== null || this.child.signalCode !== null) {
        resolve(true);
        return;
      }
      const child = this.child;
      const timer = setTimeout(() => {
        child.removeListener("exit", onExit);
        resolve(false);
      }, ms);
      const onExit = (): void => {
        clearTimeout(timer);
        resolve(true);
      };
      child.once("exit", onExit);
    });
  }

  private sendHostEventAfterReady(event: HostEvent): void {
    void this.connectionReady
      .then(() => {
        this.sendHostEvent(event);
      })
      .catch(() => {
        // Pump never attached; the caller will observe the terminal
        // state via the existing settle promise.
      });
  }

  dispose(): void {
    if (this.socket && !this.socket.destroyed) {
      this.socket.destroy();
    }
    this.socket = undefined;
    // Tear down the spawned `sim-flow` child so it doesn't outlive
    // the pump. Socket close usually causes the orchestrator to exit
    // on its own (the JsonlHost / SocketHost reads return None), but
    // a stuck orchestrator (e.g. blocked in an LLM call) won't
    // notice for a while. SIGTERM is the polite kick. The child's
    // `exit` handler will clear the pid record.
    if (this.child && this.child.exitCode === null && this.child.signalCode === null) {
      try {
        this.child.kill("SIGTERM");
      } catch {
        // child already gone
      }
    }
    this.child = null;
    // Defensive: if the exit handler never fires (e.g. test harnesses
    // that mock spawn), reap the pid record here too.
    this.clearPidRecord();
    if (this.pumpLock) {
      try {
        this.pumpLock.release();
      } catch {
        // Already logged; never throw out of dispose.
      }
      this.pumpLock = null;
    }
    this.debugLog.dispose();
  }

  private async connect(): Promise<void> {
    const startedAt = Date.now();
    while (!this.terminated) {
      try {
        await this.openSocket();
        return;
      } catch (err) {
        if (Date.now() - startedAt >= this.attachTimeoutMs) {
          this.markTerminated({
            status: "ended",
            endReason: "attach-failed",
            endMessage: `Failed to attach to reconnectable session ${this.options.sessionId}: ${(err as Error).message ?? String(err)}`,
          });
          throw err;
        }
        await delay(50);
      }
    }
  }

  private async openSocket(): Promise<void> {
    const socket = await new Promise<net.Socket>((resolve, reject) => {
      const next = net.createConnection(this.options.socketPath);
      next.once("error", (err) => {
        next.destroy();
        reject(err);
      });
      next.once("connect", () => {
        resolve(next);
      });
    });
    socket.setEncoding("utf8");
    socket.on("data", (chunk: string | Buffer) => {
      this.onSocketChunk(typeof chunk === "string" ? chunk : chunk.toString("utf8"));
    });
    socket.on("error", (err) => {
      if (!this.terminated) {
        this.markTerminated({
          status: "ended",
          endReason: "transport-error",
          endMessage: `Session transport error: ${err.message ?? String(err)}`,
        });
      }
    });
    socket.on("close", () => {
      if (!this.terminated) {
        this.markTerminated({
          status: "ended",
          endReason: "transport-closed",
          endMessage: "The reconnectable session transport closed before the session finished.",
        });
      }
    });
    this.socket = socket;
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

  private onSocketChunk(chunk: string): void {
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
    if (
      this.currentRenderer === null &&
      event.event !== "session-end" &&
      event.event !== "step-mode-changed" &&
      event.event !== "sub-session-started" &&
      event.event !== "sub-session-ended"
    ) {
      // Defer most events until the next `settle()`; the renderer
      // is gone right now and we'd lose the markdown context. But
      // session-end, step-mode-changed, and the sub-session
      // bracket events are pure state — dashboard subscribers
      // (toggle UI, per-step button gating) would otherwise miss
      // transitions that land between settles.
      this.queuedEvents.push(event);
      return;
    }
    this.handleEvent(event);
  }

  private flushQueuedEvents(): void {
    while (this.queuedEvents.length > 0) {
      const event = this.queuedEvents.shift();
      if (!event) {
        continue;
      }
      this.handleEvent(event);
      if (this.terminated) {
        break;
      }
    }
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
        this.bus.emit("msg", {
          type: "settled",
          result: { status: "awaiting-input" },
        } as SocketPumpBusEvent);
        break;
      case "request-llm-response":
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
        // Track the orchestrator's truth and notify subscribers (the
        // dashboard's toggle UI listens via `onStepModeChanged`). The
        // event also fires at session start as the orchestrator
        // echoes the initial `--step-mode` flag, so the toggle
        // matches reality before the user touches anything.
        this.currentStepMode = event.mode;
        this.bus.emit("msg", {
          type: "step-mode",
          mode: event.mode,
        } as SocketPumpBusEvent);
        break;
      case "sub-session-started":
        // Bracket open. Dashboard listeners disable per-step
        // buttons until the matching `sub-session-ended` arrives.
        this.inSubSessionFlag = true;
        this.bus.emit("msg", {
          type: "in-sub-session",
          inSubSession: true,
        } as SocketPumpBusEvent);
        break;
      case "sub-session-ended":
        // Bracket close. Dashboard listeners re-enable per-step
        // buttons (subject to disk-state preconditions).
        this.inSubSessionFlag = false;
        this.bus.emit("msg", {
          type: "in-sub-session",
          inSubSession: false,
        } as SocketPumpBusEvent);
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

  private readLiveLlmConfig(): {
    source: LlmSource;
    model?: string;
    ollamaBaseUrl?: string;
    lmstudioBaseUrl?: string;
    verbose: boolean;
  } {
    const config = vscode.workspace.getConfiguration("sim-flow");
    const source = (config.get<LlmSource>("llm.source") ?? this.llm.source) as LlmSource;
    const model = (config.get<string>("llm.model") ?? "").trim() || this.llm.model;
    const ollamaBaseUrl =
      (config.get<string>("llm.ollama.baseUrl") ?? "").trim() || this.llm.ollamaBaseUrl;
    const lmstudioBaseUrl =
      (config.get<string>("llm.lmstudio.baseUrl") ?? "").trim() || this.llm.lmstudioBaseUrl;
    const verbose = config.get<boolean>("llm.verbose") ?? true;
    return { source, model, ollamaBaseUrl, lmstudioBaseUrl, verbose };
  }

  private async dispatchLlm(
    event: ProtocolEvent & { event: "request-llm-response" },
  ): Promise<void> {
    const live = this.readLiveLlmConfig();
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
    let reasoningOpen = false;
    const closeReasoning = () => {
      if (reasoningOpen) {
        this.currentRenderer?.markdown("\n\n</details>\n\n");
        reasoningOpen = false;
      }
    };
    try {
      const token = new vscode.CancellationTokenSource().token;
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
    if (this.terminated || !this.socket || this.socket.destroyed) {
      return;
    }
    this.debugLog.logEventOut(event);
    const line = `${JSON.stringify(event)}\n`;
    this.debugLog.logRawOut(line);
    this.socket.write(line, "utf8");
  }

  private markTerminated(result: PumpSettleResult): void {
    if (this.terminated) {
      return;
    }
    this.terminated = true;
    this.terminationReason = result;
    this.bus.emit("msg", { type: "settled", result } as SocketPumpBusEvent);
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}
