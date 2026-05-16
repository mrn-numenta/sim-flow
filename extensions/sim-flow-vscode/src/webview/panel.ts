// Browser-side script for the Flow Dashboard webview. Plain DOM, no
// framework. Compiled with a webview-specific tsconfig that targets
// ES2022 and emits a bare IIFE (no module syntax) so it can be loaded
// with a <script> tag under a strict CSP.
//
// `morphdom` is the only runtime import. Every other import is a
// type-only import that erases at compile time.

import morphdom from "morphdom";

import type { DashboardState, HostMessage, WebviewMessage } from "./messages";
import type { BaselineRecord, RunRow } from "../cli/types";
import type { PromptListEntry } from "./messages";

declare function acquireVsCodeApi(): {
  postMessage(msg: WebviewMessage): void;
  setState(state: unknown): void;
  getState<T = unknown>(): T | undefined;
};

const vscode = acquireVsCodeApi();

interface UiState {
  data: DashboardState | null;
  activeTab: TabId;
  lastError: { message: string; detail?: string } | null;
  /** SVG markup for the block diagram, or null when missing / unloaded. */
  blockDiagramSvg: string | null;
  prompts: PromptListEntry[] | null;
  /**
   * IDs of actions currently in flight. Buttons rendered with one of
   * these IDs render disabled and append " ..." to the label so the
   * user knows their click was registered. Entries are cleared by
   * specific host responses or by a 5s failsafe timer for actions
   * that don't have an explicit completion event.
   */
  pendingActions: Set<string>;
}

type TabId =
  | "experiments"
  | "baselines"
  | "sweeps"
  | "prompts"
  | "block-diagram";

const ui: UiState = {
  data: null,
  activeTab: "prompts",
  lastError: null,
  blockDiagramSvg: null,
  prompts: null,
  pendingActions: new Set<string>(),
};

// --------------------------------------------------------------
// Message wiring
// --------------------------------------------------------------

window.addEventListener("message", (ev) => {
  const msg = ev.data as HostMessage;
  if (!msg || typeof msg.type !== "string") {
    return;
  }
  switch (msg.type) {
    case "state-update":
      ui.data = msg.state;
      ui.lastError = null;
      render();
      return;
    case "block-diagram":
      ui.blockDiagramSvg = msg.svg;
      ui.pendingActions.delete("regenerate-block-diagram");
      render();
      return;
    case "prompts-list-result":
      ui.prompts = msg.entries;
      // Clear all prompt-related pending IDs; list-result is the
      // canonical "settled" signal for any prompt operation.
      for (const id of Array.from(ui.pendingActions)) {
        if (id.startsWith("prompt-")) {
          ui.pendingActions.delete(id);
        }
      }
      render();
      return;
    case "error":
      ui.lastError = { message: msg.message, detail: msg.detail };
      render();
      return;
  }
});

function send(msg: WebviewMessage): void {
  // Clear any stale error from a previous action when the user issues
  // a new one -- otherwise an error from "Run Step" stays pinned
  // even after a successful "Run / Resume" click. Render the cleared
  // banner immediately so the user sees the dismiss before the new
  // action's effects roll in.
  if (ui.lastError !== null && shouldClearErrorOnSend(msg.type)) {
    ui.lastError = null;
    render();
  }
  vscode.postMessage(msg);
}

/** Action types that should clear a pinned error banner when sent. */
function shouldClearErrorOnSend(type: WebviewMessage["type"]): boolean {
  return (
    type === "run-step" ||
    type === "run-critique" ||
    type === "run-auto" ||
    type === "gate-step" ||
    type === "advance-step" ||
    type === "reset-step" ||
    type === "open-document" ||
    type === "regenerate-block-diagram"
  );
}

// Announce readiness so the host sends the first state snapshot.
window.addEventListener("DOMContentLoaded", () => {
  installTabDelegationOnce();
  send({ type: "ready" });
});

// --------------------------------------------------------------
// Rendering
// --------------------------------------------------------------

// `render()` is called from many places (message handlers, button
// clicks, watchers). During a live e2e run the host pushes
// `state-update` rapidly enough that immediate per-call rebuilds
// produce visible flicker AND tear down per-button click listeners
// in the gap between mousedown and mouseup, dropping clicks.
// Coalesce via requestAnimationFrame so the DOM rebuilds at most
// once per frame. Sub-16ms perceived latency is fine for UI updates;
// the click race window collapses to a single frame.
let pendingRender: number | null = null;
function render(): void {
  if (pendingRender !== null) {
    return;
  }
  pendingRender = requestAnimationFrame(() => {
    pendingRender = null;
    renderNow();
  });
}

// Per-render click/keydown handler registries. Buttons and other
// interactive nodes register a callback via `bindClick` /
// `bindKeydown` which returns an opaque id; the node is rendered with
// `data-click-id="<id>"` (and/or `data-keydown-id="<id>"`). A single
// delegated listener on `document.body` looks up the id at event time
// and invokes the handler. Because the listener is on the body, it
// survives the per-frame `replaceChildren` rebuild: a mousedown on a
// button followed by a renderNow() that replaces the button still
// produces a click on the new button (or on a common ancestor), and
// the registry has the fresh handler under a new id from the most
// recent render. Without delegation, the original button's listener
// was destroyed mid-click and the browser dropped the event entirely.
let clickHandlers = new Map<string, () => void>();
let keydownHandlers = new Map<string, (ev: KeyboardEvent) => void>();
let nextHandlerId = 0;

function bindClick(handler: () => void): string {
  const id = `c${nextHandlerId++}`;
  clickHandlers.set(id, handler);
  return id;
}

function renderNow(): void {
  const root = document.getElementById("app");
  if (!root) {
    return;
  }
  // Drop the previous render's handlers before rebuilding. The new
  // pass repopulates the maps with the freshly-bound callbacks; the
  // delegated listener on document.body always reads the current map.
  clickHandlers = new Map();
  keydownHandlers = new Map();
  nextHandlerId = 0;

  // Build the next tree off-screen, then ask morphdom to patch the
  // live tree to match it. `replaceChildren` used to be here and was
  // the root cause of the dashboard's interaction bugs: it destroyed
  // every DOM node on every state-update, which (a) restarted hover
  // transitions on `.step` and `.tab` -- the "jumping" / "flicker"
  // symptoms -- and (b) closed any open `<select>` dropdown because
  // the browser closes the dropdown menu when its host element is
  // removed from the DOM. morphdom keeps DOM identity by walking the
  // two trees and only mutating differences; hover/focus/selection/
  // open dropdowns all survive a refresh because the underlying
  // element doesn't move.
  const next = document.createElement("main");
  next.id = "app";
  for (const node of build()) {
    next.appendChild(node);
  }
  morphdom(root, next, {
    // Preserve focus and IME composition state on inputs even when
    // their `value` attribute would otherwise be re-applied. The
    // dashboard mirrors input values into `ui.*` on every keystroke,
    // so the new tree's value == the user's current text; skipping
    // the property write avoids any chance of a cursor-position
    // glitch mid-typing.
    onBeforeElUpdated(fromEl, toEl) {
      if (fromEl.isEqualNode(toEl)) {
        return false;
      }
      return true;
    },
  });
}

function build(): Node[] {
  if (!ui.data) {
    return [el("p", { class: "empty" }, "Loading sim-flow project state...")];
  }
  const children: Node[] = [header(ui.data)];
  if (ui.lastError) {
    children.push(
      el(
        "div",
        { class: "error" },
        el("strong", {}, ui.lastError.message),
        ui.lastError.detail ? el("pre", {}, ui.lastError.detail) : "",
      ),
    );
  }
  children.push(tabs());
  // Render only the active tab's panel. The other eight panels used
  // to be built on every state-update and hidden via CSS; with the
  // dashboard pushing updates on every file watcher tick, that meant
  // ~9x the DOM churn per refresh for content the user couldn't see.
  // Switching tabs re-runs render() (see installTabDelegationOnce)
  // so the newly-visible panel rebuilds on activation.
  children.push(renderActivePanel(ui.data));
  return children;
}

function renderActivePanel(data: DashboardState): HTMLElement {
  switch (ui.activeTab) {
    case "experiments":
      return panel("experiments", renderExperimentsTab(data));
    case "baselines":
      return panel("baselines", renderBaselinesTab(data));
    case "sweeps":
      return panel("sweeps", renderSweepsTab(data));
    case "block-diagram":
      return panel("block-diagram", renderBlockDiagramTab());
    case "prompts":
      return panel("prompts", renderPromptsTab());
  }
}

function header(data: DashboardState): HTMLElement {
  const generated = new Date(data.generatedAt).toLocaleTimeString();
  // Project name = the last path segment of projectDir. The full
  // path stays available in the toolbar tooltip / Documents tab;
  // the title bar prefers the short project name so it doesn't
  // wrap across two lines.
  const projectName = projectNameFromDir(data.projectDir);
  return el(
    "header",
    {},
    el("h1", { title: data.projectDir }, `Sim Flow Dashboard: ${projectName}`),
    // Refresh button removed: the host already pushes a fresh
    // state-update on every relevant disk / pump event (file
    // watcher + sub-session bracket events), so a manual button
    // was redundant and a click during a sub-session can race
    // the watcher's snapshot.
    el(
      "div",
      { class: "toolbar" },
      `flow: ${data.flow.flow}`,
      sep(),
      `current step: ${data.flow.current_step}`,
      sep(),
      `snapshot: ${generated}`,
    ),
  );
}

/** Last path segment of a project dir, with a fallback to the
 *  full path when no separator is present. */
function projectNameFromDir(dir: string): string {
  const trimmed = dir.replace(/[\\/]+$/, "");
  const idx = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (idx < 0) {
    return trimmed;
  }
  return trimmed.slice(idx + 1) || trimmed;
}

function tabs(): HTMLElement {
  const bar = el("nav", { class: "tabs" });
  // The literal "separator" entry splits the bar into two groups
  // separated by a vertical rule:
  //   1. per-project input (Prompts, Block Diagram)
  //   2. cross-run data / analysis (Experiments, Baselines, Sweeps)
  const defs: Array<[TabId, string] | "separator"> = [
    ["prompts", "Prompts"],
    ["block-diagram", "Block Diagram"],
    "separator",
    ["experiments", "Experiments"],
    ["baselines", "Baselines"],
    ["sweeps", "Sweeps"],
  ];
  for (const def of defs) {
    if (def === "separator") {
      bar.appendChild(el("span", { class: "tabs-separator" }));
      continue;
    }
    const [id, label] = def;
    const cls = id === ui.activeTab ? "tab active" : "tab";
    // Tab click handling lives on `document.body` via event
    // delegation (see installTabDelegationOnce) so a click survives
    // the DOM rebuild that happens on every render. `data-tab` is
    // the source of truth for which tab was clicked. The same
    // pattern (via `data-click-id` + the bindClick registry) covers
    // every other interactive node in the dashboard -- per-button
    // addEventListener used to race with state-update redraws and
    // drop clicks intermittently when refreshes ran during the
    // mousedown -> mouseup window.
    const tab = el("button", {
      class: cls,
      "data-tab": id,
    }, label) as HTMLButtonElement;
    bar.appendChild(tab);
  }
  return bar;
}

let delegationInstalled = false;

function installTabDelegationOnce(): void {
  if (delegationInstalled) {
    return;
  }
  delegationInstalled = true;
  document.body.addEventListener("click", (ev) => {
    const startEl = ev.target as HTMLElement | null;
    if (!startEl) {
      return;
    }
    // Tab clicks. `data-tab` lives on the tab button itself; the
    // closest() walk handles clicks that land on a nested span / text
    // node inside the tab.
    const tabEl = startEl.closest<HTMLElement>(".tab[data-tab]");
    if (tabEl) {
      const id = tabEl.getAttribute("data-tab") as TabId | null;
      if (id) {
        ui.activeTab = id;
        render();
      }
      return;
    }
    // Universal click delegation. Any rendered element can opt in by
    // setting `data-click-id="<id>"` where `<id>` was returned by
    // bindClick() during the same render pass. A disabled control
    // intentionally drops the click on the floor so visual feedback
    // and behavior agree.
    const clickEl = startEl.closest<HTMLElement>("[data-click-id]");
    if (!clickEl) {
      return;
    }
    if (
      clickEl instanceof HTMLButtonElement &&
      clickEl.disabled
    ) {
      return;
    }
    if (clickEl.getAttribute("aria-disabled") === "true") {
      return;
    }
    const id = clickEl.getAttribute("data-click-id");
    if (!id) {
      return;
    }
    const handler = clickHandlers.get(id);
    if (handler) {
      handler();
    }
  });
  document.body.addEventListener("keydown", (ev) => {
    const startEl = ev.target as HTMLElement | null;
    if (!startEl) {
      return;
    }
    const node = startEl.closest<HTMLElement>("[data-keydown-id]");
    if (!node) {
      return;
    }
    const id = node.getAttribute("data-keydown-id");
    if (!id) {
      return;
    }
    const handler = keydownHandlers.get(id);
    if (handler) {
      handler(ev);
    }
  });
}

function panel(id: TabId, content: Node[]): HTMLElement {
  const cls = id === ui.activeTab ? "tab-panel active" : "tab-panel";
  const section = el("section", { class: cls }, ...content);
  return section;
}

// --------------------------------------------------------------
// Experiments tab
// --------------------------------------------------------------

function renderExperimentsTab(data: DashboardState): Node[] {
  if (data.runs.length === 0) {
    return [el("h2", {}, "Experiments"), el("p", { class: "empty" }, "No runs recorded yet.")];
  }
  const table = el(
    "table",
    {},
    el(
      "thead",
      {},
      el(
        "tr",
        {},
        el("th", {}, "Run"),
        el("th", {}, "Timestamp"),
        el("th", {}, "Workload"),
        el("th", {}, "Study / Candidate"),
        el("th", {}, "Commit"),
      ),
    ),
  );
  const tbody = el("tbody", {});
  for (const row of data.runs) {
    tbody.appendChild(renderRunRow(row));
  }
  table.appendChild(tbody);
  return [el("h2", {}, `Experiments (${data.runs.length})`), table];
}

function renderRunRow(row: RunRow): HTMLElement {
  const commit = row.git_commit.length > 8 ? row.git_commit.slice(0, 8) : row.git_commit;
  const dirty = row.git_dirty ? " (dirty)" : "";
  return el(
    "tr",
    {},
    el("td", {}, row.run_id),
    el("td", {}, row.timestamp),
    el("td", {}, row.workload ?? "-"),
    el("td", {}, `${row.study ?? "-"} / ${row.candidate ?? "-"}`),
    el("td", {}, commit + dirty),
  );
}

// --------------------------------------------------------------
// Baselines tab
// --------------------------------------------------------------

function renderBaselinesTab(data: DashboardState): Node[] {
  if (data.baselines.length === 0) {
    return [el("h2", {}, "Baselines"), el("p", { class: "empty" }, "No baselines defined.")];
  }
  const table = el(
    "table",
    {},
    el(
      "thead",
      {},
      el("tr", {}, el("th", {}, "Name"), el("th", {}, "Run"), el("th", {}, "Timestamp")),
    ),
  );
  const tbody = el("tbody", {});
  for (const b of data.baselines as BaselineRecord[]) {
    tbody.appendChild(
      el("tr", {}, el("td", {}, b.name), el("td", {}, b.run_id), el("td", {}, b.timestamp)),
    );
  }
  table.appendChild(tbody);
  return [el("h2", {}, "Baselines"), table];
}

// --------------------------------------------------------------
// Sweeps tab (placeholder; M8 fills this in)
// --------------------------------------------------------------

function renderSweepsTab(_data: DashboardState): Node[] {
  return [
    el("h2", {}, "Sweeps"),
    el(
      "p",
      { class: "empty" },
      "Sweep execution and per-variant results land in Phase 8 Milestone 8.",
    ),
  ];
}

// --------------------------------------------------------------
// Block Diagram tab
// --------------------------------------------------------------

function renderBlockDiagramTab(): Node[] {
  const header = el(
    "div",
    { class: "block-diagram-header" },
    el("h2", {}, "Block Diagram"),
    actionButton("Regenerate", "regenerate-block-diagram", () =>
      send({ type: "regenerate-block-diagram" }),
    ),
  );
  const out: Node[] = [header];
  if (ui.blockDiagramSvg && ui.blockDiagramSvg.length > 0) {
    const wrap = el("div", { class: "block-diagram-svg" });
    // The SVG comes from sim-flow's block-diagram render path -- we
    // generated it in-process, so it's already trustworthy markup.
    // Inject directly so the browser parses it as SVG (innerHTML on
    // a div parses children as HTML).
    (wrap as HTMLElement).innerHTML = ui.blockDiagramSvg;
    out.push(wrap);
  } else {
    out.push(
      el(
        "p",
        { class: "empty" },
        "No block diagram yet. Click Regenerate to run `sim-flow block-diagram`, which calls `cargo run -- --dump-netlist-json` and renders an SVG via the workspace block-diagram tool.",
      ),
    );
  }
  return out;
}

// --------------------------------------------------------------
// Prompts tab
// --------------------------------------------------------------

function renderPromptsTab(): Node[] {
  // Lazy fetch on first render of this tab.
  if (ui.prompts === null) {
    send({ type: "prompts-list" });
    return [el("h2", {}, "Prompts"), el("p", { class: "empty" }, "Loading prompts...")];
  }
  return [
    el("h2", {}, "Prompts"),
    el(
      "p",
      { class: "muted" },
      'Per-step instruction prompts. Resolution order: project > global > foundation default. Click "Edit (project)" or "Edit (global)" to open the corresponding override in a regular editor tab. The foundation default is never opened, so it cannot be saved over -- only project / global overrides accept writes. "Reset" deletes the override at that scope.',
    ),
    renderPromptsTable(ui.prompts),
  ];
}

function renderPromptsTable(entries: PromptListEntry[]): HTMLElement {
  const table = el("table", { class: "prompts-table" });
  const head = el(
    "thead",
    {},
    el(
      "tr",
      {},
      el("th", {}, "Slug"),
      el("th", {}, "Kind"),
      el("th", {}, "Active"),
      el("th", {}, "Project"),
      el("th", {}, "Global"),
      el("th", {}, ""),
    ),
  );
  table.appendChild(head);
  const body = el("tbody", {});
  for (const e of entries) {
    const row = el(
      "tr",
      {},
      el("td", {}, el("code", {}, e.slug)),
      el("td", {}, e.kind),
      el("td", { class: `scope ${e.active_scope}` }, e.active_scope),
      el(
        "td",
        {},
        e.project_present
          ? el("span", { class: "ok" }, "yes")
          : el("span", { class: "muted" }, "—"),
      ),
      el(
        "td",
        {},
        e.global_present ? el("span", { class: "ok" }, "yes") : el("span", { class: "muted" }, "—"),
      ),
      el("td", { class: "actions" }, ...rowActions(e)),
    );
    body.appendChild(row);
  }
  table.appendChild(body);
  return table;
}

/**
 * Per-row action buttons: open the project / global override in a
 * regular editor tab, and (when an override is present) reset it.
 * The foundation-default path has no Edit button -- it's intentionally
 * read-only by being unreachable from this UI, which guarantees the
 * user can never save over it.
 */
function rowActions(entry: PromptListEntry): HTMLButtonElement[] {
  const id = `${entry.slug}-${entry.kind}`;
  const buttons: HTMLButtonElement[] = [];
  buttons.push(
    actionButton("Edit (project)", `prompt-open-project-${id}`, () =>
      send({
        type: "prompt-open-in-editor",
        slug: entry.slug,
        kind: entry.kind,
        scope: "project",
      }),
    ),
  );
  buttons.push(
    actionButton(
      "Edit (global)",
      `prompt-open-global-${id}`,
      () =>
        send({
          type: "prompt-open-in-editor",
          slug: entry.slug,
          kind: entry.kind,
          scope: "global",
        }),
      "secondary",
    ),
  );
  if (entry.project_present) {
    buttons.push(
      actionButton(
        "Reset (project)",
        `prompt-reset-project-${id}`,
        () =>
          send({
            type: "prompt-reset",
            slug: entry.slug,
            kind: entry.kind,
            scope: "project",
          }),
        "secondary",
      ),
    );
  }
  if (entry.global_present) {
    buttons.push(
      actionButton(
        "Reset (global)",
        `prompt-reset-global-${id}`,
        () =>
          send({
            type: "prompt-reset",
            slug: entry.slug,
            kind: entry.kind,
            scope: "global",
          }),
        "secondary",
      ),
    );
  }
  return buttons;
}

// --------------------------------------------------------------
// Helpers
// --------------------------------------------------------------

function sep(): HTMLElement {
  const s = el("span", { class: "sep" }, "·");
  (s as HTMLElement).style.opacity = "0.5";
  return s;
}

/**
 * Pending-aware button. While the supplied `actionId` is in
 * `ui.pendingActions` the button renders disabled with a "..."
 * suffix so the user knows their click was registered. On click we
 * add the id, render synchronously to reflect the disabled state,
 * then dispatch the supplied callback (which usually posts a webview
 * message). A 5-second failsafe timer clears the pending entry if
 * no host response cleared it first; in normal flow the host's
 * response handler removes the specific id before that timer fires.
 */
function actionButton(
  label: string,
  actionId: string,
  onClick: () => void,
  variant?: "secondary" | "warning",
): HTMLButtonElement {
  const isPending = ui.pendingActions.has(actionId);
  const b = document.createElement("button");
  if (variant) {
    b.className = variant;
  }
  b.textContent = isPending ? `${label} ...` : label;
  if (isPending) {
    b.disabled = true;
  }
  const handlerId = bindClick(() => {
    if (ui.pendingActions.has(actionId)) {
      return;
    }
    ui.pendingActions.add(actionId);
    render();
    try {
      onClick();
    } catch (err) {
      ui.pendingActions.delete(actionId);
      throw err;
    }
    setTimeout(() => {
      if (ui.pendingActions.has(actionId)) {
        ui.pendingActions.delete(actionId);
        render();
      }
    }, 5000);
  });
  b.setAttribute("data-click-id", handlerId);
  return b;
}

type Attrs = Record<string, string | number>;

function el(tag: string, attrs: Attrs = {}, ...children: (Node | string)[]): HTMLElement {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    node.setAttribute(key, String(value));
  }
  for (const c of children) {
    if (c === "") {
      continue;
    }
    node.append(typeof c === "string" ? document.createTextNode(c) : c);
  }
  return node;
}
