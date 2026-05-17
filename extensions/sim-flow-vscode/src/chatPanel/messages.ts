import type { Flow } from "../cli/types";
import type { LlmSourceTag, StepMode } from "../webview/messages";
import type { Finding } from "../state/types";

/**
 * Built-in palette names plus "custom" (driven by the user's
 * personal 4-colour set). The webview applies a palette by setting
 * `body[data-palette="<name>"]`; built-ins are declared in CSS,
 * custom is applied via inline CSS variables on body.
 */
export type ChatPalette = "default" | "autumn" | "olive" | "sage" | "custom";

export const CHAT_PALETTE_NAMES: readonly ChatPalette[] = [
  "default",
  "autumn",
  "olive",
  "sage",
  "custom",
];

export interface ChatCustomPalette {
  /** Brightest -- maps to `--x-palette-input` (input/current step). */
  input: string;
  /** Second brightest -- `--x-palette-tool` (tool / passed step). */
  tool: string;
  /** Third brightest -- `--x-palette-output` (output). */
  output: string;
  /** Darkest -- `--x-palette-accent` (borders / accents). */
  accent: string;
}

/** Sensible starting point when the user first picks "Custom" --
 *  mirrors the Autumn palette so they tweak from a known-good
 *  baseline instead of an all-#000 set. */
export const DEFAULT_CUSTOM_PALETTE: ChatCustomPalette = {
  input: "#add4eb",
  tool: "#fcf6cf",
  output: "#bf6f40",
  accent: "#57290f",
};

export interface ChatPanelState {
  mode: "live";
  projectLabel: string;
  projectDir: string | null;
  /**
   * Flow declared in the anchored project's `state.toml`
   * (`direct-modeling` / `design-study`). Drives the step rail's
   * canonical step list via `stepOrderFor`. Null when no project is
   * anchored.
   */
  flow: Flow | null;
  /**
   * Gate map for the anchored project (step id -> passed flag).
   * Populated from `FlowState.gates`. The step rail uses this to
   * paint completed-vs-pending tiles. Empty when no project is
   * anchored.
   */
  passedSteps: string[];
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
  /**
   * Pre-rendered Continue button label the orchestrator emitted via
   * `NextActionHint` at the most recent manual-mode park. The chat
   * panel uses this verbatim ("Run critique on DM0"). `null` means
   * the orchestrator said it has no next action available (the
   * panel renders Continue as disabled). The whole object is `null`
   * when no hint has arrived yet -- e.g. no live pump, auto mode,
   * the orchestrator hasn't parked yet, or the pump is mid-work.
   */
  nextActionHint: { label: string | null } | null;
  /**
   * Whether a sim-flow pump is currently anchored to this project's
   * chat panel. The toolbar LLM indicator uses this together with
   * `isStreaming` to render three visual states: no-session,
   * connected-and-idle, and connected-and-working.
   */
  sessionActive: boolean;
  /**
   * Milestone the orchestrator is presently working on, plus the
   * specific pending task within it. Only populated when the
   * current step drives a plan (DM2c/DM2d, DM3a-c, DM4a/DM4b) and
   * a pending task remains. Null in every other case (including
   * milestone-less steps like DM0/DM1/DM2a/DM2b). The chat panel
   * renders this as a single line under the step rail; when null,
   * the line is hidden entirely.
   */
  currentMilestone: {
    title: string;
    task: string;
    /** 1-based position of `task` within the milestone's full
     *  task-row list (counts done + deferred + pending). `null`
     *  when the task can't be located in the file. */
    taskIndex: number | null;
    /** Total task rows in the milestone. `null` when unknown. */
    taskTotal: number | null;
  } | null;
  /**
   * Whether `sim-flow.verilog.enabled` is on. When true, the chat
   * panel shows the SystemVerilog conversion rail underneath the
   * DMF rail and (when DM4b has passed) offers a "Convert to
   * SystemVerilog" continue action that flips the project into
   * the systemverilog-convert flow. Mirrors the VS Code setting
   * so changes survive panel reloads + restarts.
   */
  verilogEnabled: boolean;
  /**
   * Whether `sim-flow.chatPanel.showContextState` is on. When true,
   * transcript turns that the orchestrator has evicted from its
   * prompt stack render with a red ✗ + explanatory tooltip; when
   * false, the transcript shows the full history without any
   * eviction indicator. The transcript itself always retains every
   * turn regardless of this setting -- the indicator is purely
   * visual.
   */
  showContextState: boolean;
  /**
   * Per-message eviction record built up from `ContextEvicted`
   * events the orchestrator emits during compaction. Each entry is
   * `[messageId, reason]` where `messageId` matches the
   * `ChatTranscriptEntry.messageId` of the bubble that turn spawned.
   * The webview keys on this when `showContextState` is on to render
   * the ✗ indicator and the per-row tooltip. Empty when no
   * compaction has fired yet.
   */
  evictedMessages: Array<[string, string]>;
  /**
   * Real context window (in tokens) of the model the orchestrator
   * is dispatching to, queried at session-attach time via the
   * backend's models / show endpoint. `null` when the backend
   * doesn't expose it (Anthropic without API key plumbing, or a
   * source we haven't wired yet); the webview pie falls back to a
   * cosmetic constant for visual scaling in that case.
   */
  contextWindow: number | null;
  /**
   * Active palette name. Persisted in `workspaceState` so it
   * survives VS Code restarts (in addition to `vscode.setState`
   * for fast in-session apply).
   */
  palette: ChatPalette;
  /**
   * User's saved Custom palette colours. Always populated (seeded
   * from `DEFAULT_CUSTOM_PALETTE` on first read) so the four
   * pickers in the settings popover always have a value to bind
   * to even before the user has touched them.
   */
  customPalette: ChatCustomPalette;
}

export type ChatTranscriptEntry =
  | {
      id: string;
      kind: "note";
      title: string;
      body: string;
      tone: "info" | "error";
      /** Step id this entry was generated under. Set by the chat
       *  panel host at append time from the pump's
       *  `subSessionStep` (falling back to the orchestrator's
       *  current step from `state.toml` for entries appended
       *  between brackets). Used by the panel to group consecutive
       *  entries into a collapsible per-step section. Absent for
       *  entries that pre-date the per-step grouping feature. */
      step?: string;
    }
  | {
      id: string;
      kind: "assistant" | "user";
      title: string;
      body: string;
      meta?: string;
      /** Stable orchestrator-side message id (e.g. `msg-12`) that
       *  spawned this bubble. Used by Phase 1b to correlate
       *  `ContextEvicted` events back to the matching transcript
       *  row. Absent for bubbles that don't originate from the
       *  prompt stack (assistant turns, user prompts the user
       *  typed). */
      messageId?: string;
      requestTokensEstimate?: number;
      responseTokensEstimate?: number;
      streaming?: boolean;
      /** Assistant-only: the model's reasoning (thinking) text for
       *  this turn, captured from the orchestrator's
       *  `assistant-reasoning` event stream (which the openai-compat
       *  agent populates from vLLM's `reasoning_content` channel).
       *  Rendered as a collapsed-by-default `<details>` block above
       *  the visible answer. Absent when the turn produced no
       *  reasoning (e.g. backends without thinking mode, or
       *  non-reasoning critique turns). */
      reasoning?: string;
      /** Assistant-only: true while reasoning is still streaming;
       *  flips false on the `assistant-reasoning` final-chunk event.
       *  The collapsed block can show a small "thinking..."
       *  indicator while this is true. */
      reasoningStreaming?: boolean;
      /** Step id this entry was generated under. Set by the chat
       *  panel host at append time from the pump's
       *  `subSessionStep` (falling back to the orchestrator's
       *  current step from `state.toml` for entries appended
       *  between brackets). Used by the panel to group consecutive
       *  entries into a collapsible per-step section. Absent for
       *  entries that pre-date the per-step grouping feature. */
      step?: string;
    };

export type HostMessage =
  | { type: "state-update"; state: ChatPanelState }
  /**
   * Reply to a `pick-file` request. Carries the absolute path of
   * the file the user chose; the webview appends it to the current
   * draft. The host only posts this message when the user actually
   * selected a file (cancel + dismiss are silent).
   */
  | { type: "file-picked"; path: string }
  /**
   * Reply to an `open-critique-popup` request. Carries the parsed
   * findings for the requested step (or `null` when no critique
   * file exists on disk yet). The webview renders the popup with
   * blockers + unresolved + resolved sections; `null` shows an
   * empty-state ("No critique yet for <step>"). `step` echoes the
   * request so the webview can ignore stale replies after the user
   * clicked a different step in quick succession.
   */
  | {
      type: "critique-data";
      step: string;
      data: { findings: Finding[]; hasBlocking: boolean } | null;
    };

export type WebviewMessage =
  | { type: "ready" }
  | { type: "refresh" }
  | { type: "send-prompt"; prompt: string }
  | { type: "followup-selected"; action: string; label: string }
  | { type: "clear-transcript" }
  | { type: "stop-conversation" }
  | { type: "set-step-mode"; mode: StepMode }
  /**
   * Open the native file-picker dialog. The host responds with a
   * `file-picked` HostMessage if the user selected a file. Used by
   * the composer's Browse button so the user can drop a spec path
   * (or any other file path) into the prompt when the orchestrator
   * asks for one.
   */
  | { type: "pick-file" }
  /**
   * Continue button under the composer. The webview signals intent
   * and the host forwards `ContinueFlow` to the orchestrator, which
   * owns the manual-mode state machine and picks the next action
   * (work / critique / advance). The label on the button comes from
   * the orchestrator's `NextActionHint`.
   */
  | { type: "continue-flow" }
  /**
   * Switch the chat panel to a different sim-flow project. The
   * host shows the standard QuickPick (with a "+ New project..."
   * entry); if the user picks one, the active session is stopped
   * and a fresh session is launched against the chosen project.
   */
  | { type: "switch-project" }
  /**
   * Start a sim-flow session for this panel. If a previous
   * project is remembered and still on disk, the host launches
   * it directly; otherwise it shows the QuickPick. Sent from
   * the toolbar's "Start session" button when no session is
   * active.
   */
  | { type: "start-session" }
  /**
   * Terminate the active sim-flow session: cancel any in-flight
   * sub-session, send shutdown, escalate to SIGTERM/SIGKILL if the
   * orchestrator doesn't exit cleanly. Distinct from the composer
   * Stop button (`stop-conversation`), which only cancels the
   * current activity and drops to Manual mode without killing the
   * pump. Sent from the small power button in the toolbar.
   */
  | { type: "end-session" }
  /**
   * Reset the current step: discard its work/critique results +
   * clear its gate flag so it can be re-run from scratch. The
   * host shows a modal confirmation before any destructive action
   * lands. Sent from the Reset Step button in the composer footer.
   */
  | { type: "reset-step" }
  /**
   * Open the sim-flow dashboard for the chat panel's current
   * project. Sent from the dashboard icon in the toolbar's right
   * zone.
   */
  | { type: "open-dashboard" }
  /**
   * Open the "reset from earlier step" picker. The host shows a
   * QuickPick of previously-completed steps (gate.passed === true);
   * selecting one resets that step AND every step after it in the
   * flow's canonical order. Confirmation dialog precedes the
   * destructive action.
   */
  | { type: "reset-step-pick" }
  /**
   * Open the per-step critique popup. The host reads the latest
   * critique file for `step` and replies with a `critique-data`
   * HostMessage carrying the parsed findings (or null when no
   * critique exists yet).
   */
  | { type: "open-critique-popup"; step: string }
  /**
   * Reset a specific step plus every step after it in the flow's
   * canonical order. Sent by the rail-tile right-click context
   * menu, where the user has already picked the target step. The
   * host shows a modal confirmation listing every step that will
   * be discarded before any artifact is deleted.
   */
  | { type: "reset-from-step"; step: string }
  /**
   * Open a file mentioned in the chat transcript in a VS Code
   * editor tab. The path is whatever string the linkifier
   * detected (relative to the anchored project, or an absolute
   * path); the host resolves it and opens the doc, swallowing
   * errors so a stale path in old transcript text doesn't crash
   * the panel.
   */
  | { type: "open-file"; path: string }
  /**
   * Persist the chat panel palette + the user's custom 4-colour
   * set across VS Code restarts. Sent from the settings popover
   * whenever the dropdown or any of the four colour pickers
   * changes. The webview keeps applying the palette locally for
   * snappy feedback; this message is purely a persistence ping.
   */
  | {
      type: "set-palette";
      palette: ChatPalette;
      customPalette: ChatCustomPalette;
    }
  /**
   * Toggle `sim-flow.verilog.enabled`. The host updates the VS
   * Code workspace setting; the configuration change listener
   * already in the chat panel host refreshes the panel so the
   * SVF rail appears / disappears in response.
   */
  | { type: "set-verilog-enabled"; enabled: boolean }
  /**
   * Toggle `sim-flow.chatPanel.showContextState`. Same shape as
   * `set-verilog-enabled`: the host writes the workspace setting
   * and the configuration listener refreshes the panel so the
   * eviction indicators appear / disappear.
   */
  | { type: "set-show-context-state"; enabled: boolean }
  /**
   * Flip the anchored project from DirectModeling into the
   * SystemVerilog conversion flow at SV0. Sent from the Continue
   * button on a passed DM4b when verilog generation is enabled.
   * The host runs `sim-flow convert-sv` against the project and
   * then reconnects the pump so the orchestrator picks up the
   * post-flip state.
   */
  | { type: "convert-to-sv" };
