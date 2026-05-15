// Experimental chat panel webview. Selected when
// `sim-flow.dashboard.experimentalUi` is enabled. Renders a plain
// interleaved transcript with a textarea pinned to the bottom -- the
// shape most chat apps use.
//
// Streaming updates patch the existing DOM via morphdom instead of a
// full `replaceChildren()` tear-down: that preserves scroll position,
// hover state, focus on the composer textarea, and any in-flight
// browser selection across state pushes -- otherwise every assistant
// chunk reset all of those mid-stream.

import morphdom from "morphdom";

import type {
  ChatPanelState,
  ChatTranscriptEntry,
  HostMessage,
  WebviewMessage,
} from "./messages";
import { initShiki, renderMarkdownFragment } from "./renderMarkdown";
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
  pinnedToBottom: boolean;
  scrollTop: number;
}

interface PersistedState {
  draft?: string;
}

const persisted = vscode.getState<PersistedState>();
const ui: UiState = {
  state: null,
  draft: persisted?.draft ?? "",
  pinnedToBottom: true,
  scrollTop: 0,
};

window.addEventListener("message", (event) => {
  const msg = event.data as HostMessage;
  if (!msg || typeof msg.type !== "string") {
    return;
  }
  if (msg.type === "state-update") {
    ui.state = msg.state;
    render();
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

  const previousTranscript = app.querySelector<HTMLElement>(".x-transcript");
  if (previousTranscript) {
    ui.pinnedToBottom = isNearBottom(previousTranscript);
    ui.scrollTop = previousTranscript.scrollTop;
  }

  // Build the next tree off-screen, then morphdom-patch it onto the
  // live tree. Mirrors webview/panelExperimental.ts's pattern.
  const next = document.createElement("main");
  next.id = "app";
  for (const node of buildShell()) {
    next.appendChild(node);
  }

  morphdom(app, next, {
    onBeforeElUpdated(fromEl, toEl) {
      // Skip nodes that are already structurally identical -- avoids
      // a wasted attribute / text-node sweep for every unchanged
      // bubble during a streaming update.
      if (fromEl.isEqualNode(toEl)) {
        return false;
      }
      // Don't clobber the composer textarea while it's focused: the
      // user may be mid-typing and morphdom assigning `value` would
      // collapse the IME composition or move the cursor.
      if (
        fromEl instanceof HTMLTextAreaElement &&
        fromEl === document.activeElement
      ) {
        return false;
      }
      return true;
    },
  });

  const transcriptRoot = app.querySelector<HTMLElement>(".x-transcript");
  if (transcriptRoot) {
    // Re-attach the scroll listener if morphdom installed a fresh node.
    // The data-scroll-bound flag guards against attaching twice.
    if (!transcriptRoot.dataset.scrollBound) {
      transcriptRoot.dataset.scrollBound = "1";
      transcriptRoot.addEventListener("scroll", () => {
        ui.pinnedToBottom = isNearBottom(transcriptRoot);
        ui.scrollTop = transcriptRoot.scrollTop;
      });
    }
    queueMicrotask(() => {
      if (ui.pinnedToBottom) {
        transcriptRoot.scrollTop = transcriptRoot.scrollHeight;
        return;
      }
      const maxScrollTop = Math.max(
        0,
        transcriptRoot.scrollHeight - transcriptRoot.clientHeight,
      );
      transcriptRoot.scrollTop = Math.min(ui.scrollTop, maxScrollTop);
    });
  }
}

function buildShell(): Node[] {
  if (!ui.state) {
    return [
      div(
        "x-shell x-loading",
        div("x-loading-text", "Preparing chat panel..."),
      ),
    ];
  }
  const shell = div("x-shell");
  shell.appendChild(buildTranscript(ui.state));
  shell.appendChild(buildComposer(ui.state));
  return [shell];
}

function buildTranscript(state: ChatPanelState): HTMLElement {
  const root = div("x-transcript");

  if (state.transcript.length === 0 && !state.isStreaming) {
    root.appendChild(
      div("x-empty", "No messages yet. Type below to start a conversation."),
    );
    return root;
  }

  for (const entry of state.transcript) {
    if (entry.kind === "note") {
      root.appendChild(noteRow(entry));
      continue;
    }
    const body = renderableBody(entry);
    // Skip empty non-streaming assistant entries -- those are stale
    // placeholders from a turn that never produced visible text.
    if (entry.kind === "assistant" && body.length === 0 && !entry.streaming) {
      continue;
    }
    root.appendChild(messageBubble(entry, body));
  }

  // If the orchestrator says streaming but the latest assistant entry
  // hasn't materialised yet (between Generate Work and the first chunk,
  // or during tool-call stretches), synthesize a thinking bubble.
  if (state.isStreaming && !hasStreamingAssistantTail(state.transcript)) {
    root.appendChild(thinkingBubble());
  }

  return root;
}

function hasStreamingAssistantTail(entries: ChatTranscriptEntry[]): boolean {
  for (let i = entries.length - 1; i >= 0; i--) {
    const entry = entries[i];
    if (entry.kind === "note") {
      continue;
    }
    return entry.kind === "assistant" && entry.streaming === true;
  }
  return false;
}

function noteRow(
  entry: Extract<ChatTranscriptEntry, { kind: "note" }>,
): HTMLElement {
  const row = div(`x-note${entry.tone === "error" ? " x-note-error" : ""}`);
  row.id = `entry-${entry.id}`;
  if (entry.title) {
    row.appendChild(div("x-note-title", entry.title));
  }
  if (entry.body && entry.body.trim().length > 0) {
    row.appendChild(div("x-note-body", entry.body));
  }
  return row;
}

function renderableBody(
  entry: Extract<ChatTranscriptEntry, { kind: "user" | "assistant" }>,
): string {
  if (entry.kind === "user") {
    return entry.body;
  }
  return entry.meta === "orchestrator"
    ? stripProtocolFences(entry.body)
    : stripToolCallFencesForDisplay(entry.body);
}

function messageBubble(
  entry: Extract<ChatTranscriptEntry, { kind: "user" | "assistant" }>,
  body: string,
): HTMLElement {
  const role = entry.kind === "user" ? "user" : "assistant";
  const orchestrator =
    entry.kind === "user" &&
    typeof entry.meta === "string" &&
    entry.meta.startsWith("orchestrator-");
  const row = div(
    `x-row x-row-${role}${orchestrator ? " x-row-orchestrator" : ""}`,
  );
  // Stable id so morphdom keeps DOM identity across renders -- this is
  // what makes streaming chunks patch in place instead of rebuilding.
  row.id = `entry-${entry.id}`;
  const bubble = div(
    `x-bubble x-bubble-${role}${orchestrator ? " x-bubble-orchestrator" : ""}`,
  );
  if (body.length === 0 && entry.streaming) {
    bubble.appendChild(thinkingDots());
  } else {
    bubble.appendChild(markdownBody(body));
  }
  row.appendChild(bubble);
  return row;
}

function thinkingBubble(): HTMLElement {
  const row = div("x-row x-row-assistant");
  row.id = "entry-thinking";
  const bubble = div("x-bubble x-bubble-assistant x-bubble-thinking");
  bubble.appendChild(thinkingDots());
  row.appendChild(bubble);
  return row;
}

function thinkingDots(): HTMLElement {
  const dots = div("x-dots");
  for (let i = 0; i < 3; i++) {
    dots.appendChild(div("x-dot"));
  }
  return dots;
}

function markdownBody(text: string): HTMLElement {
  const root = div("x-body");
  if (!looksLikeMarkdown(text)) {
    root.classList.add("x-body-plain");
    root.textContent = text;
    return root;
  }
  root.appendChild(renderMarkdownFragment(text));
  return root;
}

function looksLikeMarkdown(text: string): boolean {
  return /(^|\n)(#{1,6}\s|[-*]\s|\d+\.\s|>\s|```|\|.+\||\*\*|__|`|\[.+\]\(.+\))/.test(
    text,
  );
}

function buildComposer(state: ChatPanelState): HTMLElement {
  const root = div("x-composer");
  const area = document.createElement("textarea");
  area.className = "x-composer-input";
  area.rows = 1;
  area.placeholder = state.isViewer
    ? "Read-only viewer — input disabled."
    : state.currentPlaceholder && state.currentPlaceholder.trim().length > 0
      ? state.currentPlaceholder
      : state.supportsPromptEntry
        ? "Send a message..."
        : "This backend runs in a terminal, not in the panel chat.";
  area.value = ui.draft;
  area.disabled =
    state.isViewer || !state.supportsPromptEntry || state.isStreaming;
  autoResize(area);
  area.addEventListener("input", () => {
    ui.draft = area.value;
    persist();
    autoResize(area);
  });
  area.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" || event.shiftKey) {
      return;
    }
    if (!canSend(state)) {
      return;
    }
    event.preventDefault();
    submitPrompt();
  });

  const sendBtn = document.createElement("button");
  sendBtn.type = "button";
  sendBtn.className = "x-send";
  sendBtn.textContent = state.isStreaming ? "Stop" : "Send";
  sendBtn.disabled = state.isStreaming ? !state.canStop : !canSend(state);
  sendBtn.addEventListener("click", () => {
    if (state.isStreaming) {
      if (state.canStop) {
        send({ type: "stop-conversation" });
      }
      return;
    }
    submitPrompt();
  });

  root.append(area, sendBtn);
  return root;
}

function autoResize(area: HTMLTextAreaElement): void {
  area.style.height = "auto";
  const max = 180;
  area.style.height = `${Math.min(area.scrollHeight, max)}px`;
}

function submitPrompt(): void {
  const prompt = ui.draft.trim();
  if (prompt.length === 0) {
    return;
  }
  ui.pinnedToBottom = true;
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
  return (
    !state.isViewer &&
    state.supportsPromptEntry &&
    !state.isStreaming &&
    ui.draft.trim().length > 0
  );
}

function div(
  className: string,
  ...children: Array<Node | string>
): HTMLElement {
  const node = document.createElement("div");
  node.className = className;
  for (const child of children) {
    node.append(child);
  }
  return node;
}

// Kick off the Shiki highlighter in the background. When it's ready we
// re-render so code blocks that were emitted as plain `<pre><code>` get
// repainted with token colors.
void initShiki().then(() => render());

send({ type: "ready" });
render();
