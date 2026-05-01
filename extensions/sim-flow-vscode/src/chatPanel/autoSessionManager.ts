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

export class AutoSessionManager implements vscode.Disposable {
  private activeSession: ManagedAutoSessionState | undefined;

  constructor(private readonly workspaceState: vscode.Memento) {}

  getActiveSession(): ManagedAutoSessionState | undefined {
    return this.activeSession;
  }

  isActive(session: ManagedAutoSessionState): boolean {
    return this.activeSession === session;
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
    await this.persistRecord(session);
    session.pump.sendUserMessage(prompt);
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
    await this.persistRecord(session);
    this.startDrive(session, delegate);
    return session;
  }

  async forgetStoredRecord(projectDir: string): Promise<void> {
    if (this.activeSession?.projectDir === projectDir) {
      this.activeSession = undefined;
    }
    await this.clearRecord(projectDir);
  }

  dispose(): void {
    this.activeSession?.pump.dispose();
    this.activeSession = undefined;
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
