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
import {
  inferLangFromContent,
  initShiki,
  renderMarkdownFragment,
} from "./renderMarkdown";
import { stripProtocolFences, stripToolCallFencesForDisplay } from "./state";

declare function acquireVsCodeApi(): {
  postMessage(message: WebviewMessage): void;
  setState(state: unknown): void;
  getState<T = unknown>(): T | undefined;
};

const vscode = acquireVsCodeApi();

type PaletteName = "default" | "autumn" | "olive" | "sage";

const PALETTES: ReadonlyArray<{ value: PaletteName; label: string }> = [
  { value: "default", label: "Default" },
  { value: "autumn", label: "Autumn" },
  { value: "olive", label: "Olive" },
  { value: "sage", label: "Sage" },
];

// "default" disables role tinting entirely (no stripe, no bg) so
// the chat panel inherits the editor theme verbatim. New webviews
// start there; users opt in to a tinted palette via the gear-icon
// settings popover.
const DEFAULT_PALETTE: PaletteName = "default";

function isPaletteName(value: unknown): value is PaletteName {
  return (
    value === "default" ||
    value === "autumn" ||
    value === "olive" ||
    value === "sage"
  );
}

interface UiState {
  state: ChatPanelState | null;
  draft: string;
  pinnedToBottom: boolean;
  scrollTop: number;
  palette: PaletteName;
  /**
   * Whether the toolbar's expand/collapse toggle most recently
   * set bubbles to the expanded state. The toggle icon flips on
   * each click to indicate what the *next* click would do.
   * Initial value reflects how newly-rendered bubbles default
   * (tool results closed, everything else open).
   */
  bubblesExpanded: boolean;
}

interface PersistedState {
  draft?: string;
  palette?: PaletteName;
}

const persisted = vscode.getState<PersistedState>();
const ui: UiState = {
  state: null,
  draft: persisted?.draft ?? "",
  pinnedToBottom: true,
  scrollTop: 0,
  palette: isPaletteName(persisted?.palette) ? persisted.palette : DEFAULT_PALETTE,
  bubblesExpanded: true,
};

applyPalette();

function applyPalette(): void {
  document.body.dataset.palette = ui.palette;
}

window.addEventListener("message", (event) => {
  const msg = event.data as HostMessage;
  if (!msg || typeof msg.type !== "string") {
    return;
  }
  if (msg.type === "state-update") {
    ui.state = msg.state;
    render();
    return;
  }
  if (msg.type === "file-picked") {
    insertIntoDraft(msg.path);
    return;
  }
});

/**
 * Append a file path (or any text) to the current composer draft.
 * Used by the Browse button's response from the host. Adds a space
 * separator only when the existing draft is non-empty and doesn't
 * already end in whitespace, so consecutive Browse clicks don't
 * collapse paths together.
 */
function insertIntoDraft(text: string): void {
  if (text.length === 0) {
    return;
  }
  const needsSeparator =
    ui.draft.length > 0 && !/\s$/.test(ui.draft);
  ui.draft = needsSeparator ? `${ui.draft} ${text}` : `${ui.draft}${text}`;
  persist();
  render();
  // Refocus the textarea after the re-render so the user can keep
  // typing without a manual click. The render call replaces the DOM
  // node, so `document.activeElement` would otherwise have moved.
  queueMicrotask(() => {
    const area = document.querySelector<HTMLTextAreaElement>(
      ".x-composer-input",
    );
    if (area) {
      area.focus();
      area.setSelectionRange(area.value.length, area.value.length);
    }
  });
}

function send(message: WebviewMessage): void {
  vscode.postMessage(message);
}

function persist(): void {
  vscode.setState({ draft: ui.draft, palette: ui.palette });
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
      // Preserve the user-toggled open state of tool-result <details>
      // across re-renders. Without this, morphdom would overwrite the
      // attribute with the freshly-built (collapsed) default each
      // time another transcript entry arrives.
      if (
        fromEl instanceof HTMLDetailsElement &&
        toEl instanceof HTMLDetailsElement
      ) {
        if (fromEl.open) {
          toEl.setAttribute("open", "");
        } else {
          toEl.removeAttribute("open");
        }
      }
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
  shell.appendChild(buildToolbar(ui.state));
  shell.appendChild(buildTranscript(ui.state));
  shell.appendChild(buildComposer(ui.state));
  return [shell];
}

/**
 * Inline SVGs that approximate VS Code's `expand-all` / `collapse-all`
 * codicons (two overlapping squares with a +/- in the front one).
 * Embedding the SVG avoids depending on the codicon font file, which
 * the chat panel webview doesn't currently load.
 */
const TOGGLE_BUBBLES_COLLAPSE_SVG = `
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1" stroke-linejoin="round">
  <rect x="1.5" y="1.5" width="9" height="9"/>
  <rect x="5.5" y="5.5" width="9" height="9" fill="var(--vscode-editor-background)"/>
  <line x1="7.5" y1="10" x2="12.5" y2="10"/>
</svg>`;

const TOGGLE_BUBBLES_EXPAND_SVG = `
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1" stroke-linejoin="round">
  <rect x="1.5" y="1.5" width="9" height="9"/>
  <rect x="5.5" y="5.5" width="9" height="9" fill="var(--vscode-editor-background)"/>
  <line x1="7.5" y1="10" x2="12.5" y2="10"/>
  <line x1="10" y1="7.5" x2="10" y2="12.5"/>
</svg>`;

/**
 * Settings gear icon. Approximates VS Code's `gear` codicon as
 * inline SVG so we don't need to load the codicon font.
 */
const SETTINGS_GEAR_SVG = `
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round">
  <circle cx="8" cy="8" r="2.2"/>
  <path d="M8 1.5v2 M8 12.5v2 M1.5 8h2 M12.5 8h2 M3.5 3.5l1.4 1.4 M11.1 11.1l1.4 1.4 M3.5 12.5l1.4-1.4 M11.1 4.9l1.4-1.4"/>
</svg>`;

function buildToolbar(state: ChatPanelState): HTMLElement {
  const root = div("x-toolbar");

  // Three zones: a left group, a centered middle group with the
  // LLM status + token totals, and a right group with the settings
  // gear. CSS grid-template-columns `auto 1fr auto` keeps the
  // center genuinely centered no matter how wide the side groups
  // get.
  const leftZone = div("x-toolbar-left");
  const centerZone = div("x-toolbar-center");
  const rightZone = div("x-toolbar-right");

  // ---- LEFT: bubble toggle + project button ----

  const toggleBtn = document.createElement("button");
  toggleBtn.type = "button";
  toggleBtn.className = "x-toolbar-bubble-toggle";
  toggleBtn.innerHTML = ui.bubblesExpanded
    ? TOGGLE_BUBBLES_COLLAPSE_SVG
    : TOGGLE_BUBBLES_EXPAND_SVG;
  toggleBtn.setAttribute(
    "aria-label",
    ui.bubblesExpanded ? "Collapse all bubbles" : "Expand all bubbles",
  );
  toggleBtn.title = ui.bubblesExpanded
    ? "Collapse all message bubbles in the transcript."
    : "Expand all message bubbles in the transcript.";
  toggleBtn.addEventListener("click", () => {
    ui.bubblesExpanded = !ui.bubblesExpanded;
    setAllBubblesOpen(ui.bubblesExpanded);
    render();
  });
  leftZone.appendChild(toggleBtn);

  // Project button: doubles as the Start-session affordance when
  // no pump is anchored. Window reloads no longer auto-launch, so
  // this button is how the user starts a fresh session -- and
  // how they switch projects once one is running.
  const projectBtn = document.createElement("button");
  projectBtn.type = "button";
  projectBtn.className = "x-toolbar-project";
  if (state.sessionActive) {
    const projectName =
      state.projectLabel && state.projectLabel.length > 0
        ? state.projectLabel
        : "No project";
    projectBtn.textContent = `Project: ${projectName}`;
    projectBtn.title =
      "Switch the chat panel to a different sim-flow project. Stops the active session and launches a fresh one on the chosen project.";
  } else {
    projectBtn.textContent = "Start session";
    projectBtn.classList.add("x-toolbar-project-start");
    projectBtn.title =
      "Launch a sim-flow session. Uses the last project you worked on; otherwise opens the project picker.";
  }
  projectBtn.addEventListener("click", () => {
    const live = ui.state;
    if (live?.sessionActive) {
      send({ type: "switch-project" });
    } else {
      send({ type: "start-session" });
    }
  });
  leftZone.appendChild(projectBtn);

  // ---- CENTER: LLM status indicator + total tokens ----

  const llmStatus = document.createElement("span");
  llmStatus.className = "x-toolbar-llm";
  if (!state.sessionActive) {
    llmStatus.classList.add("x-llm-offline");
    llmStatus.textContent = "○ No session";
    llmStatus.title =
      "No sim-flow pump is anchored to this project. Click \"Start session\" to start one.";
  } else if (state.isStreaming) {
    llmStatus.classList.add("x-llm-working");
    llmStatus.textContent = `● Working · ${state.sourceLabel}`;
    llmStatus.title = `sim-flow is talking to ${state.sourceLabel} right now.`;
  } else {
    llmStatus.classList.add("x-llm-ready");
    llmStatus.textContent = `● Ready · ${state.sourceLabel}`;
    llmStatus.title = `Pump is connected; ${state.sourceLabel} will be called on the next sub-session.`;
  }
  centerZone.appendChild(llmStatus);

  const totals = document.createElement("span");
  totals.className = "x-toolbar-tokens";
  const upTotal = state.totalInputTokensEstimate;
  const downTotal = state.totalOutputTokensEstimate;
  totals.textContent = `↑ ${formatTokens(upTotal)}  ↓ ${formatTokens(downTotal)}`;
  totals.title = `Approximately ${upTotal} tokens sent to the LLM and ${downTotal} tokens received in this conversation.`;
  centerZone.appendChild(totals);

  // ---- RIGHT: settings gear (palette select + future settings) ----

  // <details>-based popover. Clicking the gear toggles `open` and
  // the panel inside is absolutely-positioned via CSS so it floats
  // over the transcript instead of pushing other toolbar items
  // around.
  const settings = document.createElement("details");
  settings.className = "x-toolbar-settings";
  const settingsSummary = document.createElement("summary");
  settingsSummary.className = "x-toolbar-settings-summary";
  settingsSummary.innerHTML = SETTINGS_GEAR_SVG;
  settingsSummary.setAttribute("aria-label", "Open chat panel settings");
  settingsSummary.title = "Settings";
  settings.appendChild(settingsSummary);

  const settingsPanel = div("x-toolbar-settings-panel");
  const palLabel = document.createElement("label");
  palLabel.className = "x-toolbar-settings-label";
  palLabel.textContent = "Palette";
  settingsPanel.appendChild(palLabel);

  const select = document.createElement("select");
  select.className = "x-toolbar-palette";
  for (const entry of PALETTES) {
    const option = document.createElement("option");
    option.value = entry.value;
    option.textContent = entry.label;
    if (entry.value === ui.palette) {
      option.selected = true;
    }
    select.appendChild(option);
  }
  select.addEventListener("change", () => {
    if (isPaletteName(select.value)) {
      ui.palette = select.value;
      applyPalette();
      persist();
    }
  });
  palLabel.appendChild(select);

  settings.appendChild(settingsPanel);
  rightZone.appendChild(settings);

  root.append(leftZone, centerZone, rightZone);
  return root;
}

/**
 * Toggle every transcript `<details>` to the given state. The
 * morphdom-onBeforeElUpdated hook below preserves whatever state we
 * apply here across subsequent re-renders, so the user's choice
 * sticks. New entries that arrive after the click fall back to their
 * per-kind default (tool collapsed, others open).
 */
function setAllBubblesOpen(open: boolean): void {
  const transcript = document.querySelector<HTMLElement>(".x-transcript");
  if (!transcript) {
    return;
  }
  const details = transcript.querySelectorAll<HTMLDetailsElement>(
    "details.x-bubble-details",
  );
  for (const node of Array.from(details)) {
    node.open = open;
  }
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
  const tool = entry.kind === "user" && entry.meta === "orchestrator-tool";
  const row = div(
    `x-row x-row-${role}${orchestrator ? " x-row-orchestrator" : ""}${
      tool ? " x-row-tool" : ""
    }`,
  );
  // Stable id so morphdom keeps DOM identity across renders -- this is
  // what makes streaming chunks patch in place instead of rebuilding.
  row.id = `entry-${entry.id}`;
  const bubble = div(
    `x-bubble x-bubble-${role}${orchestrator ? " x-bubble-orchestrator" : ""}${
      tool ? " x-bubble-tool" : ""
    }`,
  );
  if (body.length === 0 && entry.streaming) {
    bubble.appendChild(thinkingDots());
  } else {
    // Tool calls collapse by default (file dumps would otherwise dwarf
    // the rest of the turn); everything else opens by default. The
    // morphdom hook preserves whatever the user toggles.
    //
    // Tool results from `read_file` on a markdown file are an
    // exception: we leave `forceCodeBlock` off so the body renders
    // as markdown (headings, lists, fenced code blocks within) and
    // the user sees the document the way it's meant to read --
    // rather than a literal dump of `#` and `-` characters.
    const renderAsCode = tool && !isReadingMarkdownFile(body);
    bubble.appendChild(
      bubbleDetails(
        entry.title,
        body,
        !tool,
        renderAsCode,
        tokenBadgeFor(entry, body),
      ),
    );
  }
  row.appendChild(bubble);
  return row;
}

/**
 * Wrap a message body in a <details>. The summary shows the role
 * label plus a short preview of the first non-empty line so the user
 * can scan the transcript without expanding every entry. User toggles
 * are preserved across morphdom diffs by the onBeforeElUpdated hook
 * above. The expand-all / collapse-all toolbar buttons flip every
 * existing <details> in place.
 */
function bubbleDetails(
  title: string,
  body: string,
  defaultOpen: boolean,
  forceCodeBlock = false,
  tokens: { text: string; title: string } | null = null,
): HTMLElement {
  const details = document.createElement("details");
  details.className = "x-bubble-details";
  if (defaultOpen) {
    details.setAttribute("open", "");
  }
  const summary = document.createElement("summary");
  const label = document.createElement("span");
  label.className = "x-bubble-summary-label";
  label.textContent = title.length > 0 ? title : "Message";
  summary.appendChild(label);
  // Preview is computed from the raw body (NOT the fenced wrap so it
  // doesn't read as "```rust" for tool results).
  const preview = firstNonEmptyLine(body);
  if (preview.length > 0) {
    const previewNode = document.createElement("span");
    previewNode.className = "x-bubble-summary-preview";
    previewNode.textContent = preview;
    summary.appendChild(previewNode);
  }
  // Per-turn token badge anchored at the summary's right edge. The
  // preview node has `flex: 1` so this gets pushed to the right.
  if (tokens) {
    const tokenNode = document.createElement("span");
    tokenNode.className = "x-bubble-summary-tokens";
    tokenNode.textContent = tokens.text;
    tokenNode.title = tokens.title;
    summary.appendChild(tokenNode);
  }
  details.appendChild(summary);
  // Tool-result bubbles (forceCodeBlock = true) are almost always
  // code-like content: file dumps, command output, JSON, build
  // logs. Wrapping in a fenced block with a guessed language gives
  // them the same code-block treatment as inline ```...``` from the
  // assistant, including Shiki syntax highlighting when the
  // inference finds a match.
  const renderText = forceCodeBlock ? wrapAsCodeBlock(body) : body;
  details.appendChild(markdownBody(renderText));
  return details;
}

/**
 * Detect tool-result bodies whose `[read_file ...]` header points
 * at a markdown file. Used to opt those bubbles out of the
 * code-block wrap so the document renders as markdown (headings,
 * lists, fenced blocks) rather than as a literal text dump.
 *
 * Matches the orchestrator's read_file format:
 *   "[read_file `path/to/doc.md`]\n\n<contents>"
 *
 * Other tool wrappers (write_file, search, etc.) still take the
 * code-block path since their output is typically log-shaped.
 */
function isReadingMarkdownFile(body: string): boolean {
  const match = body.match(/^\[read_file `([^`]+)`\]/);
  if (!match) {
    return false;
  }
  const path = match[1] ?? "";
  return /\.(md|markdown|mdx)$/i.test(path);
}

/**
 * Wrap content in a fenced code block with a content-inferred
 * language. Uses a long backtick fence so embedded shorter fences
 * in the content don't close the wrapper prematurely.
 *
 * When the heuristic can't pick a language we emit the fence
 * WITHOUT one (rather than falling back to "text") so markdown-it
 * produces a `<code>` with no `language-X` class -- otherwise
 * `applyShikiHighlight` short-circuits on the explicit "text" tag
 * and never tries the content-based re-guess inside the highlight
 * pass.
 */
function wrapAsCodeBlock(content: string): string {
  const lang = inferLangFromContent(content);
  const fence = "`".repeat(16);
  return lang
    ? `${fence}${lang}\n${content}\n${fence}`
    : `${fence}\n${content}\n${fence}`;
}

/**
 * Compact label for a token count: bare integer under 1000, "1.2k"
 * under 10000, "42k" beyond. Same shape used in both the per-bubble
 * summary badge and the toolbar totals.
 */
function formatTokens(n: number): string {
  if (n <= 0) {
    return "0";
  }
  if (n < 1000) {
    return String(n);
  }
  if (n < 10000) {
    return `${(n / 1000).toFixed(1)}k`;
  }
  return `${Math.round(n / 1000)}k`;
}

/**
 * Build a per-turn token badge. User / orchestrator-user / tool
 * bubbles read as "input" and show an up arrow; assistant bubbles
 * read as "output" and show a down arrow. Stored token estimates
 * win when available; otherwise we fall back to the same heuristic
 * the host uses (~4 chars per token).
 */
function tokenBadgeFor(
  entry: Extract<ChatTranscriptEntry, { kind: "user" | "assistant" }>,
  body: string,
): { text: string; title: string } | null {
  const isInput = entry.kind === "user";
  const stored = isInput
    ? entry.requestTokensEstimate
    : entry.responseTokensEstimate;
  const fallback = body.length === 0 ? 0 : Math.max(1, Math.ceil(body.length / 4));
  const count = stored ?? fallback;
  if (count <= 0) {
    return null;
  }
  const arrow = isInput ? "↑" : "↓";
  const direction = isInput ? "sent to" : "received from";
  return {
    text: `${arrow} ${formatTokens(count)}`,
    title: `Approximately ${count} tokens ${direction} the LLM in this turn.`,
  };
}

function firstNonEmptyLine(text: string): string {
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (line.length === 0) {
      continue;
    }
    const max = 120;
    return line.length > max ? `${line.slice(0, max - 1)}…` : line;
  }
  return "";
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
  // The composer wraps two stacked rows -- the input row (textarea +
  // send) and the meta row (mode toggle, future continue button).
  // Stack them via flex column rather than appending to the panel's
  // grid; the meta row sits inside the same bordered container so
  // the visual seam is one panel, not two.
  const area = document.createElement("textarea");
  area.className = "x-composer-input";
  area.id = "x-composer-textarea";
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
    // Re-evaluate the send button's disabled state. canSend reads
    // `ui.draft.trim().length`, which only updates here -- not on
    // a host state-update -- so without this hook the click-target
    // stays disabled until the next render. Read from `ui.state`
    // rather than the closure's `state` so the listener uses the
    // latest values after morphdom carries it across renders.
    const s = ui.state;
    if (!s) {
      return;
    }
    sendBtn.disabled = s.isStreaming ? !s.canStop : !canSend(s);
  });
  area.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" || event.shiftKey) {
      return;
    }
    const s = ui.state;
    if (!s || !canSend(s)) {
      return;
    }
    event.preventDefault();
    submitPrompt();
  });

  // Send / Stop button. Renders as an icon (↑ for send, ■ for stop)
  // since both glyphs are universally understood and read at a
  // glance. The button's class swaps between `x-send-send` and
  // `x-send-stop` so CSS can recolour the stop state in the warning
  // palette without changing the layout. aria-label + title carry
  // the verbal action for screen readers and hover tooltips.
  const sendBtn = document.createElement("button");
  sendBtn.type = "button";
  // Stable id so morphdom always matches a "send" slot to a previous
  // "send" slot. Without it, when the input row's child count
  // changes across renders (Browse appearing/disappearing on the
  // DM0 boundary), morphdom matches positionally by tag and the
  // visible Send icon can inherit the original Browse click
  // handler -- so clicking Send opens the file picker.
  sendBtn.id = "x-composer-send";
  sendBtn.className = state.isStreaming ? "x-send x-send-stop" : "x-send x-send-send";
  sendBtn.textContent = state.isStreaming ? "■" : "↑";
  sendBtn.setAttribute(
    "aria-label",
    state.isStreaming ? "Stop the current request" : "Send message",
  );
  sendBtn.title = state.isStreaming ? "Stop" : "Send";
  sendBtn.disabled = state.isStreaming ? !state.canStop : !canSend(state);
  sendBtn.addEventListener("click", () => {
    // Read from `ui.state` rather than the closure's `state` so we
    // see the latest values even when morphdom keeps an older
    // render's listener attached to the live DOM node. Otherwise
    // `state.isStreaming` here would forever reflect the first
    // render's snapshot and the Stop path would never fire.
    const s = ui.state;
    if (!s) {
      return;
    }
    if (s.isStreaming) {
      if (s.canStop) {
        send({ type: "stop-conversation" });
      }
      return;
    }
    submitPrompt();
  });

  const inputRow = div("x-composer-input-row");
  // Browse… is only meaningful while the orchestrator is in DM0
  // (the spec-ingest step). Other steps don't take a file path as
  // their primary input, so the button just adds clutter there.
  // When DM0 advances to DM1 the button disappears automatically
  // because `state.currentStep` reflects state.toml.
  if (state.currentStep === "DM0") {
    const browseBtn = document.createElement("button");
    browseBtn.type = "button";
    browseBtn.id = "x-composer-browse";
    browseBtn.className = "x-browse";
    browseBtn.textContent = "Browse…";
    browseBtn.title =
      "Pick a file or directory and insert its absolute path into the message.";
    browseBtn.disabled =
      state.isViewer || !state.supportsPromptEntry || state.isStreaming;
    browseBtn.addEventListener("click", () => {
      if (browseBtn.disabled) {
        return;
      }
      send({ type: "pick-file" });
    });
    inputRow.append(area, browseBtn, sendBtn);
  } else {
    inputRow.append(area, sendBtn);
  }
  root.appendChild(inputRow);
  root.appendChild(buildComposerMeta(state));
  return root;
}

/**
 * Composer footer holding the Manual/Auto step-mode toggle and the
 * Continue button. The mode toggle wires the orchestrator's
 * `setStepMode` (auto/manual); Continue dispatches the host-computed
 * `nextAction` so the user can advance the flow without leaving the
 * chat panel. Both are disabled when no pump is live.
 */
function buildComposerMeta(state: ChatPanelState): HTMLElement {
  const root = div("x-composer-meta");
  const disabled = state.currentStepMode === null;

  // Continue button on the left -- the primary flow-driving action,
  // placed where the eye reads first. Only meaningful when the host
  // computed a next action (manual mode + parked + has a successor);
  // we always render it so the user has a stable target.
  const continueBtn = document.createElement("button");
  continueBtn.type = "button";
  continueBtn.id = "x-composer-continue";
  continueBtn.className = "x-continue";
  const action = state.nextAction;
  continueBtn.textContent = action ? `Continue: ${action.label}` : "Continue";
  continueBtn.disabled = !action || state.isStreaming || state.isViewer;
  continueBtn.title = action
    ? `Dispatches \`${action.kind}\` on \`${action.step}\` over the live session.`
    : "Continue is available when the orchestrator parks between sub-sessions in manual mode.";
  continueBtn.addEventListener("click", () => {
    if (continueBtn.disabled) {
      return;
    }
    send({ type: "continue-flow" });
  });
  root.appendChild(continueBtn);

  root.appendChild(div("x-composer-meta-spacer"));

  if (disabled) {
    const hint = document.createElement("span");
    hint.className = "x-composer-meta-hint";
    hint.textContent = "(no live session)";
    root.appendChild(hint);
  }

  // Mode toggle on the right -- text-only button (no background,
  // no border) so it reads as a plain label that happens to be
  // clickable. The button text is the *current* mode ("Auto" or
  // "Manual"); clicking flips to the other. We keep it as a
  // <button> rather than a span so keyboard focus + Enter work
  // the same as any other control.
  const isAuto = state.currentStepMode === "auto";
  const modeBtn = document.createElement("button");
  modeBtn.type = "button";
  modeBtn.id = "x-composer-mode";
  modeBtn.className = "x-mode-toggle";
  modeBtn.textContent = isAuto ? "Auto" : "Manual";
  modeBtn.disabled = disabled;
  modeBtn.title = isAuto
    ? "Auto (orchestrator runs sub-sessions to completion). Click to switch to Manual."
    : "Manual (orchestrator parks between sub-sessions; click Continue to advance). Click to switch to Auto.";
  modeBtn.addEventListener("click", () => {
    if (modeBtn.disabled) {
      return;
    }
    // Read the live mode from `ui.state` so the click flips against
    // the latest value rather than the closure's first-render
    // snapshot (morphdom keeps the listener from render-1 attached
    // to the live DOM node).
    const live = ui.state?.currentStepMode;
    send({ type: "set-step-mode", mode: live === "auto" ? "manual" : "auto" });
  });
  root.appendChild(modeBtn);

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
// repainted with token colors. Failures are logged to the webview's
// devtools console so a regression (CSP change, bundle drift) shows up
// instead of silently disabling highlighting.
void initShiki()
  .then(() => render())
  .catch((err) => {
    console.error("sim-flow chat panel: Shiki init failed", err);
  });

send({ type: "ready" });
render();
