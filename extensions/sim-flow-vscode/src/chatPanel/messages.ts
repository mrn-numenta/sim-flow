import type { LlmSourceTag, StepMode } from "../webview/messages";

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
   * Free-form prompt the orchestrator most recently attached to
   * its `request-user-input` event, if any. When the agent is
   * parked waiting for human guidance (e.g. DM0 asking a spec
   * clarification question, a refused-advance asking the operator
   * to inspect diagnostics, an `LlmError` offering `/retry` vs
   * `/end-session`) the orchestrator embeds the *what to do next*
   * text in this field. Surfaced verbatim as a banner above the
   * composer so the user knows what's being asked. Cleared when
   * the next sub-session opens or a `UserMessage` ships.
   */
  currentPrompt: string | null;
  /**
   * Placeholder hint the orchestrator suggested for the composer
   * textarea (paired with `currentPrompt`). Renders inside the
   * `<textarea>` while empty so the user sees the expected reply
   * shape without having to read the longer prompt above it.
   */
  currentPlaceholder: string | null;
  /**
   * Followup quick-actions the orchestrator emitted near the most
   * recent `request-user-input`. The label is the button text; the
   * action is the literal string we ship back as a `UserMessage`
   * when the user clicks (e.g. `/retry`, `/end-session`). Cleared
   * when the user sends a message (whether from the textarea or by
   * clicking a chip) and when the next sub-session opens.
   */
  pendingFollowups: Array<{ label: string; action: string }>;
  /**
   * Helper text rendered above the composer when the user is in
   * idle-state Q&A (manual mode, no active sub-session, not parked
   * at request-user-input). The orchestrator interprets any
   * UserMessage typed during this window as a Q&A turn (a
   * side-conversation LLM round-trip against the project context).
   * Cleared while a sub-session is in flight or while the panel
   * isn't anchored to a live pump.
   */
  idleQaHint: string | null;
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
  sessionKind: "work" | "critique" | "qa" | null;
  supportsPromptEntry: boolean;
  canStop: boolean;
  /**
   * Current orchestrator step mode for the active pump, if any.
   * `auto` means the orchestrator walks through sub-sessions
   * without pausing; `manual` means it parks between sub-sessions
   * for the user to advance. Wired to the pump's
   * `onStepModeChanged` event; null when no pump is anchored to
   * this project or the orchestrator hasn't emitted its initial
   * `StepModeChanged` echo yet.
   */
  currentStepMode: StepMode | null;
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
  | { type: "followup-selected"; action: string; label: string }
  | { type: "clear-transcript" }
  | { type: "stop-conversation" }
  | { type: "set-step-mode"; mode: StepMode };
