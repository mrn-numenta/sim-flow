import * as vscode from "vscode";

import { type PumpSettleResult, type PumpRenderer, SessionPump } from "../session/pump";
import type { LlmSourceTag } from "../webview/messages";

export interface ManagedAutoSessionState {
  projectDir: string;
  pump: SessionPump;
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
  launchSpecPath: string | undefined;
}

export interface StoredAutoSessionRecord {
  projectDir: string;
  awaitingInput: boolean;
  sourceTag: LlmSourceTag;
  model: string;
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
      projectDir: string;
      pump: SessionPump;
      sourceTag: LlmSourceTag;
      model: string;
      launchSpecPath: string | undefined;
    },
    delegate: AutoSessionDriveDelegate,
  ): Promise<ManagedAutoSessionState> {
    const session: ManagedAutoSessionState = {
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
    return this.workspaceState.get<StoredAutoSessionRecord>(recordKey(projectDir));
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
      projectDir: session.projectDir,
      awaitingInput: session.awaitingInput,
      sourceTag: session.sourceTag,
      model: session.model,
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
