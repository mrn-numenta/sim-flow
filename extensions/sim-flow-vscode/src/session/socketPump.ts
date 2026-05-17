import { spawn, type ChildProcess } from "node:child_process";
import { EventEmitter } from "node:events";
import * as net from "node:net";

import { DebugLog } from "./debug-log";
import { removePidRecord, writePidRecord } from "./processRegistry";
import { acquirePumpLock, type PumpLock } from "./pumpLock";
import {
  type LiveSessionTransport,
  type PumpLlmConfig,
  type PumpRenderer,
  type PumpSettleResult,
} from "./pump";
import type {
  Event as ProtocolEvent,
  HostEvent,
  HostInfo,
  SessionKindOut,
  SessionTag,
  StepDescriptorOut,
  StepMode,
} from "./protocol-types";
import {
  handleEvent,
  type EventDispatchContext,
  type SocketPumpBusEvent,
} from "./socketPump/events";

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
  /**
   * Read-only viewer mode for `--watch-socket` taps. When `true`:
   * - Skip pump-lock acquisition (we're not driving this run).
   * - Skip pid-record bookkeeping (no child spawned).
   * - Treat manual-mode commands (`runStep`, `runCritique`, …) and
   *   the LLM dispatch surface as no-ops; the EventTap discards
   *   observer input on the orchestrator side, but silencing them
   *   client-side keeps the dashboard / chat panel honest.
   * - Emit a single dummy `Hello`-shaped line at attach so the
   *   EventTap's `read_line` returns and registers us as an
   *   observer.
   */
  viewer?: boolean;
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
   * True after `dispose()` has run. Idempotency guard: stopAuto and
   * the chat panel can both reach `dispose` (or `disconnectWithEscalation`,
   * which calls dispose) concurrently during teardown.
   */
  private disposed = false;
  /**
   * True when the constructor's launch path bailed out (lock acquire
   * failure). `dispose()` skips the child / pid / lock cleanup in
   * that case so we don't log spurious spawn/exit pairs for a child
   * that never existed.
   */
  private neverSpawned = false;
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
  /**
   * True after `request-user-input` and before the next active-work
   * event from the orchestrator. While parked, the orchestrator
   * isn't running anything — `run_session` is blocked waiting for a
   * user message — so the sub-session bracket stays open but the
   * dashboard should treat the session as idle (buttons clickable).
   * Without this distinction, a critique that ends with a parked
   * "ask user what to do" turn never sees `sub-session-ended` and
   * the per-step buttons stay disabled until the user resumes.
   */
  private awaitingUserInputFlag = false;

  constructor(
    private readonly options: SocketSessionPumpOptions,
    private readonly llm: PumpLlmConfig,
  ) {
    this.debugLog = DebugLog.fromTokens(llm.debugTokens, llm.projectDir);
    this.attachTimeoutMs = options.attachTimeoutMs ?? 5000;
    if (options.launch) {
      // Acquire the per-project pump lock BEFORE spawning. A second
      // VS Code window with the same project open would otherwise
      // race us for the JSONL socket and `.sim-flow/state.toml`.
      const lockResult = acquirePumpLock(llm.projectDir, options.sessionId);
      if (!lockResult.ok) {
        this.neverSpawned = true;
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
      // Test/diagnostic hook: when `SIM_FLOW_PUMP_CAPTURE_STDERR` is
      // set in the parent env, pipe the child's stderr into the
      // current process so test failures can surface the
      // orchestrator's exit reason. Production uses `ignore` to
      // avoid leaking the orchestrator's tracing output into the
      // user's terminal.
      const captureStderr = baseEnv.SIM_FLOW_PUMP_CAPTURE_STDERR === "1";
      const child = spawn(options.launch.binary, options.launch.args, {
        cwd: options.launch.cwd,
        env,
        stdio: ["ignore", "ignore", captureStderr ? "inherit" : "ignore"],
      });
      this.child = child;
      this.debugLog.logProcessSpawn(options.launch.binary, options.launch.args, child.pid);
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
          this.debugLog.logSpawnError(`pid registry write failed: ${(err as Error).message}`);
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
      return (
        this.terminationReason ?? {
          status: "ended",
          endReason: "attach-failed",
          endMessage: (err as Error).message ?? String(err),
        }
      );
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
    if (this.options.viewer) {
      // Viewer pumps don't drive; the EventTap discards observer
      // input on the server side. Skip the write so we don't pollute
      // debug logs with pointless attempts.
      return;
    }
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
    if (this.options.viewer) {
      return;
    }
    void this.connectionReady
      .then(() => {
        this.sendHostEvent({ event: "cancel" });
      })
      .catch(() => {
        // ignore; caller will observe the terminal state
      });
  }

  /**
   * True when this pump is attached to a `--watch-socket` tap as a
   * read-only observer. The dashboard / chat panel use this to
   * disable command surfaces (composer, per-step buttons, Stop)
   * since the user can't drive a run that something else owns.
   */
  get isViewer(): boolean {
    return !!this.options.viewer;
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
   * Whether the orchestrator is currently busy inside a sub-session.
   * `true` between `sub-session-started` and `sub-session-ended` —
   * but only while the session is actually processing. While parked
   * on `request-user-input` the bracket is still open server-side
   * (the orchestrator hasn't returned from `run_session`), but no
   * work is happening, so the dashboard treats it as not-busy and
   * re-enables the per-step buttons. The flag flips back to true on
   * the first work event after the user resumes.
   */
  get inSubSession(): boolean {
    return this.inSubSessionFlag && !this.awaitingUserInputFlag;
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
   * Subscribe to structured gate-result events. The orchestrator
   * emits `Event::GateResult` over JSONL when a manual-mode Run Gate
   * click runs, but the dashboard previously had no way to observe
   * the result (the bracket-only listener doesn't carry the
   * payload). The dashboard host posts a `gate-result` HostMessage
   * when this fires so the per-step gate cache updates.
   */
  onGateResult(
    listener: (result: {
      step: string;
      clean: boolean;
      failures: { description: string; reason: string }[];
    }) => void,
  ): () => void {
    const wrapped = (msg: SocketPumpBusEvent) => {
      if (msg.type === "gate-result") {
        listener({
          step: msg.step,
          clean: msg.clean,
          failures: msg.failures,
        });
      }
    };
    this.bus.on("msg", wrapped);
    return () => this.bus.off("msg", wrapped);
  }

  /**
   * Subscribe to `request-user-input` payloads. Fires when the
   * orchestrator parks the sub-session asking for human guidance,
   * with the prompt + placeholder text it embedded in the event.
   * Either field can be null when the orchestrator didn't include
   * one. Chat panels render the prompt as a banner above the
   * composer; the dashboard can use it to drive contextual help.
   */
  onRequestUserInput(
    listener: (msg: { prompt: string | null; placeholder: string | null }) => void,
  ): () => void {
    const wrapped = (msg: SocketPumpBusEvent) => {
      if (msg.type === "request-user-input") {
        listener({ prompt: msg.prompt, placeholder: msg.placeholder });
      }
    };
    this.bus.on("msg", wrapped);
    return () => this.bus.off("msg", wrapped);
  }

  /**
   * Subscribe to `Followup` events. Each event carries a label
   * (button text) and an action string (the literal message the
   * host should ship back as a `UserMessage` when the user clicks).
   * Listeners typically collect these into a pending list and
   * render them as clickable chips next to the composer.
   */
  onFollowup(listener: (msg: { label: string; action: string }) => void): () => void {
    const wrapped = (msg: SocketPumpBusEvent) => {
      if (msg.type === "followup") {
        listener({ label: msg.label, action: msg.action });
      }
    };
    this.bus.on("msg", wrapped);
    return () => this.bus.off("msg", wrapped);
  }

  /**
   * Subscribe to `NextActionHint` events. The orchestrator emits one
   * each time it parks at `wait_for_command` to advertise what
   * `ContinueFlow` would do next; the chat panel uses `label` to
   * drive its Continue button text, or disables the button when
   * `label` is null.
   */
  onNextActionHint(listener: (msg: { label: string | null }) => void): () => void {
    const wrapped = (msg: SocketPumpBusEvent) => {
      if (msg.type === "next-action-hint") {
        listener({ label: msg.label });
      }
    };
    this.bus.on("msg", wrapped);
    return () => this.bus.off("msg", wrapped);
  }

  /**
   * Subscribe to context-eviction events. Each callback fires once
   * per `ContextEvicted` wire event from the orchestrator (one
   * compaction pass can emit multiple ids in a single payload).
   */
  onContextEvicted(
    listener: (msg: {
      ids: string[];
      reason: import("./protocol-types").ContextEvictionReason;
    }) => void,
  ): () => void {
    const wrapped = (msg: SocketPumpBusEvent) => {
      if (msg.type === "context-evicted") {
        listener({ ids: msg.ids, reason: msg.reason });
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
    if (this.options.viewer) return;
    this.sendHostEventAfterReady({ event: "set-step-mode", mode });
  }

  runStep(step: string, kind: SessionKindOut): void {
    if (this.options.viewer) return;
    this.sendHostEventAfterReady({ event: "run-step", step, kind });
  }

  runCritique(step: string): void {
    if (this.options.viewer) return;
    this.sendHostEventAfterReady({ event: "run-critique", step });
  }

  runGate(step: string): void {
    if (this.options.viewer) return;
    this.sendHostEventAfterReady({ event: "run-gate", step });
  }

  advance(step: string): void {
    if (this.options.viewer) return;
    this.sendHostEventAfterReady({ event: "advance", step });
  }

  reset(step: string): void {
    if (this.options.viewer) return;
    this.sendHostEventAfterReady({ event: "reset", step });
  }

  continueFlow(): void {
    if (this.options.viewer) return;
    this.sendHostEventAfterReady({ event: "continue-flow" });
  }

  shutdown(): void {
    if (this.options.viewer) return;
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
    // If the socket never actually connected (attach timeout, refused,
    // failed handshake), `shutdown()` over a dead socket is a no-op
    // and waiting `cleanTimeoutMs` for a clean exit is dead time.
    // Skip straight to SIGTERM in that case. `terminated` is set by
    // `markTerminated` on every transport-level failure, so it's a
    // reliable proxy for "socket isn't usable."
    const socketUsable = !this.terminated && this.socket !== undefined && !this.socket.destroyed;
    if (!socketUsable) {
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
      await this.waitForChildExit(1_000);
      this.dispose();
      return "sigkill";
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
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    if (this.socket && !this.socket.destroyed) {
      this.socket.destroy();
    }
    this.socket = undefined;
    // Skip the child / pid / lock cleanup when the constructor never
    // got past the lock-acquire step -- there's no child, no pid
    // record was written, and no lock was acquired by us.
    if (!this.neverSpawned) {
      // Tear down the spawned `sim-flow` child so it doesn't outlive
      // the pump. Socket close usually causes the orchestrator to
      // exit on its own (the JsonlHost / SocketHost reads return
      // None), but a stuck orchestrator (e.g. blocked in an LLM
      // call) won't notice for a while. SIGTERM is the polite kick.
      // The child's `exit` handler will clear the pid record.
      if (this.child && this.child.exitCode === null && this.child.signalCode === null) {
        try {
          this.child.kill("SIGTERM");
        } catch {
          // child already gone
        }
      }
      this.child = null;
      // Defensive: if the exit handler never fires (e.g. test
      // harnesses that mock spawn), reap the pid record here too.
      this.clearPidRecord();
      if (this.pumpLock) {
        try {
          this.pumpLock.release();
        } catch {
          // Already logged; never throw out of dispose.
        }
        this.pumpLock = null;
      }
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
      capabilities: [
        "text",
        "markdown",
        "user-input",
        "tool-notifications",
        "followups",
      ],
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
      this.currentRenderer?.markdown(
        `\n**Error**: protocol: bad JSON from sim-flow: ${(err as Error).message}\n`,
      );
      return;
    }
    this.debugLog.logEventIn(event);
    if (
      this.currentRenderer === null &&
      event.event !== "session-end" &&
      event.event !== "step-mode-changed" &&
      event.event !== "sub-session-started" &&
      event.event !== "sub-session-ended" &&
      event.event !== "gate-result"
    ) {
      // Defer most events until the next `settle()`; the renderer
      // is gone right now and we'd lose the markdown context. But
      // session-end, step-mode-changed, the sub-session bracket
      // events, AND structured `gate-result` are pure state /
      // control-channel events -- dashboard subscribers (toggle
      // UI, per-step button gating, gate cache) would otherwise
      // miss transitions that land between settles. `gate-result`
      // specifically is the manual-mode Run Gate response: the
      // orchestrator handles RunGate OUTSIDE a sub-session
      // bracket, so when the user clicks Run Gate while the chat
      // panel is parked at `awaiting-input`, the result arrives
      // with `currentRenderer === null`. Without this bypass the
      // event would queue forever, the gate cache would never
      // update, and the Advance button would never enable.
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
    // The dispatch context interface mirrors this class's private
    // fields exactly; the cast bridges TS's nominal privacy check
    // while preserving structural compatibility at runtime.
    handleEvent(this as unknown as EventDispatchContext, event);
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
