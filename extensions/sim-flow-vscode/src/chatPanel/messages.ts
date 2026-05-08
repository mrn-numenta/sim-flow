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
  /**
   * True when an orchestrator session is attached for this project
   * and currently parked at `request-user-input` -- i.e. not
   * streaming but not finished either, just waiting for the user's
   * next click. The bottom-of-panel status row uses this to show
   * "Waiting on user to select the next step" instead of hiding
   * entirely (which would leave the user wondering whether the
   * session was still alive).
   */
  awaitingUserInput: boolean;
  /**
   * True when the active session is a read-only viewer attached to
   * a `--watch-socket` tap. The composer is disabled, the Stop
   * button is hidden, and the streaming indicator says "VIEWING"
   * instead of "STREAMING". Events still render normally so the
   * user sees the orchestrator's progress.
   */
  isViewer: boolean;
  /**
   * Step + kind the orchestrator's most recent sub-session opened
   * for (e.g. `{ step: "DM0", kind: "work" }`). Sourced from the
   * pump's `session` getter (which reads `hello-ack`). Used by
   * the bottom status indicator to prefix the streaming label with
   * a semantic anchor (`DM0.work · Reading docs/spec.md.tmpl`)
   * instead of just naming the tool. Null when no pump is active
   * or the orchestrator hasn't opened a sub-session yet.
   */
  sessionStep: string | null;
  sessionKind: "work" | "critique" | null;
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
