import * as vscode from "vscode";

import {
  type LiveSessionTransport,
  type PumpRenderer,
  type PumpSettleResult,
} from "../session/pump";
import type { LlmSourceTag } from "../webview/messages";

export interface ManagedStepRef {
  step: string;
  kind: "work" | "critique";
}

export interface ManagedAutoSessionState {
  sessionId: string;
  socketPath: string;
  projectDir: string;
  pump: LiveSessionTransport;
  awaitingInput: boolean;
  /**
   * Prompt + placeholder from the most recent `request-user-input`
   * event. Populated by the socket pump's `onRequestUserInput`
   * subscription and consumed by the chat panel host when building
   * `ChatPanelState.currentPrompt` / `currentPlaceholder`. Cleared
   * when the next sub-session opens (the user moved on) or a
   * `UserMessage` ships (we're no longer waiting).
   */
  currentPrompt: string | null;
  currentPlaceholder: string | null;
  stopRequested: boolean;
  drivePromise: Promise<void> | null;
  assistantId: string | null;
  pendingPromptEntryId: string | null;
  pendingRequestTokensEstimate: number | null;
  currentPhase: string | null;
  currentTool: string | null;
  currentArtifact: string | null;
  sourceTag: LlmSourceTag;
  model: string;
  sessionMode: "auto" | "step";
  stepRef: ManagedStepRef | null;
  launchSpecPath: string | undefined;
}

export interface StoredAutoSessionRecord {
  sessionId: string;
  socketPath: string;
  projectDir: string;
  awaitingInput: boolean;
  sourceTag: LlmSourceTag;
  model: string;
  sessionMode: "auto" | "step";
  stepRef: ManagedStepRef | null;
  launchSpecPath: string | undefined;
  updatedAtMs: number;
}

export interface AutoSessionDriveDelegate {
  markdown(session: ManagedAutoSessionState, text: string): void;
  requestTokensEstimate(session: ManagedAutoSessionState, tokens: number): void;
  settled(
    session: ManagedAutoSessionState,
    result: PumpSettleResult,
  ): Promise<void>;
}

const ACTIVE_AUTO_SESSION_KEY_PREFIX = "sim-flow.chatPanel.activeAutoSession.";

let pendingAutoSessionRecordWrites: Promise<void> = Promise.resolve();

/**
 * Notified whenever `activeSession` is set, replaced, or cleared. The
 * dashboard host uses this to attach its sub-session / step-mode bus
 * listeners the moment a pump appears, instead of waiting for the
 * next file-watcher tick or viewState change to call `refresh()` —
 * otherwise the first `sub-session-started` / `-ended` events from a
 * fresh pump can land before the listener is wired and the dashboard
 * sits at `inSubSession=true` with everything except Reset disabled.
 */
export type ActiveSessionListener = (
  session: ManagedAutoSessionState | undefined,
) => void;

export class AutoSessionManager implements vscode.Disposable {
  private activeSession: ManagedAutoSessionState | undefined;
  private readonly activeSessionListeners = new Set<ActiveSessionListener>();

  constructor(private readonly workspaceState: vscode.Memento) {}

  getActiveSession(): ManagedAutoSessionState | undefined {
    return this.activeSession;
  }

  isActive(session: ManagedAutoSessionState): boolean {
    return this.activeSession === session;
  }

  /**
   * Subscribe to active-session lifecycle changes. The listener is
   * invoked synchronously after every mutation (launch / attach /
   * clear / dispose) with the new active session (or `undefined` when
   * cleared). Returns a dispose function; the manager does not own
   * the subscriber's lifetime, so callers must dispose on teardown.
   *
   * Hand-rolled instead of `vscode.EventEmitter` so this module's
   * vitest suite can keep instantiating the manager without mocking
   * `vscode` — the only runtime reference we'd add otherwise.
   */
  onActiveSessionChanged(listener: ActiveSessionListener): () => void {
    this.activeSessionListeners.add(listener);
    return () => {
      this.activeSessionListeners.delete(listener);
    };
  }

  private notifyActiveSessionChanged(): void {
    for (const listener of this.activeSessionListeners) {
      try {
        listener(this.activeSession);
      } catch (err) {
        // Listener errors must not bubble through the mutation that
        // triggered the notify — e.g. a launch() that succeeded
        // shouldn't be reported as failed because the dashboard's
        // refresh threw. Swallow + log so the lifecycle stays clean.
        console.error("autoSessionManager: active-session listener threw", err);
      }
    }
  }

  async launch(
    options: {
      sessionId: string;
      socketPath: string;
      projectDir: string;
      pump: LiveSessionTransport;
      sourceTag: LlmSourceTag;
      model: string;
      sessionMode: "auto" | "step";
      stepRef: ManagedStepRef | null;
      launchSpecPath: string | undefined;
    },
    delegate: AutoSessionDriveDelegate,
  ): Promise<ManagedAutoSessionState> {
    const session: ManagedAutoSessionState = {
      sessionId: options.sessionId,
      socketPath: options.socketPath,
      projectDir: options.projectDir,
      pump: options.pump,
      awaitingInput: false,
      currentPrompt: null,
      currentPlaceholder: null,
      stopRequested: false,
      drivePromise: null,
      assistantId: null,
      pendingPromptEntryId: null,
      pendingRequestTokensEstimate: null,
      currentPhase: null,
      currentTool: null,
      currentArtifact: null,
      sourceTag: options.sourceTag,
      model: options.model,
      sessionMode: options.sessionMode,
      stepRef: options.stepRef,
      launchSpecPath: options.launchSpecPath,
    };
    this.activeSession = session;
    this.notifyActiveSessionChanged();
    await this.persistRecord(session);
    this.startDrive(session, delegate);
    return session;
  }

  async resumeWithPrompt(
    session: ManagedAutoSessionState,
    prompt: string,
    delegate: AutoSessionDriveDelegate,
  ): Promise<void> {
    if (session.drivePromise) {
      await session.drivePromise;
    }
    if (!this.isActive(session)) {
      return;
    }
    session.awaitingInput = false;
    // Clear the parked-prompt context the moment the user replies.
    // The orchestrator's next `request-user-input` (if any) will
    // refill these fields with fresh values.
    session.currentPrompt = null;
    session.currentPlaceholder = null;
    await this.persistRecord(session);
    session.pump.sendUserMessage(prompt);
    this.startDrive(session, delegate);
  }

  /**
   * Record the prompt / placeholder text the orchestrator embedded
   * in its most recent `request-user-input` event so the chat panel
   * can render it above the composer. Either field may be null when
   * the orchestrator parks without an explicit question (the panel
   * falls back to its generic "Waiting on user" notice).
   */
  setPendingPrompt(
    session: ManagedAutoSessionState,
    prompt: string | null,
    placeholder: string | null,
  ): void {
    if (!this.isActive(session)) {
      return;
    }
    session.currentPrompt = prompt;
    session.currentPlaceholder = placeholder;
  }

  /**
   * Re-attach a drive cycle WITHOUT sending a user message.
   *
   * Needed when the orchestrator started a new sub-session under us
   * while the chat panel was parked at "awaiting user input" -- e.g.
   * the dashboard's Run Step click went through `AutoHost`'s
   * cancel-and-dispatch path, which closed the parked critique
   * bracket and opened a fresh work-session bracket without any
   * input from the chat panel. The previous `driveSession()`
   * already returned with `awaiting-input`, so `currentRenderer` is
   * `null` and the pump silently queues the new sub-session's
   * `request-llm-response`. Calling `startDrive` here re-attaches
   * the renderer and flushes the queue so the orchestrator unblocks.
   */
  async resumeDriveOnly(
    session: ManagedAutoSessionState,
    delegate: AutoSessionDriveDelegate,
  ): Promise<void> {
    if (session.drivePromise) {
      // Already driving; nothing to do.
      return;
    }
    if (!this.isActive(session)) {
      return;
    }
    session.awaitingInput = false;
    await this.persistRecord(session);
    this.startDrive(session, delegate);
  }

  async waitForDrive(session: ManagedAutoSessionState): Promise<void> {
    if (session.drivePromise) {
      await session.drivePromise;
    }
  }

  async markAwaitingInput(session: ManagedAutoSessionState): Promise<void> {
    if (!this.isActive(session)) {
      return;
    }
    session.awaitingInput = true;
    await this.persistRecord(session);
  }

  async clearIfActive(session: ManagedAutoSessionState): Promise<void> {
    if (!this.isActive(session)) {
      return;
    }
    this.activeSession = undefined;
    this.notifyActiveSessionChanged();
    await this.clearRecord(session.projectDir);
  }

  async cancel(session: ManagedAutoSessionState): Promise<void> {
    session.awaitingInput = false;
    session.pump.cancel();
    if (this.isActive(session)) {
      await this.persistRecord(session);
    }
  }

  readStoredRecord(projectDir: string): StoredAutoSessionRecord | undefined {
    const record = this.workspaceState.get<StoredAutoSessionRecord>(recordKey(projectDir));
    if (!record) {
      return undefined;
    }
    return {
      ...record,
      sessionMode: record.sessionMode ?? "auto",
      stepRef: record.stepRef ?? null,
    };
  }

  async attach(
    record: StoredAutoSessionRecord,
    pump: LiveSessionTransport,
    delegate: AutoSessionDriveDelegate,
  ): Promise<ManagedAutoSessionState> {
    const session: ManagedAutoSessionState = {
      sessionId: record.sessionId,
      socketPath: record.socketPath,
      projectDir: record.projectDir,
      pump,
      awaitingInput: record.awaitingInput,
      currentPrompt: null,
      currentPlaceholder: null,
      stopRequested: false,
      drivePromise: null,
      assistantId: null,
      pendingPromptEntryId: null,
      pendingRequestTokensEstimate: null,
      currentPhase: null,
      currentTool: null,
      currentArtifact: null,
      sourceTag: record.sourceTag,
      model: record.model,
      sessionMode: record.sessionMode,
      stepRef: record.stepRef,
      launchSpecPath: record.launchSpecPath,
    };
    this.activeSession = session;
    this.notifyActiveSessionChanged();
    await this.persistRecord(session);
    this.startDrive(session, delegate);
    return session;
  }

  async forgetStoredRecord(projectDir: string): Promise<void> {
    if (this.activeSession?.projectDir === projectDir) {
      this.activeSession = undefined;
      this.notifyActiveSessionChanged();
    }
    await this.clearRecord(projectDir);
  }

  dispose(): void {
    this.activeSession?.pump.dispose();
    if (this.activeSession !== undefined) {
      this.activeSession = undefined;
      this.notifyActiveSessionChanged();
    }
    this.activeSessionListeners.clear();
  }

  private startDrive(
    session: ManagedAutoSessionState,
    delegate: AutoSessionDriveDelegate,
  ): void {
    const drive = this.driveSession(session, delegate).finally(() => {
      if (session.drivePromise === drive) {
        session.drivePromise = null;
      }
    });
    session.drivePromise = drive;
  }

  private async driveSession(
    session: ManagedAutoSessionState,
    delegate: AutoSessionDriveDelegate,
  ): Promise<void> {
    const renderer: PumpRenderer = {
      markdown: (text: string) => {
        delegate.markdown(session, text);
      },
      requestTokensEstimate: (tokens: number) => {
        delegate.requestTokensEstimate(session, tokens);
      },
    };
    const result = await session.pump.settle(renderer);
    if (!this.isActive(session)) {
      return;
    }
    await delegate.settled(session, result);
  }

  private async persistRecord(session: ManagedAutoSessionState): Promise<void> {
    const record: StoredAutoSessionRecord = {
      sessionId: session.sessionId,
      socketPath: session.socketPath,
      projectDir: session.projectDir,
      awaitingInput: session.awaitingInput,
      sourceTag: session.sourceTag,
      model: session.model,
      sessionMode: session.sessionMode,
      stepRef: session.stepRef,
      launchSpecPath: session.launchSpecPath,
      updatedAtMs: Date.now(),
    };
    await queueAutoSessionRecordWrite(async () => {
      await this.workspaceState.update(recordKey(session.projectDir), record);
    });
  }

  private async clearRecord(projectDir: string): Promise<void> {
    await queueAutoSessionRecordWrite(async () => {
      await this.workspaceState.update(recordKey(projectDir), undefined);
    });
  }
}

function recordKey(projectDir: string): string {
  return `${ACTIVE_AUTO_SESSION_KEY_PREFIX}${projectDir}`;
}

function queueAutoSessionRecordWrite(task: () => Promise<void>): Promise<void> {
  const write = pendingAutoSessionRecordWrites.catch(() => undefined).then(task);
  pendingAutoSessionRecordWrites = write.catch(() => undefined);
  return write;
}
