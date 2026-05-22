// Webview-side renderer for the live perf-data panel.
//
// Pure renderer: receives `state-update` messages from the host and
// rebuilds the DOM from scratch. The host does all DB reading and
// diff-CLI invocation; the panel only renders state and forwards
// user actions.

import type {
  HostMessage,
  PerfPanelState,
  PerfRunRow,
  PerfStudyGroup,
  WebviewMessage,
} from "./messages";

declare function acquireVsCodeApi(): {
  postMessage(message: WebviewMessage): void;
  setState(state: unknown): void;
  getState<T = unknown>(): T | undefined;
};

const vscode = acquireVsCodeApi();

interface UiState {
  state: PerfPanelState | null;
  selection: [string | null, string | null];
}

const ui: UiState = {
  state: null,
  selection: [null, null],
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

function render(): void {
  const app = document.getElementById("app");
  if (!app) {
    return;
  }
  if (!ui.state) {
    app.replaceChildren(div("empty", "Waiting for first refresh..."));
    return;
  }
  const state = ui.state;

  const root = el("div");
  root.appendChild(header(state));
  root.appendChild(toolbar(state));

  if (state.databaseAbsent) {
    root.appendChild(
      div(
        "empty",
        "experiments.db does not exist yet. Record a run with `record_run` " +
          "or run a perf plan via `sim-flow perf-run` to populate it.",
      ),
    );
  } else if (state.totalRuns === 0) {
    root.appendChild(
      div("empty", "experiments.db is empty. Records will appear here as runs are logged."),
    );
  } else {
    for (const group of state.groups) {
      root.appendChild(renderStudyGroup(group));
    }
    if (state.ungrouped.length > 0) {
      root.appendChild(renderStudyGroup({ name: "(no study)", runs: state.ungrouped }));
    }
  }

  root.appendChild(renderDiffPanel(state));
  app.replaceChildren(root);
}

function header(state: PerfPanelState): HTMLElement {
  const wrap = el("div", "header");
  const left = el("div");
  const h1 = el("h1");
  h1.textContent = "Perf Runs";
  left.appendChild(h1);
  const meta = el("div", "meta");
  meta.textContent = `${state.totalRuns} run(s) · last refresh ${formatTime(state.lastUpdated)}`;
  left.appendChild(meta);
  wrap.appendChild(left);
  return wrap;
}

function toolbar(_state: PerfPanelState): HTMLElement {
  const wrap = el("div", "toolbar");
  const refresh = el("button") as HTMLButtonElement;
  refresh.textContent = "Refresh";
  refresh.addEventListener("click", () => send({ type: "request-refresh" }));
  wrap.appendChild(refresh);

  const diff = el("button") as HTMLButtonElement;
  diff.textContent = "Diff selected";
  const [lhs, rhs] = ui.selection;
  diff.disabled = !(lhs && rhs && lhs !== rhs);
  diff.addEventListener("click", () => {
    const [l, r] = ui.selection;
    if (l && r && l !== r) {
      send({ type: "run-diff", lhs: l, rhs: r });
    }
  });
  wrap.appendChild(diff);

  const sel = el("span", "selection");
  if (lhs && rhs) {
    sel.textContent = `Selection: ${lhs}  ↔  ${rhs}`;
  } else if (lhs) {
    sel.textContent = `Selection: ${lhs}  ↔  (pick second run)`;
  } else {
    sel.textContent = "Click two run rows to select for diff.";
  }
  wrap.appendChild(sel);

  return wrap;
}

function renderStudyGroup(group: PerfStudyGroup): HTMLElement {
  const wrap = el("div", "group");
  const h2 = el("h2");
  h2.textContent = group.name;
  const count = el("span", "count");
  count.textContent = `${group.runs.length} run(s)`;
  h2.appendChild(count);
  wrap.appendChild(h2);

  const table = el("table");
  const thead = el("thead");
  const headerRow = el("tr");
  for (const label of ["run_id", "workload", "candidate", "parent", "timestamp"]) {
    const th = el("th");
    th.textContent = label;
    headerRow.appendChild(th);
  }
  thead.appendChild(headerRow);
  table.appendChild(thead);

  const tbody = el("tbody");
  for (const row of group.runs) {
    tbody.appendChild(renderRunRow(row));
  }
  table.appendChild(tbody);
  wrap.appendChild(table);
  return wrap;
}

function renderRunRow(row: PerfRunRow): HTMLElement {
  const tr = el("tr", "run-row");
  const [lhs, rhs] = ui.selection;
  if (row.run_id === lhs || row.run_id === rhs) {
    tr.classList.add("selected");
  }
  tr.addEventListener("click", () => toggleSelection(row.run_id));

  const cells: [string, string | null][] = [
    ["run_id", row.run_id],
    ["workload", row.workload],
    ["candidate", row.candidate],
    ["parent", row.parent_run_id],
    ["timestamp", formatTime(row.timestamp)],
  ];
  for (const [_label, value] of cells) {
    const td = el("td");
    if (value === null || value === undefined || value === "") {
      td.textContent = "—";
    } else {
      const code = el("code");
      code.textContent = value;
      td.appendChild(code);
    }
    tr.appendChild(td);
  }
  return tr;
}

function toggleSelection(runId: string): void {
  const [lhs, rhs] = ui.selection;
  if (lhs === runId) {
    ui.selection = [rhs, null];
  } else if (rhs === runId) {
    ui.selection = [lhs, null];
  } else if (!lhs) {
    ui.selection = [runId, rhs];
  } else if (!rhs) {
    ui.selection = [lhs, runId];
  } else {
    // Two already selected: shift -- replace lhs with the old rhs and
    // make the new click the rhs. Gives a natural feel for picking
    // "compare this against the one I just clicked."
    ui.selection = [rhs, runId];
  }
  render();
}

function renderDiffPanel(state: PerfPanelState): HTMLElement {
  const wrap = el("div", "diff-panel");
  if (state.diffStatus) {
    const status = el("div", "diff-status");
    status.textContent = state.diffStatus;
    wrap.appendChild(status);
  }
  if (state.diffMarkdown) {
    const pre = el("pre");
    pre.textContent = state.diffMarkdown;
    wrap.appendChild(pre);
  }
  return wrap;
}

function formatTime(iso: string): string {
  if (!iso) {
    return "";
  }
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) {
    node.className = className;
  }
  return node;
}

function div(className: string, text: string): HTMLElement {
  const node = el("div", className);
  node.textContent = text;
  return node;
}

render();
