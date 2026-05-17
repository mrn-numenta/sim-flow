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
import {
  type ChatCustomPalette,
  type ChatPalette,
  type ChatPanelState,
  type ChatTranscriptEntry,
  CHAT_PALETTE_NAMES,
  DEFAULT_CUSTOM_PALETTE,
  type HostMessage,
  type WebviewMessage,
} from "./messages";
import {
  inferLangFromContent,
  initShiki,
  renderMarkdownFragment,
} from "./renderMarkdown";
import { stripProtocolFences, stripToolCallFencesForDisplay } from "./state";
import {
  STEP_DESCRIPTIONS,
  STEP_FULL_LABELS,
  STEP_LABELS,
} from "./panelExperimental/stepConstants";

declare function acquireVsCodeApi(): {
  postMessage(message: WebviewMessage): void;
  setState(state: unknown): void;
  getState<T = unknown>(): T | undefined;
};

const vscode = acquireVsCodeApi();

type PaletteName = ChatPalette;

const PALETTES: ReadonlyArray<{ value: PaletteName; label: string }> = [
  { value: "default", label: "Default" },
  { value: "autumn", label: "Autumn" },
  { value: "olive", label: "Olive" },
  { value: "sage", label: "Sage" },
  { value: "custom", label: "Custom" },
];

// "default" disables role tinting entirely (no stripe, no bg) so
// the chat panel inherits the editor theme verbatim. New webviews
// start there; users opt in to a tinted palette via the gear-icon
// settings popover.
const DEFAULT_PALETTE: PaletteName = "default";

function isPaletteName(value: unknown): value is PaletteName {
  return (
    typeof value === "string" &&
    (CHAT_PALETTE_NAMES as readonly string[]).includes(value)
  );
}

function isCustomPalette(value: unknown): value is ChatCustomPalette {
  if (!value || typeof value !== "object") return false;
  const v = value as Partial<ChatCustomPalette>;
  return (
    typeof v.input === "string" &&
    typeof v.tool === "string" &&
    typeof v.output === "string" &&
    typeof v.accent === "string"
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
   * Current Custom palette colours. Always populated even when
   * `palette !== "custom"`, so the four pickers in the settings
   * popover always have a value to bind to. Saved alongside the
   * palette name so a quick toggle back to Custom remembers the
   * last set of colours.
   */
  customPalette: ChatCustomPalette;
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
  /**
   * Bubble kinds the next left-click on the collapse/expand toggle
   * should target. "all" covers everything (the default); the
   * remaining kinds let the user filter the toggle to specific
   * roles via the toggle's right-click context menu. "all" plus
   * anything else is mutually-exclusive -- selecting any concrete
   * kind drops "all"; selecting "all" clears the others.
   */
  collapseFilter: Set<CollapseKind>;
  /**
   * Right-click context menu on the collapse toggle. `null` when
   * nothing is open; otherwise carries the click coordinates so
   * the overlay can place itself there.
   */
  openCollapseMenu: { x: number; y: number } | null;
}

type CollapseKind = "all" | "system" | "user" | "assistant" | "tools";

const COLLAPSE_KIND_ORDER: readonly CollapseKind[] = [
  "all",
  "system",
  "user",
  "assistant",
  "tools",
];

const COLLAPSE_KIND_LABELS: Record<CollapseKind, string> = {
  all: "All",
  system: "System",
  user: "User",
  assistant: "Assistant",
  tools: "Tools",
};

/**
 * CSS selector pinning each filter kind to the matching bubble
 * `<details>` element. `system` covers any orchestrator-driven
 * row that isn't a tool; `user` excludes orchestrator-flavoured
 * users (those are the synthetic prompt-stack messages, not
 * keystrokes from the human).
 */
const COLLAPSE_KIND_SELECTORS: Record<Exclude<CollapseKind, "all">, string> = {
  system: ".x-row-system details.x-bubble-details",
  user:
    ".x-row-user:not(.x-row-orchestrator) details.x-bubble-details",
  assistant: ".x-row-assistant details.x-bubble-details",
  tools: ".x-row-tool details.x-bubble-details",
};

interface PersistedState {
  draft?: string;
  palette?: PaletteName;
  customPalette?: ChatCustomPalette;
  collapseFilter?: CollapseKind[];
}

function isCollapseKind(value: unknown): value is CollapseKind {
  return (
    value === "all" ||
    value === "system" ||
    value === "user" ||
    value === "assistant" ||
    value === "tools"
  );
}

const persisted = vscode.getState<PersistedState>();
const ui: UiState = {
  state: null,
  draft: persisted?.draft ?? "",
  pinnedToBottom: true,
  scrollTop: 0,
  palette: isPaletteName(persisted?.palette) ? persisted.palette : DEFAULT_PALETTE,
  customPalette: isCustomPalette(persisted?.customPalette)
    ? persisted.customPalette
    : { ...DEFAULT_CUSTOM_PALETTE },
  bubblesExpanded: true,
  openPopup: null,
  critiqueData: new Map(),
  openContextMenu: null,
  collapseFilter: new Set<CollapseKind>(
    Array.isArray(persisted?.collapseFilter)
      ? persisted.collapseFilter.filter(isCollapseKind)
      : ["all"],
  ),
  openCollapseMenu: null,
};
if (ui.collapseFilter.size === 0) {
  ui.collapseFilter.add("all");
}

applyPalette();
installFileLinkDelegation();

/**
 * Reconcile the webview's local palette state with whatever the
 * host had stored in workspaceState. The host is the source of
 * truth across VS Code restarts; the webview's `vscode.setState`
 * is just a fast-path so the first paint after a panel reload
 * doesn't flash the old palette. When they disagree we trust the
 * host and re-apply.
 */
/**
 * Suppression window for syncPaletteFromHost after a local
 * pushPaletteToHost. workspaceState.update is async, so a state
 * refresh that fires for unrelated reasons (config change, file
 * watcher tick) reads the OLD palette while our latest push is
 * still in flight; without this guard, syncPaletteFromHost would
 * see a "diff" and overwrite the user's most recent drag. 600 ms
 * covers the round-trip comfortably without leaving the panel out
 * of sync if the host ever rejects the write. See chat-panel
 * audit #6 (2026-05-16).
 */
const PALETTE_PUSH_SUPPRESS_MS = 600;
let paletteLastPushAt = 0;

function syncPaletteFromHost(state: ChatPanelState): void {
  if (
    paletteLastPushAt !== 0
    && Date.now() - paletteLastPushAt < PALETTE_PUSH_SUPPRESS_MS
  ) {
    // Local push is still in flight; the host's view of
    // workspaceState may not yet reflect it. Trust the webview's
    // current palette until the suppression window elapses.
    return;
  }
  let changed = false;
  if (state.palette && state.palette !== ui.palette) {
    ui.palette = state.palette;
    changed = true;
  }
  if (
    state.customPalette &&
    (state.customPalette.input !== ui.customPalette.input ||
      state.customPalette.tool !== ui.customPalette.tool ||
      state.customPalette.output !== ui.customPalette.output ||
      state.customPalette.accent !== ui.customPalette.accent)
  ) {
    ui.customPalette = { ...state.customPalette };
    changed = true;
  }
  if (changed) {
    persist();
    applyPalette();
  }
}

function applyPalette(): void {
  document.body.dataset.palette = ui.palette;
  if (ui.palette === "custom") {
    document.body.style.setProperty("--x-palette-input", ui.customPalette.input);
    document.body.style.setProperty("--x-palette-tool", ui.customPalette.tool);
    document.body.style.setProperty(
      "--x-palette-output",
      ui.customPalette.output,
    );
    document.body.style.setProperty(
      "--x-palette-accent",
      ui.customPalette.accent,
    );
  } else {
    document.body.style.removeProperty("--x-palette-input");
    document.body.style.removeProperty("--x-palette-tool");
    document.body.style.removeProperty("--x-palette-output");
    document.body.style.removeProperty("--x-palette-accent");
  }
}

/**
 * One delegated click listener on the document catches every
 * click + keyboard activation on a `.x-file-link` node (those are
 * tagged by `linkifyFilePaths` after each markdown render). The
 * listener survives morphdom rebuilds because it's bound to
 * `document`, not to any node morphdom owns.
 */
let fileLinkDelegationInstalled = false;

function installFileLinkDelegation(): void {
  // Guard against double-install. The module-init call at the top of
  // this file fires once per webview script load, and VS Code's
  // `.html` reassignment for the experimental-UI toggle DOES rebuild
  // the iframe in current versions -- but if a future webview surface
  // ever re-evaluates the script without reloading the document, the
  // delegated `click`/`keydown` listeners would multiply and each
  // x-file-link click would fire `open-file` twice. The guard makes
  // the install idempotent. See chat-panel audit #8 (2026-05-16).
  if (fileLinkDelegationInstalled) {
    return;
  }
  fileLinkDelegationInstalled = true;
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
    syncPaletteFromHost(msg.state);
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
  vscode.setState({
    draft: ui.draft,
    palette: ui.palette,
    customPalette: ui.customPalette,
    collapseFilter: Array.from(ui.collapseFilter),
  });
}

/** Send the current palette + custom colours to the host for
 *  cross-restart persistence. The webview's `vscode.setState`
 *  only survives panel reloads; workspaceState (host-side)
 *  survives VS Code restarts. */
function pushPaletteToHost(): void {
  // Mark the moment of the push so syncPaletteFromHost knows to
  // ignore any stale-state echo for the suppression window above.
  paletteLastPushAt = Date.now();
  send({
    type: "set-palette",
    palette: ui.palette,
    customPalette: ui.customPalette,
  });
}

function render(): void {
  const app = document.getElementById("app");
  if (!app) {
    return;
  }

  // Toggle the body-level `data-show-context-state` flag so the
  // eviction-indicator CSS (`x-bubble-evicted`) only paints when
  // the user has the setting on. Re-applied every render in case
  // the setting just flipped.
  document.body.dataset.showContextState =
    ui.state?.showContextState === true ? "on" : "off";

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
  // Orchestrator-supplied state (notice / parked prompt / idle-Q&A
  // hint / awaiting-user-input cue). The standard panel.ts renders
  // these in its hero block; the experimental panel previously only
  // surfaced state.currentPlaceholder via the textarea placeholder
  // and dropped the rest on the floor, so RequestUserInput parks
  // were invisible. See chat-panel audit #1 (2026-05-16).
  const banner = buildOrchestratorBanner(ui.state);
  if (banner) {
    shell.appendChild(banner);
  }
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
  const collapseMenu = buildCollapseMenu();
  if (collapseMenu) {
    nodes.push(collapseMenu);
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
  const filterScope = describeCollapseFilter();
  toggleBtn.setAttribute(
    "aria-label",
    ui.bubblesExpanded
      ? `Collapse ${filterScope} bubbles`
      : `Expand ${filterScope} bubbles`,
  );
  toggleBtn.title =
    `${ui.bubblesExpanded ? "Collapse" : "Expand"} ${filterScope} message bubbles. ` +
    "Right-click to choose which roles this button targets.";
  toggleBtn.addEventListener("click", () => {
    ui.bubblesExpanded = !ui.bubblesExpanded;
    setFilteredBubblesOpen(ui.bubblesExpanded);
    render();
  });
  toggleBtn.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    ui.openCollapseMenu = { x: event.clientX, y: event.clientY };
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
    // Viewer mode: clicking still dispatches `switch-project`,
    // which tears down the viewer pump and launches a fresh
    // driving session on the chosen project. Title text reflects
    // that explicitly so the click isn't a surprise. See
    // chat-panel audit #13 (2026-05-16).
    projectBtn.title = state.isViewer
      ? "Leave viewer mode and start a fresh sim-flow session on the chosen project. Detaches the current viewer attachment."
      : "Switch the chat panel to a different sim-flow project. Stops the active session and launches a fresh one on the chosen project.";
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
      pushPaletteToHost();
    }
  });
  palLabel.appendChild(select);

  // Custom palette colour pickers. Rendered all the time so the
  // user can prep colours before switching the dropdown to
  // "Custom", but the row is muted (`disabled` attribute on the
  // <fieldset>) while a non-custom palette is active so the
  // accents make it clear the values aren't currently in effect.
  settingsPanel.appendChild(buildCustomPalettePickers());

  // Enable Verilog: checkmark-style row in the same popover. The
  // host writes the change back to `sim-flow.verilog.enabled` and
  // the configuration listener triggers a refresh.
  settingsPanel.appendChild(buildVerilogToggle(state));

  // Show context state: companion checkmark row. When on, transcript
  // turns evicted from the orchestrator's prompt stack render with
  // a red ✗ + tooltip. Default off; the transcript stays
  // full-history regardless of the toggle.
  settingsPanel.appendChild(buildShowContextStateToggle(state));

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
    // Prefer the real context window the host queried from the
    // backend (Anthropic /v1/models, vLLM max_model_len, LM Studio
    // loaded_context_length, Ollama context_length); fall back to
    // the cosmetic constant only when the query was unsupported.
    const windowTokens = state.contextWindow ?? LLM_CONTEXT_WINDOW;
    const fraction = Math.max(0, Math.min(1, usedTokens / windowTokens));
    const pct = Math.round(fraction * 100);
    rightZone.appendChild(
      buildContextPie(fraction, pct, usedTokens, windowTokens),
    );
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
 * Cosmetic fallback for the context-usage pie when the host hasn't
 * yet queried the real window from the backend (Anthropic without
 * an API key, source we haven't wired, query timed out). 128k is
 * a sensible mid-range for frontier models; smaller local models
 * will over-report and larger will under-report -- both informative
 * as a trend.
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
  windowTokens: number,
): HTMLElement {
  const NS = "http://www.w3.org/2000/svg";
  const wrap = document.createElement("span");
  wrap.className = "x-toolbar-context-pie";
  if (pct >= 80) {
    wrap.classList.add("x-toolbar-context-pie-warn");
  }
  const windowSource =
    windowTokens === LLM_CONTEXT_WINDOW
      ? "estimated reference window"
      : "queried from the backend";
  wrap.title =
    `Approximate context usage: ${pct}% (${usedTokens} tokens estimated against a ${formatTokens(windowTokens)}-token window, ${windowSource}). ` +
    "Cumulative across the whole conversation; actual usage per LLM call depends on the model's effective context size.";
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
/**
 * Apply `open` to whichever `<details>` bubbles the current
 * `ui.collapseFilter` covers. "all" → every bubble. Any concrete
 * kind set → only the matching role's rows (via per-kind
 * selectors defined alongside the filter type).
 */
function setFilteredBubblesOpen(open: boolean): void {
  const transcript = document.querySelector<HTMLElement>(".x-transcript");
  if (!transcript) {
    return;
  }
  const selectors = collectCollapseSelectors();
  for (const sel of selectors) {
    const details = transcript.querySelectorAll<HTMLDetailsElement>(sel);
    for (const node of Array.from(details)) {
      node.open = open;
    }
  }
}

/** Selectors for whichever bubble kinds the filter currently
 *  targets. `all` short-circuits to the catch-all selector. */
function collectCollapseSelectors(): string[] {
  if (ui.collapseFilter.has("all")) {
    return ["details.x-bubble-details"];
  }
  const out: string[] = [];
  for (const kind of COLLAPSE_KIND_ORDER) {
    if (kind === "all") {
      continue;
    }
    if (ui.collapseFilter.has(kind)) {
      out.push(COLLAPSE_KIND_SELECTORS[kind]);
    }
  }
  return out;
}

/** Short human-readable description of the active filter, used in
 *  the toggle button's aria-label + tooltip ("All", "User",
 *  "User + Tools", etc.). */
function describeCollapseFilter(): string {
  if (ui.collapseFilter.has("all")) {
    return "all";
  }
  const labels = COLLAPSE_KIND_ORDER.filter(
    (k) => k !== "all" && ui.collapseFilter.has(k),
  ).map((k) => COLLAPSE_KIND_LABELS[k]);
  if (labels.length === 0) {
    return "all";
  }
  return labels.join(" + ");
}

/** Toggle a kind in the filter set, enforcing the "all is
 *  mutually exclusive with concrete kinds" rule. */
function toggleCollapseKind(kind: CollapseKind): void {
  if (kind === "all") {
    ui.collapseFilter = new Set<CollapseKind>(["all"]);
  } else {
    ui.collapseFilter.delete("all");
    if (ui.collapseFilter.has(kind)) {
      ui.collapseFilter.delete(kind);
    } else {
      ui.collapseFilter.add(kind);
    }
    if (ui.collapseFilter.size === 0) {
      ui.collapseFilter.add("all");
    }
  }
  persist();
}

/**
 * Render the four colour pickers for the Custom palette. Their
 * labels (Input / Tool / Output / Accent) match the role each
 * slot drives in the palette anchors at the top of
 * `chat-panel-experimental.css`. The row stays interactive
 * regardless of which palette is currently selected, so a user
 * can preview-edit before switching to Custom -- but a callout
 * makes it clear the values only take effect with Custom active.
 */
function buildCustomPalettePickers(): HTMLElement {
  const root = div("x-toolbar-settings-row");
  const heading = document.createElement("div");
  heading.className = "x-toolbar-settings-sublabel";
  heading.textContent =
    ui.palette === "custom"
      ? "Custom colours"
      : "Custom colours (preview only — switch palette to Custom to apply)";
  root.appendChild(heading);

  const grid = div("x-custom-palette-grid");
  const slots: Array<{ key: keyof ChatCustomPalette; label: string }> = [
    { key: "input", label: "Input" },
    { key: "tool", label: "Tool" },
    { key: "output", label: "Output" },
    { key: "accent", label: "Accent" },
  ];
  for (const slot of slots) {
    const cell = document.createElement("label");
    cell.className = "x-custom-palette-cell";
    const text = document.createElement("span");
    text.textContent = slot.label;
    const picker = document.createElement("input");
    picker.type = "color";
    picker.className = "x-custom-palette-picker";
    picker.value = ui.customPalette[slot.key];
    picker.addEventListener("input", () => {
      ui.customPalette = {
        ...ui.customPalette,
        [slot.key]: picker.value,
      };
      // Apply immediately when Custom is the active palette so
      // the user sees their picks live. Persist regardless so a
      // switch back to Custom remembers the latest set.
      if (ui.palette === "custom") {
        applyPalette();
      }
      persist();
      pushPaletteToHost();
    });
    cell.append(text, picker);
    grid.appendChild(cell);
  }
  root.appendChild(grid);
  return root;
}

/**
 * "Enable Verilog" checkmark row in the settings popover. The
 * leading ✓ glyph is present when enabled and hidden (replaced by
 * matching whitespace so the label position stays stable) when
 * disabled. Clicking the row sends `set-verilog-enabled` to the
 * host; the host writes the workspace setting and a config-change
 * listener triggers a fresh state-update so the SVF rail appears
 * / disappears.
 */
function buildVerilogToggle(state: ChatPanelState): HTMLElement {
  const row = document.createElement("button");
  row.type = "button";
  row.className = "x-toolbar-settings-checkrow";
  const check = document.createElement("span");
  check.className = "x-toolbar-settings-check";
  check.textContent = state.verilogEnabled ? "✓" : " ";
  const label = document.createElement("span");
  label.textContent = "Enable Verilog";
  row.append(check, label);
  row.title = state.verilogEnabled
    ? "SystemVerilog conversion is enabled. Disabling hides the SV rail and treats DM4b as the terminal step."
    : "Show the SystemVerilog conversion rail under the DM rail and let DM4b advance into the SV flow.";
  row.addEventListener("click", () => {
    send({ type: "set-verilog-enabled", enabled: !state.verilogEnabled });
  });
  return row;
}

/**
 * "Show context state" checkmark row. When on, transcript turns
 * the orchestrator has evicted from its prompt stack render with
 * a red ✗ and a tooltip explaining why. Default off so most users
 * see a clean transcript; turn it on to debug what the agent
 * actually still remembers.
 */
function buildShowContextStateToggle(state: ChatPanelState): HTMLElement {
  const row = document.createElement("button");
  row.type = "button";
  row.className = "x-toolbar-settings-checkrow";
  const check = document.createElement("span");
  check.className = "x-toolbar-settings-check";
  check.textContent = state.showContextState ? "✓" : " ";
  const label = document.createElement("span");
  label.textContent = "Show context state";
  row.append(check, label);
  row.title = state.showContextState
    ? "Context-state indicators are on. Evicted turns render with a red ✗ + tooltip explaining the eviction reason. Turn off to see a clean transcript."
    : "Show a red ✗ on transcript turns the orchestrator has evicted from its prompt stack (dedup / mutation / phase boundary / summarization). The transcript itself always keeps the full history.";
  row.addEventListener("click", () => {
    send({
      type: "set-show-context-state",
      enabled: !state.showContextState,
    });
  });
  return row;
}

function buildTranscript(state: ChatPanelState): HTMLElement {
  const root = div("x-transcript");

  if (state.transcript.length === 0 && !state.isStreaming) {
    root.appendChild(
      div("x-empty", "No messages yet. Type below to start a conversation."),
    );
    return root;
  }

  // Build a map of orchestrator-side message id -> eviction reason
  // so each bubble can look up its own status in O(1). The map is
  // tiny (one entry per evicted message), so this is cheap.
  const evictions = new Map<string, string>(state.evictedMessages);

  // Group consecutive entries by step into a collapsible <details>
  // section. Entries without a step (older transcript rows from
  // before per-bubble step tagging, plus cross-step notes like
  // "Session ended") render ungrouped, inline. The current step's
  // section is rendered open by default; prior steps render
  // collapsed so the panel doesn't drown the user in history when
  // they scroll back through a multi-step run. Toggling state is
  // preserved across re-renders by morphdom's `<details>` open-bit
  // guard in render().
  //
  // The transcript array is already in append order, which mirrors
  // the orchestrator's bracket order, so single-pass grouping is
  // sufficient -- no need to reorder.
  let currentGroup: HTMLElement | null = null;
  let currentGroupStep: string | undefined = undefined;
  let groupContent: HTMLElement | null = null;
  const startGroup = (step: string | undefined): void => {
    if (step === undefined) {
      currentGroup = null;
      currentGroupStep = undefined;
      groupContent = null;
      return;
    }
    const isCurrent = state.currentStep === step;
    const details = document.createElement("details");
    details.className = "x-transcript-step-group";
    if (isCurrent) {
      details.setAttribute("open", "");
    }
    details.setAttribute("data-step", step);
    const summary = document.createElement("summary");
    summary.className = "x-transcript-step-summary";
    summary.textContent = isCurrent ? `${step} (current)` : step;
    details.appendChild(summary);
    const content = div("x-transcript-step-body");
    details.appendChild(content);
    root.appendChild(details);
    currentGroup = details;
    currentGroupStep = step;
    groupContent = content;
  };
  const appendToActiveContainer = (node: HTMLElement): void => {
    if (groupContent) {
      groupContent.appendChild(node);
    } else {
      root.appendChild(node);
    }
  };

  for (const entry of state.transcript) {
    if (entry.step !== currentGroupStep) {
      startGroup(entry.step);
    }
    if (entry.kind === "note") {
      appendToActiveContainer(noteRow(entry));
      continue;
    }
    const body = renderableBody(entry);
    // Skip empty non-streaming assistant entries -- those are stale
    // placeholders from a turn that never produced visible text.
    if (entry.kind === "assistant" && body.length === 0 && !entry.streaming) {
      continue;
    }
    const evictionReason = entry.messageId
      ? (evictions.get(entry.messageId) ?? null)
      : null;
    appendToActiveContainer(messageBubble(entry, body, evictionReason));
  }
  // Silence unused-locals lint for the bookkeeping variables; they
  // exist for readability but only their side effects matter.
  void currentGroup;

  // If the orchestrator says streaming but the latest assistant entry
  // hasn't materialised yet (between Generate Work and the first chunk,
  // or during tool-call stretches), synthesize a thinking bubble.
  // The thinking bubble joins the current step's group when one is
  // open (we're mid-bracket); otherwise it falls outside any group.
  if (state.isStreaming && !hasStreamingAssistantTail(state.transcript)) {
    appendToActiveContainer(thinkingBubble());
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

/**
 * Map an orchestrator `ContextEvictionReason` wire string to the
 * user-facing tooltip that appears on hover over an evicted bubble.
 * Keep these short -- the host's native title tooltip displays
 * them and long strings get awkward line wrapping.
 */
function evictionTooltip(reason: string): string {
  switch (reason) {
    case "superseded-by-dedup":
      return "Evicted: a later turn re-read the same path; this body is no longer in the agent's context.";
    case "invalidated-by-mutation":
      return "Evicted: a later turn wrote / edited this path; the cached body no longer reflects disk and is no longer in the agent's context.";
    case "phase-boundary":
      return "Evicted: dropped at a sub-session boundary; no longer in the agent's context.";
    case "ttl-expired":
      return "Evicted: time-to-live elapsed without a citation; no longer in the agent's context.";
    case "agent-forget":
      return "Evicted: the agent explicitly discarded this turn via `forget`.";
    case "summarized-range":
      return "Evicted: replaced by a summary turn earlier in the transcript.";
    case "overflow-trim":
      return "Evicted: trimmed to fit the model's context window.";
    default:
      return `Evicted from context (${reason}).`;
  }
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
  evictionReason: string | null,
): HTMLElement {
  const role = entry.kind === "user" ? "user" : "assistant";
  const orchestrator =
    entry.kind === "user" &&
    typeof entry.meta === "string" &&
    entry.meta.startsWith("orchestrator-");
  const tool = entry.kind === "user" && entry.meta === "orchestrator-tool";
  const system = entry.kind === "user" && entry.meta === "orchestrator-system";
  const evicted = evictionReason !== null;
  const row = div(
    `x-row x-row-${role}${orchestrator ? " x-row-orchestrator" : ""}${
      tool ? " x-row-tool" : ""
    }${system ? " x-row-system" : ""}${evicted ? " x-row-evicted" : ""}`,
  );
  // Stable id so morphdom keeps DOM identity across renders -- this is
  // what makes streaming chunks patch in place instead of rebuilding.
  row.id = `entry-${entry.id}`;
  if (evicted) {
    row.dataset.evictionReason = evictionReason;
    row.title = evictionTooltip(evictionReason);
  }
  const bubble = div(
    `x-bubble x-bubble-${role}${orchestrator ? " x-bubble-orchestrator" : ""}${
      tool ? " x-bubble-tool" : ""
    }${system ? " x-bubble-system" : ""}${evicted ? " x-bubble-evicted" : ""}`,
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

/**
 * Render the orchestrator's parked-state surface: the banner notice,
 * the literal RequestUserInput prompt, the idle-Q&A hint, and an
 * "awaiting input" status pill. Returns `null` when none of the
 * fields carry content (the normal running-state) so buildShell can
 * skip the empty wrapper. Mirrors the rendering blocks in `panel.ts`
 * around lines 247-278 -- the experimental panel was missing all of
 * this (chat-panel audit #1, 2026-05-16) so RequestUserInput parks
 * were invisible to the user.
 */
function buildOrchestratorBanner(state: ChatPanelState): HTMLElement | null {
  const notice = state.notice && state.notice.trim().length > 0 ? state.notice : null;
  const prompt =
    state.currentPrompt && state.currentPrompt.trim().length > 0
      ? state.currentPrompt
      : null;
  const idleHint =
    state.idleQaHint && state.idleQaHint.trim().length > 0 ? state.idleQaHint : null;
  const showAwaiting = state.awaitingUserInput && !prompt;
  if (!notice && !prompt && !idleHint && !showAwaiting) {
    return null;
  }
  const root = div("x-orchestrator-banner");
  if (showAwaiting) {
    const pill = div("x-orchestrator-awaiting");
    pill.textContent = "Waiting on you";
    pill.setAttribute("role", "status");
    pill.setAttribute("aria-live", "polite");
    root.appendChild(pill);
  }
  if (notice) {
    const node = div("x-orchestrator-notice", notice);
    node.setAttribute("role", "status");
    root.appendChild(node);
  }
  if (prompt) {
    const node = div("x-orchestrator-prompt", prompt);
    node.setAttribute("role", "status");
    node.setAttribute("aria-live", "polite");
    root.appendChild(node);
  }
  if (idleHint) {
    const node = div("x-orchestrator-qa-hint", idleHint);
    node.setAttribute("role", "note");
    root.appendChild(node);
  }
  return root;
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
  area.value = ui.draft;
  area.disabled =
    state.isViewer || !state.supportsPromptEntry || state.isStreaming;
  area.placeholder = area.disabled ? "Busy..." : "Available: Send a message...";
  autoResize(area);
  area.addEventListener("input", () => {
    ui.draft = area.value;
    persist();
    autoResize(area);
    // Re-evaluate the send button's disabled state. canSend reads
    // `ui.draft.trim().length`, which only updates here -- not on
    // a host state-update -- so without this hook the click-target
    // stays disabled until the next render. Only the Send variant
    // depends on draft length; the Stop variant follows canStop,
    // which doesn't change with typing.
    const s = ui.state;
    if (!s || s.isStreaming) {
      return;
    }
    const liveSend = document.querySelector<HTMLButtonElement>(
      "#x-composer-send",
    );
    if (liveSend) {
      liveSend.disabled = !canSend(s);
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
  // "Upload Spec" is only meaningful while the orchestrator is in
  // DM0 (the spec-ingest step). Other steps don't take a file path
  // as their primary input, so the button just adds clutter there.
  if (state.currentStep === "DM0") {
    const browseBtn = document.createElement("button");
    browseBtn.type = "button";
    browseBtn.id = "x-composer-browse";
    browseBtn.className = "x-browse";
    browseBtn.textContent = "Upload Spec";
    browseBtn.title =
      "Pick a spec file or directory (markdown / text / PDF) and insert its absolute path into the message.";
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
  // Followup quick-action chips, when the orchestrator parked at a
  // RequestUserInput with named follow-ups attached. Placed between
  // the input row and the action controls so they read as a peer to
  // typing (each chip submits the same UserMessage the textarea
  // would). Hidden in viewer mode -- viewers don't write back to the
  // session. See chat-panel audit #1 (2026-05-16).
  if (
    state.pendingFollowups
    && state.pendingFollowups.length > 0
    && !state.isViewer
  ) {
    const followupsRow = div("x-composer-followups");
    for (const f of state.pendingFollowups) {
      const chip = document.createElement("button");
      chip.type = "button";
      chip.className = "x-composer-followup-chip";
      chip.textContent = f.label;
      chip.title = f.action;
      chip.disabled = state.isStreaming;
      chip.addEventListener("click", () => {
        send({ type: "followup-selected", action: f.action, label: f.label });
      });
      followupsRow.appendChild(chip);
    }
    root.appendChild(followupsRow);
  }
  root.appendChild(buildComposerControls(state));
  if (state.currentMilestone) {
    const ms = state.currentMilestone;
    const progress =
      ms.taskIndex !== null && ms.taskTotal !== null
        ? ` (${ms.taskIndex}/${ms.taskTotal})`
        : "";
    const sub = div("x-step-rail-substep");
    sub.textContent = `${ms.title}: ${ms.task}${progress}`;
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
  // Special case: DM4b has passed AND Verilog generation is enabled
  // -> Continue means "flip the project into the SV flow at SV0".
  // The orchestrator's NextActionHint is null in this state (DM4b
  // is terminal in the DM flow), so we override the disabled +
  // dispatch behaviour rather than relying on hint.
  const passedSet = new Set(state.passedSteps);
  const dm4bConvertReady =
    state.verilogEnabled &&
    state.flow === "direct-modeling" &&
    passedSet.has("DM4b") &&
    !state.isStreaming &&
    !state.isViewer;
  // The hint label is informational only. The orchestrator's
  // `ContinueFlow` handler decides what to run based on `state.toml`
  // + critique -- the chat panel doesn't need the hint to dispatch.
  // Gating on the hint also misses the cold-start race where the
  // orchestrator parks (emitting NextActionHint) BEFORE the chat
  // panel's listener attaches: the bus event is dropped and the
  // button stays disabled even though Continue is perfectly safe.
  // So we let the button enable whenever the session is live and
  // in manual mode; the hint, when present, just sharpens the label.
  const hintLabel = state.nextActionHint?.label ?? null;
  const continueReady =
    state.sessionActive &&
    state.currentStepMode === "manual" &&
    !state.isStreaming &&
    !state.isViewer;
  if (dm4bConvertReady) {
    playBtn.disabled = false;
    playBtn.setAttribute("aria-label", "Convert to SystemVerilog");
    playBtn.title =
      "Convert to SystemVerilog -- runs `sim-flow convert-sv` to flip the project into the SV flow at SV0, then reconnects the pump.";
  } else {
    playBtn.disabled = !continueReady;
    playBtn.setAttribute(
      "aria-label",
      hintLabel ? `Continue: ${hintLabel}` : "Continue",
    );
    playBtn.title = hintLabel
      ? `Continue: ${hintLabel}`
      : "Continue the flow from its current position. The orchestrator decides the next action based on state.toml.";
  }
  playBtn.addEventListener("click", () => {
    if (playBtn.disabled) {
      return;
    }
    if (dm4bConvertReady) {
      send({ type: "convert-to-sv" });
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
  modeBtn.textContent = isAuto ? "Auto mode" : "Manual mode";
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
  // Two distinct elements (different ids + handlers) so morphdom does
  // a full replace when the mode flips, instead of mutating the
  // existing button's attributes while leaving its old click handler
  // attached. Each handler is hardcoded to a single action and is bound
  // off the build-time state, so the user always gets the action that
  // matches the glyph they were looking at when they clicked: a state
  // flip between render and click swaps the element in/out wholesale
  // rather than morphing one button's semantics under the cursor.
  // (See the "manual mode: ignored unexpected host event: Cancel"
  // regression: the old dual-purpose handler read `ui.state` at click
  // time, so a quick LLM dispatch flipping isStreaming mid-click turned
  // the user's send into a stop.)
  const sendBtn = document.createElement("button");
  sendBtn.type = "button";
  if (state.isStreaming) {
    sendBtn.id = "x-composer-stop";
    sendBtn.className = "x-send x-send-stop";
    sendBtn.textContent = "■";
    sendBtn.setAttribute("aria-label", "Stop the current activity");
    sendBtn.title =
      "Stop the current activity and drop to Manual mode. The session stays attached -- this is not End session.";
    sendBtn.disabled = !state.canStop;
    sendBtn.addEventListener("click", () => {
      if (sendBtn.disabled) {
        return;
      }
      send({ type: "stop-conversation" });
    });
  } else {
    sendBtn.id = "x-composer-send";
    sendBtn.className = "x-send";
    sendBtn.textContent = "↑";
    sendBtn.setAttribute("aria-label", "Send message");
    sendBtn.title = "Send";
    sendBtn.disabled = !canSend(state);
    sendBtn.addEventListener("click", () => {
      if (sendBtn.disabled) {
        return;
      }
      submitPrompt();
    });
  }
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
  // Clear the live textarea directly. The morphdom diff in render()
  // skips updates to a focused textarea (the onBeforeElUpdated hook
  // returns false for focused fields so the user's caret position
  // survives state-update churn), which means an Enter-to-send -- the
  // most common submit path -- would leave the just-sent text visible
  // in the textarea. The user then either pressed Enter again
  // (silently no-op, ui.draft is empty) or typed more characters
  // which got concatenated onto the stale text and resubmitted as a
  // single message on the next send. See chat-panel audit #3.
  const liveArea = document.getElementById("x-composer-textarea") as
    | HTMLTextAreaElement
    | null;
  if (liveArea) {
    liveArea.value = "";
    autoResize(liveArea);
  }
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
  // When the user is mid-DMF and Verilog generation is enabled,
  // stack the SV preview rail below the active DMF rail so they
  // can see the pipeline ahead of them. When the project has
  // already flipped to SV (post-DM4b convert-sv), the active
  // rail IS the SV one; nothing extra to show.
  const showSvPreview =
    state.flow === "direct-modeling" && state.verilogEnabled;
  if (!showSvPreview) {
    return renderRailForFlow(state, state.flow, true);
  }
  const stack = div("x-step-rail-stack");
  stack.appendChild(renderRailForFlow(state, "direct-modeling", true));
  stack.appendChild(renderRailForFlow(state, "systemverilog-convert", false));
  return stack;
}

/**
 * Render a single horizontal rail for `flow`. When `active` is
 * true, the rail reflects the orchestrator's truth (current step,
 * passed gates) for the live project; when false, every tile is
 * rendered pending (this is the SV preview shown under DMF when
 * Verilog generation is enabled but the project hasn't flipped
 * yet).
 */
function renderRailForFlow(
  state: ChatPanelState,
  flow: import("../cli/types").Flow,
  active: boolean,
): HTMLElement {
  const order = stepOrderFor(flow);
  const passed = active ? new Set(state.passedSteps) : new Set<string>();
  const currentStep = active ? state.currentStep : null;
  const rail = div("x-step-rail");
  rail.setAttribute("role", "tablist");
  rail.setAttribute(
    "aria-label",
    active ? "Flow step rail" : "Upcoming SystemVerilog step rail",
  );
  if (!active) {
    rail.classList.add("x-step-rail-preview");
  }
  // Pulse the current step whenever the orchestrator is attached
  // but not actively streaming. A working pump reads as static
  // (no need to draw the user's eye); an idle / parked pump
  // pulses so the user notices it's their turn.
  const pulseCurrent =
    active &&
    state.sessionActive &&
    !state.isStreaming &&
    !state.isViewer;
  for (const stepId of order) {
    const tile = document.createElement("button");
    tile.type = "button";
    let cls = "x-step-rail-step";
    let title: string;
    if (stepId === currentStep) {
      cls += " x-step-rail-step-current";
      if (pulseCurrent) {
        cls += " x-step-rail-step-current-pulse";
      }
      title = `${STEP_LABELS[stepId] ?? stepId}: current step. Click for the latest critique findings.`;
    } else if (passed.has(stepId)) {
      cls += " x-step-rail-step-passed";
      title = `${STEP_LABELS[stepId] ?? stepId}: gate passed. Click for that step's critique findings.`;
    } else {
      cls += " x-step-rail-step-pending";
      title = active
        ? `${STEP_LABELS[stepId] ?? stepId}: not yet completed. Click for any critique findings on disk.`
        : `${STEP_LABELS[stepId] ?? stepId}: upcoming. The project flips into the SV flow once DM4b passes and you click Convert to SystemVerilog.`;
    }
    tile.className = cls;
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
      event.preventDefault();
      // No reset menu on the SV preview rail -- there's nothing
      // to reset there yet.
      if (!active) {
        return;
      }
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

/**
 * Render the collapse-toggle right-click menu as a fixed overlay.
 * Multi-select checkboxes for All / System / User / Assistant /
 * Tools; the "all + concrete" exclusivity rule is enforced by
 * `toggleCollapseKind`. The menu stays open while the user
 * checks/unchecks rows; a click outside (or on the gear's escape
 * close affordance) dismisses it.
 */
function buildCollapseMenu(): HTMLElement | null {
  const open = ui.openCollapseMenu;
  if (!open) {
    return null;
  }
  const backdrop = div("x-ctxmenu-backdrop");
  backdrop.addEventListener("click", () => {
    ui.openCollapseMenu = null;
    render();
  });
  backdrop.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    ui.openCollapseMenu = null;
    render();
  });
  const menu = div("x-ctxmenu");
  menu.style.left = `${open.x}px`;
  menu.style.top = `${open.y}px`;
  menu.setAttribute("role", "menu");
  menu.addEventListener("click", (e) => e.stopPropagation());

  for (const kind of COLLAPSE_KIND_ORDER) {
    const item = document.createElement("label");
    item.className = "x-ctxmenu-item x-ctxmenu-check";
    item.setAttribute("role", "menuitemcheckbox");
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.checked = ui.collapseFilter.has(kind);
    checkbox.className = "x-ctxmenu-checkbox";
    const text = document.createElement("span");
    text.textContent = COLLAPSE_KIND_LABELS[kind];
    item.append(checkbox, text);
    item.title =
      kind === "all"
        ? "Target every message bubble (selecting this clears the role checkboxes)."
        : `Target ${COLLAPSE_KIND_LABELS[kind]} bubbles only.`;
    item.addEventListener("click", (e) => {
      e.preventDefault();
      toggleCollapseKind(kind);
      render();
    });
    menu.appendChild(item);
  }

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
