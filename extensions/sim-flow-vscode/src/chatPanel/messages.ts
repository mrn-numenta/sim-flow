import type { LlmSourceTag } from "../webview/messages";

export interface ChatPanelState {
  mode: "live";
  projectLabel: string;
  projectDir: string | null;
  currentStep: string | null;
  currentPhase: string | null;
  currentTool: string | null;
  currentArtifact: string | null;
  source: LlmSourceTag;
  sourceLabel: string;
  model: string;
  verbose: boolean;
  sessionLabel: string;
  statusLine: string;
  notice: string;
  totalInputTokensEstimate: number;
  totalOutputTokensEstimate: number;
  transcript: ChatTranscriptEntry[];
  isStreaming: boolean;
  supportsPromptEntry: boolean;
  canStop: boolean;
}

export type ChatTranscriptEntry =
  | {
      id: string;
      kind: "note";
      title: string;
      body: string;
      tone: "info" | "error";
    }
  | {
      id: string;
      kind: "assistant" | "user";
      title: string;
      body: string;
      meta?: string;
      requestTokensEstimate?: number;
      responseTokensEstimate?: number;
      streaming?: boolean;
    };

export type HostMessage = { type: "state-update"; state: ChatPanelState };

export type WebviewMessage =
  | { type: "ready" }
  | { type: "refresh" }
  | { type: "send-prompt"; prompt: string }
  | { type: "clear-transcript" }
  | { type: "stop-conversation" };
