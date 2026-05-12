import type {
  ChatPanelState,
  ChatTranscriptEntry,
  HostMessage,
  WebviewMessage,
} from "./messages";
import { renderMarkdownFragment } from "./renderMarkdown";
import { stripProtocolFences, stripToolCallFencesForDisplay } from "./state";

declare function acquireVsCodeApi(): {
  postMessage(message: WebviewMessage): void;
  setState(state: unknown): void;
  getState<T = unknown>(): T | undefined;
};

const vscode = acquireVsCodeApi();

interface UiState {
  state: ChatPanelState | null;
  draft: string;
  transcriptPinnedToBottom: boolean;
  transcriptScrollTop: number;
}

interface PersistedState {
  draft?: string;
}

const persisted = vscode.getState<PersistedState>();
const ui: UiState = {
  state: null,
  draft: persisted?.draft ?? "",
  transcriptPinnedToBottom: true,
  transcriptScrollTop: 0,
};

window.addEventListener("message", (event) => {
  const msg = event.data as HostMessage;
  if (!msg || typeof msg.type !== "string") {
    return;
  }
  switch (msg.type) {
    case "state-update":
      ui.state = msg.state;
      render();
      return;
  }
});

function send(message: WebviewMessage): void {
  vscode.postMessage(message);
}

function persist(): void {
  vscode.setState({ draft: ui.draft });
}

function render(): void {
  const app = document.getElementById("app");
  if (!app) {
    return;
  }

  const previousTranscript = app.querySelector<HTMLElement>(".transcript");
  if (previousTranscript) {
    ui.transcriptPinnedToBottom = isNearBottom(previousTranscript);
    ui.transcriptScrollTop = previousTranscript.scrollTop;
  }

  if (!ui.state) {
    app.replaceChildren(
      section(
        "chat-shell loading",
        titleBlock("sim-flow Chat", "Preparing chat panel..."),
      ),
    );
    return;
  }

  // Keep the scroll container stable across streamed state updates.
  // Replacing `.transcript` on every chunk tears down an in-flight
  // scrollbar drag / wheel gesture, which makes the panel feel like
  // it "won't scroll" while a response is streaming.
  let root = app.querySelector<HTMLElement>(".chat-shell");
  if (!root || root.classList.contains("loading")) {
    root = section("chat-shell");
    app.replaceChildren(root);
  }

  const transcriptRoot = ensureTranscriptRoot(root);
  renderTranscript(transcriptRoot, ui.state.transcript);
  syncHero(root, header(ui.state));
  syncIndicator(root, bottomStatusIndicator(ui.state));
  syncComposer(root, composer(ui.state));
}

/**
 * Bottom-of-panel status row. Visible whenever there's an active
 * orchestrator session for this project, with three modes:
 *
 * - Streaming: animated dots + "Tool / Writing / Generating"
 *   label so the user sees what the LLM is busy with.
 * - Awaiting input: static dot + "Waiting on user to select the
 *   next step." This is the case the user kept missing -- session
 *   parked for the next dashboard click, header pill stale, no
 *   streaming animation.
 * - Otherwise (no live session): hidden.
 *
 * The header's STREAMING pill is easy to miss when the user has
 * scrolled mid-transcript or is focused on the composer; the
 * bottom row sits directly above the textarea so the active /
 * idle distinction is impossible to overlook.
 */
function bottomStatusIndicator(state: ChatPanelState): HTMLElement | null {
  if (state.isViewer) {
    if (state.isStreaming) {
      return renderIndicator("active", `VIEWING — ${streamingLabel(state)}`);
    }
    return renderIndicator(
      "parked",
      "VIEWING — read-only attach to another host's run. Detach to reclaim the chat.",
    );
  }
  if (state.isStreaming) {
    return renderIndicator("active", streamingLabel(state));
  }
  if (state.awaitingUserInput) {
    return renderIndicator(
      "parked",
      "Waiting on user to select the next step.",
    );
  }
  return null;
}

function streamingLabel(state: ChatPanelState): string {
  // Prefix with the step + kind the orchestrator is in, when
  // known. The status row sits below a long transcript, so the
  // semantic anchor ("DM0.work") makes the action ("Reading
  // docs/spec.md.tmpl") immediately situated even when the user
  // has scrolled away from the session banner. Falls back to no
  // prefix when the pump hasn't opened a sub-session yet.
  const stepPrefix =
    state.sessionStep && state.sessionKind
      ? `${state.sessionStep}.${state.sessionKind} · `
      : "";
  const phase = state.currentPhase ? ` (${state.currentPhase})` : "";
  if (state.currentTool) {
    return `${stepPrefix}${humanizeToolAction(state.currentTool)}${phase}`;
  }
  if (state.currentArtifact) {
    return `${stepPrefix}Writing ${state.currentArtifact}${phase}`;
  }
  // Between tools: the LLM is generating. Make the semantic
  // anchor still visible so the user knows what step is in
  // flight even when nothing else is.
  if (stepPrefix) {
    return `${stepPrefix}Generating response${phase}`;
  }
  return `Generating response from ${state.model || state.sourceLabel}${phase}`;
}

/**
 * Translate the pump's `_Tool \`name\` (args_summary) -> ok ..._`
 * markdown into a human-readable action: `read_file (docs/spec.md.tmpl)`
 * becomes `Reading docs/spec.md.tmpl`. Unknown tool names fall
 * through as `Tool: <raw>` so the indicator never lies about
 * what's running.
 */
function humanizeToolAction(toolSummary: string): string {
  // toolSummary is the verbatim chunk from `classifyPumpMarkdown`'s
  // `tool-activity` branch -- shape: `<name> (<args>)` (no
  // duration / status tail; that's stripped upstream).
  const m = /^([a-zA-Z_][a-zA-Z0-9_]*)\s*(?:\(([^)]*)\))?/.exec(toolSummary);
  if (!m) {
    return `Tool: ${toolSummary}`;
  }
  const name = m[1];
  const args = (m[2] ?? "").trim();
  switch (name) {
    case "read_file":
      return args ? `Reading ${args}` : "Reading file";
    case "write_file":
      return args ? `Writing ${args}` : "Writing file";
    case "edit_file":
      return args ? `Editing ${args}` : "Editing file";
    case "list_dir":
      return args ? `Listing ${args}` : "Listing directory";
    case "search":
      return args ? `Searching ${args}` : "Searching";
    case "run_cargo":
      return args ? `Running cargo ${args}` : "Running cargo";
    default:
      return `Tool: ${toolSummary}`;
  }
}

function renderIndicator(
  mode: "active" | "parked",
  text: string,
): HTMLElement {
  const root = section(`streaming-indicator streaming-indicator-${mode}`);
  const dots = section("streaming-indicator-dots");
  // Three dots in active mode (animated via CSS); a single static
  // dot in parked mode reads visually as "idle / waiting".
  const dotCount = mode === "active" ? 3 : 1;
  for (let i = 0; i < dotCount; i++) {
    dots.appendChild(el("span", { class: "streaming-indicator-dot" }));
  }
  root.append(dots, el("div", { class: "streaming-indicator-text" }, text));
  return root;
}

function header(state: ChatPanelState): HTMLElement {
  const root = section("hero");
  const titleRow = section("hero-row");
  titleRow.append(
    section(
      "hero-main",
      el("h1", { class: "title" }, "sim-flow Chat"),
      el("p", { class: "subtitle" }, state.statusLine),
    ),
    section(
      "hero-actions",
      statusPill(state),
      actionButton("Stop", state.canStop, () => {
        send({ type: "stop-conversation" });
      }, "hero-stop"),
    ),
  );

  const meta = section("hero-meta");
  meta.append(
    metaPill("Project", state.projectLabel),
    metaPill("Session", state.currentStep ?? state.sessionLabel),
    metaPill("Phase", state.currentPhase ?? "Idle"),
    ...(state.currentTool ? [metaPill("Tool", state.currentTool)] : []),
    ...(state.currentArtifact ? [metaPill("Writing", state.currentArtifact)] : []),
    metaPill("Model", state.model || state.sourceLabel),
    metaPill(
      "Tokens",
      `Up ${formatTokenCount(state.totalInputTokensEstimate)} • Down ${formatTokenCount(state.totalOutputTokensEstimate)}`,
    ),
  );

  root.append(titleRow, meta);
  if (state.notice.trim().length > 0) {
    root.appendChild(el("div", { class: "hero-notice" }, state.notice));
  }
  // Orchestrator-supplied prompt (carried by `request-user-input`).
  // Renders BELOW the generic notice so the user sees both:
  // - notice: "sim-flow is waiting for your next reply"
  // - prompt: the actual question or instruction
  // Wrapped in a dedicated `.hero-prompt` block so styling can
  // distinguish the orchestrator's literal text from our boilerplate.
  if (state.currentPrompt && state.currentPrompt.trim().length > 0) {
    root.appendChild(
      el(
        "div",
        { class: "hero-prompt", role: "status", "aria-live": "polite" },
        state.currentPrompt,
      ),
    );
  }
  return root;
}

function ensureTranscriptRoot(shell: HTMLElement): HTMLElement {
  const existing = shell.querySelector<HTMLElement>(".transcript");
  if (existing) {
    return existing;
  }
  const root = section("transcript");
  root.addEventListener("scroll", () => {
    ui.transcriptPinnedToBottom = isNearBottom(root);
    ui.transcriptScrollTop = root.scrollTop;
  });
  const heroNode = shell.querySelector<HTMLElement>(".hero");
  if (heroNode?.nextSibling) {
    shell.insertBefore(root, heroNode.nextSibling);
  } else {
    shell.appendChild(root);
  }
  return root;
}

function renderTranscript(root: HTMLElement, entries: ChatTranscriptEntry[]): void {
  root.replaceChildren();
  if (entries.length === 0) {
    root.appendChild(
      section(
        "entry entry-note",
        el("div", { class: "entry-header" }, "No messages yet"),
        el(
          "div",
          { class: "entry-body" },
          "Send a prompt below to start a direct conversation with the configured backend.",
        ),
      ),
    );
    return;
  }
  for (const entry of entries) {
    switch (entry.kind) {
      case "note":
        root.appendChild(noteEntry(entry));
        break;
      case "assistant": {
        const body = renderableAssistantBody(entry);
        if (entry.kind === "assistant" && body.length === 0) {
          break;
        }
        root.appendChild(
          section(
            `entry entry-${entry.kind}${entry.streaming ? " entry-streaming" : ""}`,
            messageHeader(
              entry.title,
              entryMeta(entry),
            ),
            markdownBody(body, false),
          ),
        );
        break;
      }
      case "user":
        root.appendChild(
          section(
            `entry entry-${entry.kind}${entry.streaming ? " entry-streaming" : ""}`,
            messageHeader(
              entry.title,
              entryMeta(entry),
            ),
            markdownBody(entry.body, false),
          ),
        );
        break;
    }
  }
  queueMicrotask(() => {
    if (ui.transcriptPinnedToBottom) {
      root.scrollTop = root.scrollHeight;
      return;
    }
    const maxScrollTop = Math.max(0, root.scrollHeight - root.clientHeight);
    root.scrollTop = Math.min(ui.transcriptScrollTop, maxScrollTop);
  });
}

function syncHero(shell: HTMLElement, next: HTMLElement): void {
  const existing = shell.querySelector<HTMLElement>(".hero");
  if (existing) {
    existing.replaceWith(next);
    return;
  }
  shell.insertBefore(next, shell.firstChild);
}

function syncIndicator(shell: HTMLElement, next: HTMLElement | null): void {
  const existing = shell.querySelector<HTMLElement>(".streaming-indicator");
  if (!next) {
    existing?.remove();
    return;
  }
  const composerNode = shell.querySelector<HTMLElement>(".composer");
  if (existing) {
    existing.replaceWith(next);
    return;
  }
  if (composerNode) {
    shell.insertBefore(next, composerNode);
    return;
  }
  shell.appendChild(next);
}

function syncComposer(shell: HTMLElement, next: HTMLElement): void {
  const existing = shell.querySelector<HTMLElement>(".composer");
  if (existing) {
    existing.replaceWith(next);
    return;
  }
  shell.appendChild(next);
}

function composer(state: ChatPanelState): HTMLElement {
  const root = section("composer");
  const interruptedSession = isInterruptedSessionRestore(state);
  const label = section(
    "composer-header",
    el("div", { class: "composer-title" }, "Message"),
    el(
      "div",
      { class: "composer-subtitle" },
      state.isViewer
        ? "Viewing a run driven by another host (read-only)."
        : state.supportsPromptEntry
          ? `Target: ${state.sessionLabel}`
          : interruptedSession
            ? "Relaunch the flow or clear the transcript to start a fresh direct chat."
            : "Switch to an API backend to enable direct chat here.",
    ),
  );

  const area = document.createElement("textarea");
  area.className = "composer-input";
  // Orchestrator-supplied placeholder (paired with currentPrompt)
  // wins over our generic ones when present -- the orchestrator
  // knows what shape of reply it expects (e.g. "/retry,
  // /end-session, or a course-correction message").
  area.placeholder = state.isViewer
    ? "Read-only viewer — input disabled. Detach to reclaim the chat."
    : state.currentPlaceholder && state.currentPlaceholder.trim().length > 0
      ? state.currentPlaceholder
      : state.supportsPromptEntry
        ? "Ask a question about the current project, request a code change, or continue the conversation here."
        : interruptedSession
          ? "This restored flow session is no longer live."
          : "This backend runs in a terminal, not in the panel chat.";
  area.value = ui.draft;
  area.disabled = state.isViewer || !state.supportsPromptEntry || state.isStreaming;
  area.addEventListener("input", () => {
    ui.draft = area.value;
    persist();
  });
  area.addEventListener("keydown", (event) => {
    const wantsSend = event.key === "Enter" && (event.metaKey || event.ctrlKey);
    if (!wantsSend || !canSend(state)) {
      return;
    }
    event.preventDefault();
    submitPrompt();
  });

  const footer = section("composer-footer");
  const actions = section("composer-actions");
  // Refresh stays useful in viewer mode (force a re-fetch of the
  // observed state). Stop / Send / Clear Chat affect the host's
  // run, which a viewer doesn't own -- hide them rather than
  // showing them disabled, since "disabled" implies "could be
  // enabled later" and a viewer can't drive without detaching.
  actions.append(
    actionButton("Refresh", true, () => {
      send({ type: "refresh" });
    }),
  );
  if (!state.isViewer) {
    actions.append(
      actionButton("Clear Chat", !state.isStreaming, () => {
        send({ type: "clear-transcript" });
      }),
      actionButton("Stop", state.canStop, () => {
        send({ type: "stop-conversation" });
      }),
      actionButton("Send", canSend(state), () => {
        submitPrompt();
      }),
    );
  }
  footer.append(
    el(
      "div",
      { class: "composer-hint" },
      state.isViewer
        ? "Viewer mode — input disabled"
        : state.supportsPromptEntry
          ? "Ctrl+Enter to send"
          : "Panel chat supports API backends only",
    ),
    actions,
  );

  // Followup quick-action chips. Surface them between the textarea
  // and the footer so the user sees them as a peer to typing (they
  // are alternative ways to send the same UserMessage).
  let followups: HTMLElement | null = null;
  if (state.pendingFollowups && state.pendingFollowups.length > 0 && !state.isViewer) {
    followups = section("composer-followups");
    for (const f of state.pendingFollowups) {
      const chip = document.createElement("button");
      chip.type = "button";
      chip.className = "followup-chip";
      chip.textContent = f.label;
      chip.title = f.action;
      chip.disabled = state.isStreaming;
      chip.addEventListener("click", () => {
        send({ type: "followup-selected", action: f.action, label: f.label });
      });
      followups.appendChild(chip);
    }
  }

  root.append(label, area);
  if (followups) {
    root.append(followups);
  }
  root.append(footer);
  return root;
}

function actionButton(
  label: string,
  enabled: boolean,
  handler: () => void,
  className?: string,
): HTMLButtonElement {
  const button = document.createElement("button");
  button.className = className ? `composer-button ${className}` : "composer-button";
  button.type = "button";
  button.textContent = label;
  button.disabled = !enabled;
  button.addEventListener("click", () => {
    if (!enabled) {
      return;
    }
    handler();
  });
  return button;
}

function submitPrompt(): void {
  const prompt = ui.draft.trim();
  if (prompt.length === 0) {
    return;
  }
  ui.transcriptPinnedToBottom = true;
  ui.draft = "";
  persist();
  send({ type: "send-prompt", prompt });
  render();
}

function isNearBottom(node: HTMLElement): boolean {
  const thresholdPx = 16;
  return node.scrollHeight - node.scrollTop - node.clientHeight <= thresholdPx;
}

function canSend(state: ChatPanelState): boolean {
  return state.supportsPromptEntry && !state.isStreaming && ui.draft.trim().length > 0;
}

function isInterruptedSessionRestore(state: ChatPanelState): boolean {
  return state.notice.includes("Relaunch the flow from the dashboard");
}

function titleBlock(title: string, subtitle: string): HTMLElement {
  return section(
    "title-block",
    el("h1", { class: "title" }, title),
    el("p", { class: "subtitle" }, subtitle),
  );
}

function messageHeader(title: string, meta?: string): HTMLElement {
  const root = section("entry-header-row");
  root.append(el("div", { class: "entry-header" }, title));
  if (meta) {
    root.appendChild(el("div", { class: "entry-meta" }, meta));
  }
  return root;
}

function noteEntry(
  entry: Extract<ChatTranscriptEntry, { kind: "note" }>,
): HTMLElement {
  return section(
    `entry entry-note entry-note-compact ${entry.tone === "error" ? "entry-note-error" : ""}`,
    el("div", { class: "entry-header" }, entry.title),
    markdownBody(entry.body, false),
  );
}

function entryMeta(
  entry: Extract<ChatTranscriptEntry, { kind: "assistant" | "user" }>,
): string | undefined {
  const parts: string[] = [];
  if (entry.meta) {
    parts.push(entry.meta);
  }
  if (entry.requestTokensEstimate && entry.requestTokensEstimate > 0) {
    parts.push(`up ${formatTokenCount(entry.requestTokensEstimate)}`);
  }
  if (entry.responseTokensEstimate && entry.responseTokensEstimate > 0) {
    parts.push(`down ${formatTokenCount(entry.responseTokensEstimate)}`);
  }
  if (entry.streaming) {
    parts.push("streaming");
  }
  return parts.length > 0 ? parts.join(" • ") : undefined;
}

function markdownBody(text: string, allowToolStripping: boolean): HTMLElement {
  const root = document.createElement("div");
  root.className = "entry-body";
  const rendered = allowToolStripping ? stripToolCallFencesForDisplay(text) : text;
  if (!looksLikeMarkdown(rendered)) {
    root.classList.add("entry-body-plain");
    root.textContent = rendered;
    return root;
  }
  root.appendChild(renderMarkdownFragment(rendered));
  return root;
}

function looksLikeMarkdown(text: string): boolean {
  return /(^|\n)(#{1,6}\s|[-*]\s|\d+\.\s|>\s|```|\|.+\||\*\*|__|`|\[.+\]\(.+\))/.test(text);
}

function metaPill(label: string, value: string): HTMLElement {
  return section(
    "meta-pill",
    el("span", { class: "meta-pill-label" }, `${label}:`),
    el("span", { class: "meta-pill-value" }, value),
  );
}

function pill(text: string, variant: string): HTMLElement {
  return el("span", { class: `pill pill-${variant}` }, text);
}

function statusPill(state: ChatPanelState): HTMLElement {
  // Viewer attach takes precedence: even with no messages yet the
  // user has explicitly opted into observing a run. Without this
  // branch the pill rendered OFFLINE during the gap between attach
  // and the first event from the orchestrator, masking the live
  // (read-only) connection.
  if (state.isViewer) {
    return pill("VIEWING", "live");
  }
  if (state.isStreaming) {
    return pill("STREAMING", "streaming");
  }
  if (state.canStop) {
    return pill("LIVE", "live");
  }
  return pill("OFFLINE", "offline");
}

function formatTokenCount(tokens: number): string {
  if (tokens >= 1000) {
    return `~${(tokens / 1000).toFixed(tokens >= 10000 ? 0 : 1)}k tok`;
  }
  return `~${tokens} tok`;
}

function renderableAssistantBody(
  entry: ChatTranscriptEntry,
): string {
  if (entry.kind !== "assistant") {
    return "";
  }
  return entry.meta === "orchestrator"
    ? stripProtocolFences(entry.body)
    : stripToolCallFencesForDisplay(entry.body);
}

function section(className: string, ...children: Node[]): HTMLElement {
  return el("section", { class: className }, ...children);
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Record<string, string>,
  ...children: Array<Node | string>
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    node.setAttribute(key, value);
  }
  for (const child of children) {
    node.append(child);
  }
  return node;
}

send({ type: "ready" });
render();
