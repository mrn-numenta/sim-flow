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

import { stepOrderFor } from "../state/stepOrder";
import type { Finding } from "../state/types";
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

type OpenPopup =
  | null
  | { kind: "help" }
  | { kind: "critique"; step: string };

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
  /**
   * Which overlay popup is currently visible above the chat panel,
   * if any. `help` covers the per-step help icon in the header;
   * `critique` covers a click on any step rail tile. Persisted
   * across renders so morphdom keeps the popup mounted while the
   * transcript continues to update underneath.
   */
  openPopup: OpenPopup;
  /**
   * Per-step critique data cached after a `critique-data` reply
   * lands. Cleared when the user closes / switches popups, so a
   * stale entry doesn't pre-fill the next opening with the wrong
   * step's findings. `undefined` for "not yet fetched";
   * `null` for "fetched and no critique on disk".
   */
  critiqueData: Map<
    string,
    { findings: Finding[]; hasBlocking: boolean } | null
  >;
  /**
   * Right-click context menu anchored over the step rail. `null`
   * when nothing is open; otherwise carries the step id the user
   * clicked plus the click coordinates so the overlay can place
   * itself there. A click anywhere else closes the menu.
   */
  openContextMenu: { step: string; x: number; y: number } | null;
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
  openPopup: null,
  critiqueData: new Map(),
  openContextMenu: null,
};

applyPalette();
installFileLinkDelegation();

function applyPalette(): void {
  document.body.dataset.palette = ui.palette;
}

/**
 * One delegated click listener on the document catches every
 * click + keyboard activation on a `.x-file-link` node (those are
 * tagged by `linkifyFilePaths` after each markdown render). The
 * listener survives morphdom rebuilds because it's bound to
 * `document`, not to any node morphdom owns.
 */
function installFileLinkDelegation(): void {
  const activate = (target: EventTarget | null): boolean => {
    if (!(target instanceof Element)) {
      return false;
    }
    const link = target.closest<HTMLElement>(".x-file-link");
    if (!link) {
      return false;
    }
    const path = link.getAttribute("data-file-path");
    if (!path) {
      return false;
    }
    send({ type: "open-file", path });
    return true;
  };
  document.addEventListener("click", (event) => {
    if (activate(event.target)) {
      event.preventDefault();
      event.stopPropagation();
    }
  });
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") {
      return;
    }
    if (activate(event.target)) {
      event.preventDefault();
      event.stopPropagation();
    }
  });
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
  if (msg.type === "critique-data") {
    // Stale-reply guard: a fast user could have already closed the
    // popup or clicked a different step. Drop the result unless
    // they're still looking at the matching critique popup.
    ui.critiqueData.set(msg.step, msg.data);
    if (
      ui.openPopup &&
      ui.openPopup.kind === "critique" &&
      ui.openPopup.step === msg.step
    ) {
      render();
    }
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
  const nodes: Node[] = [shell];
  // Overlay popups live as siblings of the shell so their fixed
  // positioning can cover the toolbar + transcript + composer
  // without inheriting the grid's row tracks.
  const popup = buildPopup(ui.state);
  if (popup) {
    nodes.push(popup);
  }
  const ctxMenu = buildContextMenu();
  if (ctxMenu) {
    nodes.push(ctxMenu);
  }
  return nodes;
}

// Codicon markup for the toolbar icons. The codicon stylesheet
// is loaded by the host HTML; `<i class="codicon codicon-X">`
// renders the glyph from the bundled icon font.
const TOGGLE_BUBBLES_COLLAPSE_ICON = `<i class="codicon codicon-collapse-all" aria-hidden="true"></i>`;
const TOGGLE_BUBBLES_EXPAND_ICON = `<i class="codicon codicon-expand-all" aria-hidden="true"></i>`;

const SETTINGS_GEAR_ICON = `<i class="codicon codicon-settings-gear" aria-hidden="true"></i>`;

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
    ? TOGGLE_BUBBLES_COLLAPSE_ICON
    : TOGGLE_BUBBLES_EXPAND_ICON;
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

  // ---- CENTER: end-session + LLM status, fused into one pill ----

  // The end-session button and the LLM status pill are presented
  // as one visual unit so the user reads them as "this control
  // belongs to that connection." Same border, same colour family
  // (offline / ready / working), no gap between them. The state
  // class on the wrapping group drives the shared palette.
  const llmGroup = div("x-llm-group");
  let stateClass: "x-llm-offline" | "x-llm-ready" | "x-llm-working";
  let statusGlyph: string;
  let statusText: string;
  let statusTitle: string;
  if (!state.sessionActive) {
    stateClass = "x-llm-offline";
    statusGlyph = "○";
    statusText = "No session";
    statusTitle =
      "No sim-flow pump is anchored to this project. Click \"Start session\" to start one.";
  } else if (state.isStreaming) {
    stateClass = "x-llm-working";
    statusGlyph = "●";
    statusText = `Working · ${state.sourceLabel}`;
    statusTitle = `sim-flow is talking to ${state.sourceLabel} right now.`;
  } else {
    stateClass = "x-llm-ready";
    statusGlyph = "●";
    statusText = `Ready · ${state.sourceLabel}`;
    statusTitle = `Pump is connected; ${state.sourceLabel} will be called on the next sub-session.`;
  }
  llmGroup.classList.add(stateClass);

  if (state.sessionActive) {
    const endBtn = document.createElement("button");
    endBtn.type = "button";
    endBtn.className = "x-toolbar-end-session";
    endBtn.innerHTML = `<i class="codicon codicon-debug-disconnect" aria-hidden="true"></i>`;
    endBtn.setAttribute("aria-label", "End session");
    endBtn.title =
      "End the sim-flow session: disconnect from the LLM and terminate the orchestrator.";
    endBtn.addEventListener("click", () => {
      send({ type: "end-session" });
    });
    llmGroup.appendChild(endBtn);
  }

  const llmStatus = document.createElement("span");
  llmStatus.className = "x-toolbar-llm";
  llmStatus.textContent = `${statusGlyph} ${statusText}`;
  llmStatus.title = statusTitle;
  llmGroup.appendChild(llmStatus);
  centerZone.appendChild(llmGroup);

  // ---- RIGHT: dashboard + help + settings + total tokens ----
  // Help sits adjacent to settings so the user reads
  // "info / config" as one pair on the right edge.

  // Dashboard button: opens the sim-flow dashboard for the chat
  // panel's current project (no picker; host reads the same
  // remembered project the chat panel itself is anchored to).
  const dashboardBtn = document.createElement("button");
  dashboardBtn.type = "button";
  dashboardBtn.className = "x-toolbar-dashboard";
  dashboardBtn.innerHTML = `<i class="codicon codicon-dashboard" aria-hidden="true"></i>`;
  dashboardBtn.setAttribute("aria-label", "Open dashboard");
  dashboardBtn.title =
    "Open the sim-flow dashboard for the current project.";
  dashboardBtn.addEventListener("click", () => {
    send({ type: "open-dashboard" });
  });
  rightZone.appendChild(dashboardBtn);

  // Help button: opens a popup describing every step in the rail.
  const helpBtn = document.createElement("button");
  helpBtn.type = "button";
  helpBtn.className = "x-toolbar-help";
  helpBtn.innerHTML = `<i class="codicon codicon-question" aria-hidden="true"></i>`;
  helpBtn.setAttribute("aria-label", "Help");
  helpBtn.title = "Open the per-step help guide.";
  helpBtn.addEventListener("click", () => {
    // Toggle: a second click on the same icon closes the popup so
    // the user can dismiss without moving the mouse.
    ui.openPopup = ui.openPopup?.kind === "help" ? null : { kind: "help" };
    render();
  });
  rightZone.appendChild(helpBtn);

  // Settings gear: <details>-based popover. Clicking the gear
  // toggles `open` and the panel inside is absolutely-positioned
  // via CSS so opening it doesn't shift the rest of the toolbar.
  const settings = document.createElement("details");
  settings.className = "x-toolbar-settings";
  const settingsSummary = document.createElement("summary");
  settingsSummary.className = "x-toolbar-settings-summary";
  settingsSummary.innerHTML = SETTINGS_GEAR_ICON;
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

  // Context-window usage pie + total token counts on the far right
  // edge of the toolbar. The pie shows cumulative input+output
  // tokens as a fraction of a reference context window; the counts
  // sit directly above the per-bubble ↑/↓ counts inside the
  // transcript summaries. Both are hidden when no session is
  // anchored.
  if (state.sessionActive) {
    const usedTokens =
      state.totalInputTokensEstimate + state.totalOutputTokensEstimate;
    const fraction = Math.max(0, Math.min(1, usedTokens / LLM_CONTEXT_WINDOW));
    const pct = Math.round(fraction * 100);
    rightZone.appendChild(buildContextPie(fraction, pct, usedTokens));
  }

  const totals = document.createElement("span");
  totals.className = "x-toolbar-tokens";
  const upTotal = state.totalInputTokensEstimate;
  const downTotal = state.totalOutputTokensEstimate;
  totals.textContent = `↑ ${formatTokens(upTotal)}  ↓ ${formatTokens(downTotal)}`;
  totals.title =
    `Approximately ${upTotal} tokens sent to the LLM and ${downTotal} tokens received in this conversation.`;
  rightZone.appendChild(totals);

  root.append(leftZone, centerZone, rightZone);
  return root;
}

/**
 * Reference context window for the context-used pie indicator in
 * the toolbar. Most current frontier models support at least this
 * much; smaller local models will over-report (still informative
 * as a trend, just not absolute).
 */
const LLM_CONTEXT_WINDOW = 128_000;

/**
 * Render the context-usage pie chart icon. A white-stroked outer
 * circle holds a white-filled sector covering `fraction` (0..1)
 * of the disc, swept clockwise from 12 o'clock. The element gets
 * a `data-warn` flag past 80% so CSS can paint the fill red.
 */
function buildContextPie(
  fraction: number,
  pct: number,
  usedTokens: number,
): HTMLElement {
  const NS = "http://www.w3.org/2000/svg";
  const wrap = document.createElement("span");
  wrap.className = "x-toolbar-context-pie";
  if (pct >= 80) {
    wrap.classList.add("x-toolbar-context-pie-warn");
  }
  wrap.title =
    `Approximate context usage: ${pct}% (${usedTokens} tokens estimated against a ${formatTokens(LLM_CONTEXT_WINDOW)} reference window). ` +
    "Cumulative across the whole conversation; actual usage per LLM call depends on the model's context size.";
  const svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("width", "14");
  svg.setAttribute("height", "14");
  svg.setAttribute("aria-hidden", "true");
  // Outer ring.
  const ring = document.createElementNS(NS, "circle");
  ring.setAttribute("cx", "8");
  ring.setAttribute("cy", "8");
  ring.setAttribute("r", "7");
  ring.setAttribute("fill", "none");
  ring.setAttribute("stroke", "currentColor");
  ring.setAttribute("stroke-width", "1");
  svg.appendChild(ring);
  // Filled sector. SVG arcs need explicit start + end points; a
  // full pie (fraction=1) needs a tiny epsilon shave so the path
  // closes properly instead of rendering as a degenerate zero-area
  // shape.
  if (fraction > 0) {
    const safe = Math.min(0.9999, fraction);
    const angle = safe * Math.PI * 2;
    const cx = 8;
    const cy = 8;
    const r = 6;
    const startX = cx;
    const startY = cy - r;
    const endX = cx + r * Math.sin(angle);
    const endY = cy - r * Math.cos(angle);
    const largeArc = safe > 0.5 ? 1 : 0;
    const path = document.createElementNS(NS, "path");
    path.setAttribute(
      "d",
      `M ${cx} ${cy} L ${startX} ${startY} A ${r} ${r} 0 ${largeArc} 1 ${endX} ${endY} Z`,
    );
    path.setAttribute("fill", "currentColor");
    svg.appendChild(path);
  }
  wrap.appendChild(svg);
  return wrap;
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
  const system = entry.kind === "user" && entry.meta === "orchestrator-system";
  const row = div(
    `x-row x-row-${role}${orchestrator ? " x-row-orchestrator" : ""}${
      tool ? " x-row-tool" : ""
    }${system ? " x-row-system" : ""}`,
  );
  // Stable id so morphdom keeps DOM identity across renders -- this is
  // what makes streaming chunks patch in place instead of rebuilding.
  row.id = `entry-${entry.id}`;
  const bubble = div(
    `x-bubble x-bubble-${role}${orchestrator ? " x-bubble-orchestrator" : ""}${
      tool ? " x-bubble-tool" : ""
    }${system ? " x-bubble-system" : ""}`,
  );
  if (body.length === 0 && entry.streaming) {
    bubble.appendChild(thinkingDots());
  } else {
    // Tool results and System messages collapse by default so the
    // standing system prompt and chunky file dumps don't dominate
    // the scroll. Everything else opens by default. The morphdom
    // hook preserves whatever the user toggles.
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
        !tool && !system,
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
  const fragment = renderMarkdownFragment(text);
  linkifyFilePaths(fragment);
  root.appendChild(fragment);
  return root;
}

function looksLikeMarkdown(text: string): boolean {
  return /(^|\n)(#{1,6}\s|[-*]\s|\d+\.\s|>\s|```|\|.+\||\*\*|__|`|\[.+\]\(.+\))/.test(
    text,
  );
}

/**
 * Recognised file extensions for the chat transcript linkifier.
 * Anything outside this set is treated as prose -- prevents bogus
 * "links" on things like `1.5GHz`, `v2.3`, `foo.bar.baz`.
 */
const FILE_EXT_RE =
  /\.(?:md|markdown|txt|text|pdf|json|toml|yaml|yml|rs|ts|tsx|js|jsx|py|sv|svh|v|vh|c|h|cpp|hpp|cs|go|rb|sh|bash|css|html|xml|sql|lock|cfg|ini|conf)$/i;

/**
 * Test whether `text` looks like a file path we should turn into
 * a clickable link. Conservative: requires either an absolute path
 * or at least one slash, plus a recognised file extension. Skips
 * web URLs (those are handled by the sanitizer's `<a target=_blank>`
 * path) and bare words like "foo.md" with no slash (high false-
 * positive rate -- agents say things like `README.md` in prose).
 */
function looksLikeFilePath(text: string): boolean {
  const trimmed = text.trim();
  if (trimmed.length === 0 || trimmed.length > 512) {
    return false;
  }
  if (/^(?:https?:|mailto:|file:|ftp:)/i.test(trimmed)) {
    return false;
  }
  if (/\s/.test(trimmed)) {
    return false;
  }
  if (!FILE_EXT_RE.test(trimmed)) {
    return false;
  }
  return trimmed.startsWith("/") || trimmed.includes("/");
}

/**
 * Walk the rendered markdown fragment and tag every node whose
 * text looks like a file path with `.x-file-link` + a
 * `data-file-path` attribute. The shell installs one delegated
 * click listener that intercepts these and posts `open-file` to
 * the host.
 *
 * Targets:
 *   - `<code>` spans whose body is exactly a file path. Agents
 *     wrap paths in backticks reliably, so this catches the
 *     common case (`docs/spec.md`, `src/lib/foo.rs`).
 *   - `<a>` anchors whose href was stripped by the sanitizer's
 *     "no relative URLs" rule but whose text content is a file
 *     path. Re-routes markdown-style `[label](docs/x.md)` links.
 */
function linkifyFilePaths(root: ParentNode): void {
  // Inline `<code>` paths. Inside `<pre>` is a code block -- leave
  // those alone; clicking a path inside a fenced block shouldn't
  // navigate.
  const codeNodes = root.querySelectorAll("code");
  for (const code of Array.from(codeNodes)) {
    if (code.closest("pre")) {
      continue;
    }
    const text = code.textContent ?? "";
    if (!looksLikeFilePath(text)) {
      continue;
    }
    code.classList.add("x-file-link");
    code.setAttribute("data-file-path", text.trim());
    code.setAttribute("role", "link");
    code.setAttribute("tabindex", "0");
    code.setAttribute("title", `Open ${text.trim()} in the editor`);
  }
  // Anchors the sanitizer stripped because the href was relative.
  const anchors = root.querySelectorAll("a");
  for (const a of Array.from(anchors)) {
    if (a.getAttribute("href")) {
      continue;
    }
    const text = a.textContent ?? "";
    if (!looksLikeFilePath(text)) {
      continue;
    }
    a.classList.add("x-file-link");
    a.setAttribute("data-file-path", text.trim());
    a.setAttribute("role", "link");
    a.setAttribute("tabindex", "0");
    a.setAttribute("title", `Open ${text.trim()} in the editor`);
  }
}

function buildComposer(state: ChatPanelState): HTMLElement {
  const root = div("x-composer");
  // The composer stacks two (or three) rows inside one bordered
  // container:
  //   1. textarea (+ Browse on DM0)
  //   2. controls: [▶ play | step rail | Manual/Auto | Send/Stop]
  //   3. milestone+task hint (only when a milestone-driven step is
  //      processing a pending task)
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
    // stays disabled until the next render.
    const s = ui.state;
    if (!s) {
      return;
    }
    const live = document.querySelector<HTMLButtonElement>("#x-composer-send");
    if (live) {
      live.disabled = s.isStreaming ? !s.canStop : !canSend(s);
    }
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

  const inputRow = div("x-composer-input-row");
  // Browse… is only meaningful while the orchestrator is in DM0
  // (the spec-ingest step). Other steps don't take a file path as
  // their primary input, so the button just adds clutter there.
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
    inputRow.append(area, browseBtn);
  } else {
    inputRow.append(area);
  }
  root.appendChild(inputRow);
  root.appendChild(buildComposerControls(state));
  if (state.currentMilestone) {
    const sub = div("x-step-rail-substep");
    sub.textContent = `${state.currentMilestone.title}: ${state.currentMilestone.task}`;
    sub.title = sub.textContent;
    root.appendChild(sub);
  }
  return root;
}

/**
 * Consolidated control row anchored at the bottom of the composer:
 *   [▶ play] [step rail] [Manual/Auto] [Send/Stop]
 * - Play forwards `ContinueFlow` to the orchestrator (icon only;
 *   the orchestrator picks the next action, so a single glyph
 *   suffices). Disabled when there's no next action.
 * - Rail (built by `buildStepRail`) takes the flex middle slot
 *   and shrinks neighbours when a tile is hovered open.
 * - Manual/Auto toggles step mode.
 * - Send/Stop submits the draft (or cancels the current activity).
 */
function buildComposerControls(state: ChatPanelState): HTMLElement {
  const root = div("x-composer-controls");
  const modeDisabled = state.currentStepMode === null;

  // ---- Play (formerly the Continue text button) ----
  const playBtn = document.createElement("button");
  playBtn.type = "button";
  playBtn.id = "x-composer-play";
  playBtn.className = "x-play";
  playBtn.innerHTML = `<i class="codicon codicon-play" aria-hidden="true"></i>`;
  const hintLabel = state.nextActionHint?.label ?? null;
  playBtn.disabled = !hintLabel || state.isStreaming || state.isViewer;
  playBtn.setAttribute(
    "aria-label",
    hintLabel ? `Continue: ${hintLabel}` : "Continue",
  );
  playBtn.title = hintLabel
    ? `Continue: ${hintLabel}`
    : "Continue the flow from its current position. Available when the orchestrator is parked in Manual mode between sub-sessions.";
  playBtn.addEventListener("click", () => {
    if (playBtn.disabled) {
      return;
    }
    send({ type: "continue-flow" });
  });
  root.appendChild(playBtn);

  // ---- Step rail (middle, flex-grow) ----
  const rail = buildStepRail(state);
  if (rail) {
    root.appendChild(rail);
  } else {
    // Spacer keeps the row's columns stable when no flow is
    // anchored yet -- play stays on the left edge, mode + send
    // stay on the right edge.
    root.appendChild(div("x-composer-controls-spacer"));
  }

  // ---- Manual / Auto toggle ----
  const isAuto = state.currentStepMode === "auto";
  const modeBtn = document.createElement("button");
  modeBtn.type = "button";
  modeBtn.id = "x-composer-mode";
  modeBtn.className = "x-mode-toggle";
  modeBtn.textContent = isAuto ? "Auto" : "Manual";
  modeBtn.disabled = modeDisabled;
  modeBtn.title = isAuto
    ? "Auto (orchestrator runs sub-sessions to completion). Click to switch to Manual."
    : "Manual (orchestrator parks between sub-sessions; click Play to advance). Click to switch to Auto.";
  modeBtn.addEventListener("click", () => {
    if (modeBtn.disabled) {
      return;
    }
    const live = ui.state?.currentStepMode;
    send({ type: "set-step-mode", mode: live === "auto" ? "manual" : "auto" });
  });
  root.appendChild(modeBtn);

  // ---- Send / Stop button ----
  const sendBtn = document.createElement("button");
  sendBtn.type = "button";
  sendBtn.id = "x-composer-send";
  sendBtn.className = state.isStreaming ? "x-send x-send-stop" : "x-send";
  sendBtn.textContent = state.isStreaming ? "■" : "↑";
  sendBtn.setAttribute(
    "aria-label",
    state.isStreaming ? "Stop the current activity" : "Send message",
  );
  sendBtn.title = state.isStreaming
    ? "Stop the current activity and drop to Manual mode. The session stays attached -- this is not End session."
    : "Send";
  sendBtn.disabled = state.isStreaming ? !state.canStop : !canSend(state);
  sendBtn.addEventListener("click", () => {
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
    if (!canSend(s)) {
      return;
    }
    submitPrompt();
  });
  root.appendChild(sendBtn);

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

// ---------------------------------------------------------------
// Step rail
// ---------------------------------------------------------------

/**
 * Short title shown in the rail tooltip and (informationally) in
 * the help popup. Source: the dashboard's DM_STEPS array.
 */
const STEP_LABELS: Record<string, string> = {
  DM0: "Spec",
  DM1: "Setup",
  DM2a: "Decomp",
  DM2b: "Pipeline",
  DM2c: "ImplPlan",
  DM2d: "Model",
  DM3a: "TestPlan",
  DM3b: "Bench",
  DM3c: "Tests",
  DM4a: "PerfPlan",
  DM4b: "Perf",
  DS0: "Spec",
  DS1: "Setup",
  DS2: "Decomp",
  DS3a: "Outline",
  DS3b: "Bench",
  DS3c: "Tests",
  DS4: "Screen",
  DS5: "Compare",
};

/**
 * Full step labels surfaced on hover in the step rail. Replaces the
 * 3-4 character step id with a `<id>: <descriptive name>` form so
 * the user can mouse over any tile and read what it is without
 * leaving the chat panel. CSS shrinks the other tiles to make room.
 */
const STEP_FULL_LABELS: Record<string, string> = {
  DM0: "DM0: Specification Intake",
  DM1: "DM1: Modeling Setup",
  DM2a: "DM2a: Design Decomposition",
  DM2b: "DM2b: Pipeline Mapping",
  DM2c: "DM2c: Implementation Plan",
  DM2d: "DM2d: Model Execution",
  DM3a: "DM3a: Test Plan",
  DM3b: "DM3b: Testbench Build",
  DM3c: "DM3c: Test Execution",
  DM4a: "DM4a: Performance Plan",
  DM4b: "DM4b: Performance Execution",
};

/**
 * One-paragraph help text per step. Authored to be readable as a
 * standalone description; the help popup renders all entries in
 * the order returned by `stepOrderFor(flow)`. Keep these short --
 * the popup is a quick reference, not the canonical docs.
 */
const STEP_DESCRIPTIONS: Record<string, { title: string; body: string }> = {
  DM0: {
    title: "DM0 — Specification",
    body:
      "Ingest the user-supplied spec (markdown or PDF) into `docs/spec.md` / `docs/spec/`. The agent asks clarifying questions until the spec declares a clock frequency and an explicit gates-per-cycle budget. Critique gate passes when no blockers remain.",
  },
  DM1: {
    title: "DM1 — Modeling Setup",
    body:
      "Translate the spec into engineering targets and pick a UVM-lite testbench shape. Outputs `docs/targets.md` (quantitative targets) and `docs/testbench.md` (sequencer / driver / monitor / scoreboard plus a `lib:examples/<NN-name>` baseline DM3b will mirror).",
  },
  DM2a: {
    title: "DM2a — Decomposition",
    body:
      "Break the design into named operations under `docs/analysis/decomposition.md` and characterize each with a data-movement summary in `docs/analysis/data-movement.md`. Every operation that DM2b will map to pipeline stages must appear here.",
  },
  DM2b: {
    title: "DM2b — Pipeline Mapping",
    body:
      "Assign each operation to a pipeline stage in `docs/analysis/pipeline-mapping.md`. Defines the in-order shape DM2c's implementation plan and DM2d's model will follow.",
  },
  DM2c: {
    title: "DM2c — Implementation Plan",
    body:
      "Break the modeling work into milestones under `docs/impl-plan/milestone-NN-*.md`, each with a checklist that DM2d will tick off as the model is implemented. The milestone files are this step's only output.",
  },
  DM2d: {
    title: "DM2d — Model Execution",
    body:
      "Implement the SystemVerilog model milestone-by-milestone, ticking off `- [x]` entries in each `milestone-NN-*.md` as code lands. Critique runs between milestones; the gate clears once every milestone is fully resolved.",
  },
  DM3a: {
    title: "DM3a — Test Plan",
    body:
      "Outline testbench scaffolding (`tb-milestone-NN-*.md`) and per-operation test sequences (`test-milestone-NN-*.md`) under `docs/test-plan/`. Both prefixes feed one pipeline that DM3b and DM3c walk in order.",
  },
  DM3b: {
    title: "DM3b — Testbench Build",
    body:
      "Implement the UVM-lite testbench components named in DM1's `docs/testbench.md`, ticking off the `tb-milestone-NN-*.md` rows. Lands the agents, scoreboard, and `SimEnvBuilder` wiring DM3c's tests will exercise.",
  },
  DM3c: {
    title: "DM3c — Test Execution",
    body:
      "Run the per-operation tests scaffolded in DM3a's `test-milestone-NN-*.md`. Failures route back through critique; the gate clears once every test milestone is resolved.",
  },
  DM4a: {
    title: "DM4a — Performance Plan",
    body:
      "Plan the perf experiments under `docs/perf-plan/perf-milestone-NN-*.md`. Each stub names the workload, the metric of interest, and how a run should be invoked.",
  },
  DM4b: {
    title: "DM4b — Performance Execution",
    body:
      "Execute each perf milestone, recording at least one run in `experiments.db`. The gate inspects experiment artifacts and clears once the perf plan is fully covered.",
  },
};

/**
 * Render the horizontal step rail that lives in the composer's
 * controls row. Each tile is half the height of the Send button
 * and just wide enough for a four-character step id; on hover the
 * tile expands to the full descriptive label and neighbours
 * flex-shrink to give up width. Returns null when there's no
 * anchored project (no flow declared yet -> nothing to show).
 *
 * Visual rules:
 *   - current step  -> filled, brightest palette colour
 *   - passed step   -> filled, second-brightest palette colour
 *   - pending step  -> outline-only, transparent fill
 */
function buildStepRail(state: ChatPanelState): HTMLElement | null {
  if (!state.flow) {
    return null;
  }
  const order = stepOrderFor(state.flow);
  const passed = new Set(state.passedSteps);
  const rail = div("x-step-rail");
  rail.setAttribute("role", "tablist");
  rail.setAttribute("aria-label", "Flow step rail");
  for (const stepId of order) {
    const tile = document.createElement("button");
    tile.type = "button";
    let cls = "x-step-rail-step";
    let title: string;
    if (stepId === state.currentStep) {
      cls += " x-step-rail-step-current";
      title = `${STEP_LABELS[stepId] ?? stepId}: current step. Click for the latest critique findings.`;
    } else if (passed.has(stepId)) {
      cls += " x-step-rail-step-passed";
      title = `${STEP_LABELS[stepId] ?? stepId}: gate passed. Click for that step's critique findings.`;
    } else {
      cls += " x-step-rail-step-pending";
      title = `${STEP_LABELS[stepId] ?? stepId}: not yet completed. Click for any critique findings on disk.`;
    }
    tile.className = cls;
    // Two labels share the tile: a short one (just the step id) for
    // the default narrow tile, and a full one (`DM0: Specification
    // Intake`) revealed on hover. CSS swaps which span is `display:
    // inline` so the layout reflows naturally when the hovered tile
    // widens.
    const short = document.createElement("span");
    short.className = "x-step-rail-step-short";
    short.textContent = stepId;
    const full = document.createElement("span");
    full.className = "x-step-rail-step-full";
    full.textContent = STEP_FULL_LABELS[stepId] ?? stepId;
    tile.append(short, full);
    tile.title = title;
    tile.setAttribute("aria-label", title);
    tile.addEventListener("click", () => {
      // Same-tile second click closes; otherwise switch which
      // step's critique is shown.
      if (
        ui.openPopup &&
        ui.openPopup.kind === "critique" &&
        ui.openPopup.step === stepId
      ) {
        ui.openPopup = null;
      } else {
        ui.openPopup = { kind: "critique", step: stepId };
        if (!ui.critiqueData.has(stepId)) {
          send({ type: "open-critique-popup", step: stepId });
        }
      }
      render();
    });
    tile.addEventListener("contextmenu", (event) => {
      // Right-click opens a custom HTML context menu at the cursor
      // position. The native context menu would be a webview-level
      // copy/paste menu, which isn't useful here; preventDefault
      // suppresses it so only our menu shows.
      event.preventDefault();
      ui.openContextMenu = {
        step: stepId,
        x: event.clientX,
        y: event.clientY,
      };
      render();
    });
    rail.appendChild(tile);
  }
  return rail;
}

// ---------------------------------------------------------------
// Popups (help + critique)
// ---------------------------------------------------------------

function buildPopup(state: ChatPanelState): HTMLElement | null {
  const open = ui.openPopup;
  if (!open) {
    return null;
  }
  const root = div("x-popup-backdrop");
  // Click outside the panel closes the popup. The inner panel
  // stops propagation so clicks on its content don't dismiss.
  root.addEventListener("click", () => {
    ui.openPopup = null;
    render();
  });
  const panel = div("x-popup-panel");
  panel.addEventListener("click", (e) => e.stopPropagation());
  const header = div("x-popup-header");
  const title = document.createElement("h2");
  title.className = "x-popup-title";
  const closeBtn = document.createElement("button");
  closeBtn.type = "button";
  closeBtn.className = "x-popup-close";
  closeBtn.innerHTML = `<i class="codicon codicon-close" aria-hidden="true"></i>`;
  closeBtn.setAttribute("aria-label", "Close");
  closeBtn.title = "Close";
  closeBtn.addEventListener("click", () => {
    ui.openPopup = null;
    render();
  });
  if (open.kind === "help") {
    title.textContent = "Step guide";
    header.append(title, closeBtn);
    panel.appendChild(header);
    panel.appendChild(buildHelpBody(state));
  } else {
    title.textContent = `${open.step} — critique`;
    header.append(title, closeBtn);
    panel.appendChild(header);
    panel.appendChild(buildCritiqueBody(open.step));
  }
  root.appendChild(panel);
  return root;
}

function buildHelpBody(state: ChatPanelState): HTMLElement {
  const body = div("x-popup-body");
  // Render descriptions in flow order so DM and DS users get the
  // canonical ordering of their pipeline. Falls back to every
  // known DM step when no flow is anchored yet -- the user can
  // still read the help up-front before launching a session.
  const order = state.flow
    ? stepOrderFor(state.flow)
    : Object.keys(STEP_DESCRIPTIONS);
  for (const stepId of order) {
    const entry = STEP_DESCRIPTIONS[stepId];
    if (!entry) {
      continue;
    }
    const row = div("x-popup-step");
    const heading = document.createElement("h3");
    heading.className = "x-popup-step-title";
    heading.textContent = entry.title;
    const para = document.createElement("p");
    para.className = "x-popup-step-body";
    para.textContent = entry.body;
    row.append(heading, para);
    body.appendChild(row);
  }
  return body;
}

function buildCritiqueBody(step: string): HTMLElement {
  const body = div("x-popup-body");
  const cached = ui.critiqueData.get(step);
  if (cached === undefined) {
    // Request is still in flight (or the popup mounted before the
    // first reply landed). Show a brief loading state; the next
    // render will replace this once `critique-data` arrives.
    body.appendChild(div("x-popup-empty", "Loading critique…"));
    return body;
  }
  if (cached === null || cached.findings.length === 0) {
    body.appendChild(
      div(
        "x-popup-empty",
        `No critique findings on disk for ${step} yet. Run the critique sub-session to generate one.`,
      ),
    );
    return body;
  }
  const blockers = cached.findings.filter((f) => f.kind === "blocker");
  const unresolved = cached.findings.filter((f) => f.kind === "unresolved");
  const resolved = cached.findings.filter((f) => f.kind === "resolved");
  appendFindingSection(body, "Blockers", blockers, "x-popup-finding-blocker");
  appendFindingSection(
    body,
    "Unresolved",
    unresolved,
    "x-popup-finding-unresolved",
  );
  appendFindingSection(body, "Resolved", resolved, "x-popup-finding-resolved");
  return body;
}

function appendFindingSection(
  body: HTMLElement,
  label: string,
  findings: Finding[],
  cls: string,
): void {
  if (findings.length === 0) {
    return;
  }
  const section = div("x-popup-finding-section");
  const heading = document.createElement("h3");
  heading.className = "x-popup-finding-heading";
  heading.textContent = `${label} (${findings.length})`;
  section.appendChild(heading);
  const list = document.createElement("ul");
  list.className = "x-popup-finding-list";
  for (const f of findings) {
    const item = document.createElement("li");
    item.className = cls;
    item.textContent = f.text.length > 0 ? f.text : "(no description)";
    list.appendChild(item);
  }
  section.appendChild(list);
  body.appendChild(section);
}

// ---------------------------------------------------------------
// Right-click context menu on rail tiles
// ---------------------------------------------------------------

/**
 * Render the rail-tile right-click context menu as a fixed-position
 * overlay. Returns null when no menu is open. The backdrop catches
 * a click anywhere outside the menu and closes it; the menu itself
 * stops propagation so clicks on its items don't dismiss before the
 * handler runs.
 */
function buildContextMenu(): HTMLElement | null {
  const open = ui.openContextMenu;
  if (!open) {
    return null;
  }
  const backdrop = div("x-ctxmenu-backdrop");
  backdrop.addEventListener("click", () => {
    ui.openContextMenu = null;
    render();
  });
  backdrop.addEventListener("contextmenu", (e) => {
    // A second right-click anywhere outside the menu also closes
    // it. Suppress the default native menu so the user gets one
    // consistent dismiss gesture.
    e.preventDefault();
    ui.openContextMenu = null;
    render();
  });
  const menu = div("x-ctxmenu");
  menu.style.left = `${open.x}px`;
  menu.style.top = `${open.y}px`;
  menu.setAttribute("role", "menu");
  menu.addEventListener("click", (e) => e.stopPropagation());

  const resetItem = document.createElement("button");
  resetItem.type = "button";
  resetItem.className = "x-ctxmenu-item";
  resetItem.setAttribute("role", "menuitem");
  resetItem.textContent = `Reset from ${open.step}…`;
  resetItem.title = `Reset \`${open.step}\` and every step after it. A confirmation dialog lists every step that will be reset before anything is touched.`;
  resetItem.addEventListener("click", () => {
    const step = open.step;
    ui.openContextMenu = null;
    send({ type: "reset-from-step", step });
    render();
  });
  menu.appendChild(resetItem);

  backdrop.appendChild(menu);
  return backdrop;
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
