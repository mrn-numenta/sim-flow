// Browser-side script for the Flow Dashboard webview. Plain DOM, no
// framework. Compiled with a webview-specific tsconfig that targets
// ES2022 and emits a bare IIFE (no module syntax) so it can be loaded
// with a <script> tag under a strict CSP.
//
// This file imports only *types* from sibling modules; nothing emits
// at runtime beyond the IIFE itself.

import type { DashboardState, HostMessage, WebviewMessage } from "./messages";
import type { BaselineRecord, GateFailure, GateResult, RunRow } from "../cli/types";
import type { CritiqueFile, Finding } from "../state/types";
import {
  LLM_SOURCE_LABELS,
  type DocumentEntry,
  type LlmSourceTag,
  type PromptListEntry,
} from "./messages";
import { deriveStepActionState, isStepSelectableInRail } from "./stepActions";

declare function acquireVsCodeApi(): {
  postMessage(msg: WebviewMessage): void;
  setState(state: unknown): void;
  getState<T = unknown>(): T | undefined;
};

const vscode = acquireVsCodeApi();

interface UiState {
  data: DashboardState | null;
  activeTab: TabId;
  selectedStep: string | null;
  gateReport: GateResult | null;
  lastError: { message: string; detail?: string } | null;
  specPath: string;
  newProjectName: string;
  llmSource: LlmSourceTag | null;
  /** Active `sim-flow.llm.model` setting; empty string means "any". */
  llmModel: string;
  /** Active `sim-flow.llm.verbose` setting. When false the pump prepends a "be concise" system message. */
  llmVerbose: boolean;
  /**
   * Cached list of models for `llmModelListSource`. We refetch on
   * source change and on a manual refresh click; otherwise we keep
   * showing the cached list.
   */
  llmModelList: string[];
  /** Source the cached model list was fetched for. */
  llmModelListSource: LlmSourceTag | null;
  /** Set while a model-list fetch is in flight (for the refresh button spinner). */
  llmModelListPending: boolean;
  /** Last error / empty reason from a model-list fetch, if any. */
  llmModelListNote: string | null;
  /** SVG markup for the block diagram, or null when missing / unloaded. */
  blockDiagramSvg: string | null;
  prompts: PromptListEntry[] | null;
  /**
   * Webview-local "user has clicked Play in this dashboard session"
   * flag. While false, the step rail and per-step buttons render in
   * a disabled / grayed-out state so the user can't drive the flow
   * without explicitly starting the automated driver. Resets to
   * false on every window reload (intentional -- forces an explicit
   * Play to confirm the user wants the flow live again).
   */
  autoRunning: boolean;
  /**
   * True after we've seeded `specPath` from the persisted server
   * value at least once. The seeding happens only on the very first
   * `state-update` so subsequent updates don't clobber the user's
   * in-progress typing.
   */
  specPathInitialized: boolean;
  /**
   * IDs of actions currently in flight. Buttons rendered with one of
   * these IDs render disabled and append " ..." to the label so the
   * user knows their click was registered. Entries are cleared by
   * specific host responses (e.g. `gate-result` clears the matching
   * gate action) or by a 5s failsafe timer for actions that don't
   * have an explicit completion event (open-chat-tab, etc).
   */
  pendingActions: Set<string>;
}

type TabId =
  | "projects"
  | "settings"
  | "flow"
  | "experiments"
  | "baselines"
  | "sweeps"
  | "prompts"
  | "documents"
  | "block-diagram";

const ui: UiState = {
  data: null,
  activeTab: "flow",
  selectedStep: null,
  gateReport: null,
  lastError: null,
  specPath: "",
  newProjectName: "",
  llmSource: null,
  llmModel: "",
  llmVerbose: true,
  llmModelList: [],
  llmModelListSource: null,
  llmModelListPending: false,
  llmModelListNote: null,
  blockDiagramSvg: null,
  prompts: null,
  pendingActions: new Set<string>(),
  autoRunning: false,
  specPathInitialized: false,
};

const FLOW_LOCKED_REASON =
  "Click Connect first to unlock the step controls for this dashboard session.";

// --------------------------------------------------------------
// Message wiring
// --------------------------------------------------------------

window.addEventListener("message", (ev) => {
  const msg = ev.data as HostMessage;
  if (!msg || typeof msg.type !== "string") {
    return;
  }
  switch (msg.type) {
    case "state-update": {
      // Auto-follow the current step in the buttons pane: if the
      // user was tracking the previous current step (or hasn't
      // selected anything yet), move the selection forward when
      // /advance bumps the flow. If the user manually clicked a
      // different step in the rail, leave their selection alone --
      // they're inspecting that step on purpose.
      const prevCurrent = ui.data?.flow.current_step ?? null;
      const wasTracking = !ui.selectedStep || ui.selectedStep === prevCurrent;
      ui.data = msg.state;
      ui.lastError = null;
      // Fresh state means whatever action prompted this refresh has
      // landed. Clear all pending so any re-enabled button reflects
      // current truth.
      ui.pendingActions.clear();
      if (wasTracking) {
        ui.selectedStep = msg.state.flow.current_step;
        // The selected step changed: drop any stale gate report from
        // the previous step so the new step's pane starts clean.
        if (prevCurrent !== msg.state.flow.current_step) {
          ui.gateReport = null;
        }
      }
      // Seed the spec input from persisted state on first sync.
      // Subsequent edits flow back via `set-spec-path` so the user's
      // typing wins over server state. If the persisted value is
      // empty, we keep whatever the user typed (avoids clobbering
      // their input mid-session).
      if (!ui.specPathInitialized) {
        if (msg.state.specPath.length > 0) {
          ui.specPath = msg.state.specPath;
        }
        ui.specPathInitialized = true;
      }
      render();
      return;
    }
    case "gate-result":
      // Clear gate / advance action pendings for this step.
      ui.pendingActions.delete(`gate-${msg.step}`);
      ui.pendingActions.delete(`advance-${msg.step}`);
      if (ui.selectedStep === msg.step) {
        ui.gateReport = msg.result;
        render();
      } else {
        render();
      }
      return;
    case "spec-path-picked":
      ui.specPath = msg.path;
      // Persist the picked path the same way an inline edit would,
      // so a window reload restores the user's last choice.
      send({ type: "set-spec-path", path: msg.path });
      // Reflect into the input element if it's already mounted; the
      // next render() picks it up via `value`.
      render();
      return;
    case "llm-config": {
      const sourceChanged = ui.llmSource !== msg.source;
      ui.llmSource = msg.source;
      ui.llmModel = msg.model ?? "";
      ui.llmVerbose = msg.verbose;
      // Source changed: drop cached models and request a fresh list.
      // The host's enumerator handles the per-source dispatch.
      if (sourceChanged) {
        ui.llmModelList = [];
        ui.llmModelListSource = msg.source;
        ui.llmModelListPending = true;
        ui.llmModelListNote = null;
        send({ type: "request-model-list", source: msg.source });
      }
      render();
      return;
    }
    case "model-list":
      // Ignore stale responses (e.g. user picked source A then B
      // before A's enumeration finished).
      if (ui.llmSource !== msg.source) {
        return;
      }
      ui.llmModelList = msg.models;
      ui.llmModelListSource = msg.source;
      ui.llmModelListPending = false;
      ui.llmModelListNote = msg.error ?? msg.emptyReason ?? null;
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
      // Any error clears all pending so users aren't left with
      // permanently-disabled buttons.
      ui.pendingActions.clear();
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
  send({ type: "ready" });
});

// --------------------------------------------------------------
// Rendering
// --------------------------------------------------------------

function render(): void {
  const root = document.getElementById("app");
  if (!root) {
    return;
  }
  root.replaceChildren(...build());
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
  children.push(
    panel("projects", renderProjectsTab(ui.data)),
    panel("settings", renderSettingsTab()),
    panel("flow", renderFlowTab(ui.data)),
    panel("experiments", renderExperimentsTab(ui.data)),
    panel("baselines", renderBaselinesTab(ui.data)),
    panel("sweeps", renderSweepsTab(ui.data)),
    panel("documents", renderDocumentsTab(ui.data)),
    panel("block-diagram", renderBlockDiagramTab()),
    panel("prompts", renderPromptsTab()),
  );
  return children;
}

function header(data: DashboardState): HTMLElement {
  const generated = new Date(data.generatedAt).toLocaleTimeString();
  return el(
    "header",
    {},
    el("h1", {}, `sim-flow: ${shortPath(data.projectDir)}`),
    el(
      "div",
      { class: "toolbar" },
      `flow: ${data.flow.flow}`,
      sep(),
      `current step: ${data.flow.current_step}`,
      sep(),
      `snapshot: ${generated}`,
      sep(),
      actionButton("Refresh", "refresh", () => send({ type: "refresh" })),
    ),
  );
}

/**
 * Projects tab: project lifecycle controls (switch, rename, new).
 * Lives at the leftmost edge of the tab bar so the user reaches
 * for it once per project session and otherwise stays in the Flow
 * tab.
 */
function renderProjectsTab(data: DashboardState): Node[] {
  const switchBtn = actionButton("Switch project...", "switch-project", () => {
    send({ type: "switch-project" });
  });
  const renameBtn = actionButton(
    "Rename...",
    "rename-project",
    () => send({ type: "rename-project" }),
    "secondary",
  );

  const nameInput = document.createElement("input");
  nameInput.type = "text";
  nameInput.placeholder = "New project name";
  nameInput.value = ui.newProjectName;
  nameInput.className = "new-project-input";
  nameInput.addEventListener("input", () => {
    ui.newProjectName = nameInput.value;
  });
  nameInput.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" && ui.newProjectName.trim().length > 0) {
      ev.preventDefault();
      submitNewProject();
    }
  });
  const createBtn = actionButton("Create", "new-project", () => submitNewProject());

  return [
    el("h2", {}, "Projects"),
    el("p", { class: "muted" }, `Active: ${shortPath(data.projectDir)}.`),
    el(
      "div",
      { class: "settings-section" },
      el("h3", {}, "Active project"),
      el("div", { class: "settings-row" }, switchBtn, renameBtn),
    ),
    el(
      "div",
      { class: "settings-section" },
      el("h3", {}, "New project"),
      el(
        "p",
        { class: "muted" },
        "Scaffolds a new project under the workspace's sim-models root (`<sim-models>/users/<USER>/<name>`).",
      ),
      el("div", { class: "settings-row" }, nameInput, createBtn),
    ),
  ];
}

/**
 * Settings tab: LLM source / model / verbose controls. These mutate
 * VS Code workspace settings so the choice persists across sessions.
 */
function renderSettingsTab(): Node[] {
  return [
    el("h2", {}, "Settings"),
    el(
      "p",
      { class: "muted" },
      "Per-workspace LLM and dashboard options. Changes are written through to VS Code workspace settings.",
    ),
    el(
      "div",
      { class: "settings-section" },
      el("h3", {}, "Language model"),
      el("div", { class: "settings-row" }, renderLlmSourcePicker(), renderLlmModelPicker()),
    ),
    el(
      "div",
      { class: "settings-section" },
      el("h3", {}, "Output style"),
      el("div", { class: "settings-row" }, renderVerboseToggle()),
    ),
    el(
      "div",
      { class: "settings-section" },
      el("h3", {}, "Dashboard"),
      el(
        "p",
        { class: "muted" },
        "Show the red end-to-end automated-flow button next to Connect / Disconnect on the Flow tab. Hidden by default because the automated flow walks every step without stopping for review and can burn meaningful LLM credits.",
      ),
      el("div", { class: "settings-row" }, renderFullyAutomatedToggle()),
    ),
    el(
      "div",
      { class: "settings-section" },
      el("h3", {}, "Verilog simulation"),
      el(
        "p",
        { class: "muted" },
        "When enabled and a simulator path is set, clicking Generate Verilog also instructs the agent to run the emitted RTL through that simulator, diagnose failures, and iterate the generated SystemVerilog until simulation matches the Foundation model. Path can be an absolute file or a PATH-resolvable command (e.g. `verilator`).",
      ),
      el("div", { class: "settings-row" }, renderVerilogSimToggle()),
      el("div", { class: "settings-row" }, renderVerilogSimulatorPath()),
    ),
  ];
}

function renderVerilogSimToggle(): HTMLElement {
  const wrap = el("label", { class: "llm-verbose" });
  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = ui.data?.verilogSimEnabled === true;
  input.title =
    "When checked AND the simulator path below is non-empty, the Generate Verilog prompt is extended with a 'Simulate and iterate' section that drives the emitted RTL through the simulator and asks the agent to fix failures.";
  input.addEventListener("change", () => {
    send({ type: "set-verilog-sim-enabled", enabled: input.checked });
  });
  wrap.appendChild(input);
  wrap.appendChild(
    document.createTextNode(" Run and debug generated SystemVerilog after emission"),
  );
  return wrap;
}

function renderVerilogSimulatorPath(): HTMLElement {
  const wrap = el("label", { class: "llm-source" }, "Simulator path: ");
  const input = document.createElement("input");
  input.type = "text";
  input.className = "verilog-simulator-path";
  input.placeholder = "/usr/local/bin/verilator";
  input.value = ui.data?.verilogSimulatorPath ?? "";
  input.title =
    "Absolute path or PATH-resolvable command for a SystemVerilog simulator (Verilator, VCS, ModelSim, Xcelium, ...). Saved to `sim-flow.dashboard.verilogSimulatorPath`.";
  // Persist on blur so the user can finish typing without firing a
  // round-trip per keystroke; also persist on Enter for parity with
  // other path fields in the dashboard.
  const commit = (): void => {
    send({ type: "set-verilog-simulator-path", path: input.value.trim() });
  };
  input.addEventListener("blur", commit);
  input.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      commit();
    }
  });
  wrap.appendChild(input);
  return wrap;
}

function renderFullyAutomatedToggle(): HTMLElement {
  const wrap = el("label", { class: "llm-verbose" });
  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = ui.data?.fullyAutomatedEnabled === true;
  input.title =
    "When checked, a red play button appears to the left of the manual ▶ on the Flow tab. " +
    "The red button kicks off `sim-flow auto` end-to-end (the host confirms first).";
  input.addEventListener("change", () => {
    send({ type: "set-fully-auto-enabled", enabled: input.checked });
  });
  wrap.appendChild(input);
  wrap.appendChild(document.createTextNode(" Show fully-automated flow button"));
  return wrap;
}

function renderVerboseToggle(): HTMLElement {
  const wrap = el("label", { class: "llm-verbose" });
  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = ui.llmVerbose;
  input.title =
    "When OFF, the dashboard prepends a `be concise` system message to every LLM request so the model skips preamble, recaps, and hedging language. ON keeps the model's natural verbosity.";
  input.addEventListener("change", () => {
    ui.llmVerbose = input.checked;
    send({ type: "set-llm-verbose", verbose: input.checked });
  });
  wrap.appendChild(input);
  wrap.appendChild(document.createTextNode(" Verbose"));
  return wrap;
}

function renderLlmModelPicker(): HTMLElement {
  const wrap = el("label", { class: "llm-source" }, "Model: ");
  const select = document.createElement("select");
  select.className = "llm-source-select";
  // First option is always the "let the source pick" / unset state.
  // Writing an empty string clears `sim-flow.llm.model` so the
  // backend uses whatever default the source provides.
  const blankOpt = document.createElement("option");
  blankOpt.value = "";
  blankOpt.textContent = ui.llmModelListPending ? "loading..." : "(default)";
  select.appendChild(blankOpt);
  // If the configured model isn't in the fetched list (yet, or
  // ever) keep it as a selectable option so the dropdown reflects
  // settings.json reality.
  const seen = new Set<string>();
  for (const id of ui.llmModelList) {
    seen.add(id);
    const opt = document.createElement("option");
    opt.value = id;
    opt.textContent = id;
    if (ui.llmModel === id) {
      opt.selected = true;
    }
    select.appendChild(opt);
  }
  if (ui.llmModel.length > 0 && !seen.has(ui.llmModel)) {
    const opt = document.createElement("option");
    opt.value = ui.llmModel;
    opt.textContent = `${ui.llmModel} (custom)`;
    opt.selected = true;
    select.appendChild(opt);
  } else if (ui.llmModel.length === 0) {
    blankOpt.selected = true;
  }
  select.disabled = ui.llmModelListPending && ui.llmModelList.length === 0;
  let title = "Active model. Writes through to `sim-flow.llm.model`.";
  if (ui.llmModelListNote) {
    title += `\n${ui.llmModelListNote}`;
  }
  select.title = title;
  select.addEventListener("change", () => {
    ui.llmModel = select.value;
    send({ type: "set-llm-model", model: select.value });
  });
  wrap.appendChild(select);

  // Refresh button -- two-arrow circle (Unicode CSV: U+27F3
  // CLOCKWISE GAPPED CIRCLE ARROW). On click we drop the cache and
  // re-request, with a pending state on the button while the host
  // enumerates.
  const refresh = document.createElement("button");
  refresh.className = "llm-source-refresh";
  refresh.type = "button";
  refresh.textContent = ui.llmModelListPending ? "..." : "⟳";
  refresh.title = "Re-query the active source for available models.";
  refresh.disabled = ui.llmModelListPending || ui.llmSource === null;
  refresh.addEventListener("click", () => {
    if (!ui.llmSource) {
      return;
    }
    ui.llmModelListPending = true;
    ui.llmModelListNote = null;
    send({ type: "request-model-list", source: ui.llmSource });
    render();
  });
  wrap.appendChild(refresh);
  return wrap;
}

function renderLlmSourcePicker(): HTMLElement {
  const wrap = el("label", { class: "llm-source" }, "Agent: ");
  const select = document.createElement("select");
  select.className = "llm-source-select";
  for (const id of Object.keys(LLM_SOURCE_LABELS) as LlmSourceTag[]) {
    const opt = document.createElement("option");
    opt.value = id;
    opt.textContent = LLM_SOURCE_LABELS[id];
    if (ui.llmSource === id) {
      opt.selected = true;
    }
    select.appendChild(opt);
  }
  if (ui.llmSource === null) {
    // Loading state -- disable until the host posts the live value.
    select.disabled = true;
  }
  select.title =
    "Active LLM backend. Changing this here writes through to `sim-flow.llm.source` (workspace scope) and takes effect on the next LLM call -- you can switch mid-run if e.g. tokens are exhausted.";
  select.addEventListener("change", () => {
    const value = select.value as LlmSourceTag;
    ui.llmSource = value;
    // Kick off the source-local model refresh immediately. The host
    // echoes the chosen source back via `llm-config`, but by then the
    // optimistic local `ui.llmSource` update means a pure
    // "did-source-change?" check would not fire.
    ui.llmModel = "";
    ui.llmModelList = [];
    ui.llmModelListSource = value;
    ui.llmModelListPending = true;
    ui.llmModelListNote = null;
    render();
    send({ type: "set-llm-source", source: value });
    send({ type: "request-model-list", source: value });
  });
  wrap.appendChild(select);
  return wrap;
}

function submitNewProject(): void {
  const name = ui.newProjectName.trim();
  if (name.length === 0) {
    return;
  }
  send({ type: "new-project", name });
  ui.newProjectName = "";
}

function tabs(): HTMLElement {
  const bar = el("nav", { class: "tabs" });
  // The literal "separator" entries split the bar into three groups
  // separated by vertical rules:
  //   1. project / global config (Projects, Settings)
  //   2. flow + per-project input (Flow, Prompts, Documents, Block Diagram)
  //   3. cross-run data / analysis (Experiments, Baselines, Sweeps)
  const defs: Array<[TabId, string] | "separator"> = [
    ["projects", "Projects"],
    ["settings", "Settings"],
    "separator",
    ["flow", "Flow"],
    ["prompts", "Prompts"],
    ["documents", "Documents"],
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
    const tab = el("button", { class: cls }, label) as HTMLButtonElement;
    tab.addEventListener("click", () => {
      ui.activeTab = id;
      render();
    });
    bar.appendChild(tab);
  }
  return bar;
}

function panel(id: TabId, content: Node[]): HTMLElement {
  const cls = id === ui.activeTab ? "tab-panel active" : "tab-panel";
  const section = el("section", { class: cls }, ...content);
  return section;
}

// --------------------------------------------------------------
// Flow tab: step rail + selected-step detail
// --------------------------------------------------------------

function renderFlowTab(data: DashboardState): Node[] {
  const rail = el("div", { class: "step-rail" });
  const flowSteps = data.flow.flow === "direct-modeling" ? DM_STEPS : DS_STEPS;
  for (let i = 0; i < flowSteps.length; i++) {
    const step = flowSteps[i];
    const box = stepBox(data, step);
    // The first / last steps get flat outer edges; intermediate
    // steps have triangular cut-outs at both top and bottom that
    // the gate diamonds sit inside. See `.step` in panel.css for
    // the clip-path geometry.
    if (i === 0) {
      box.classList.add("step-first");
    }
    if (i === flowSteps.length - 1) {
      box.classList.add("step-last");
    }
    rail.appendChild(box);
    if (i < flowSteps.length - 1) {
      rail.appendChild(gateDiamond(data, step));
    }
  }
  // Grey out + disable interaction with the step rail and the
  // per-step detail panel until the user has clicked Play. The Run
  // / Resume / Stop controls stay live so the user can start the
  // flow; everything that drives the flow forward (Run Step,
  // Advance, Reset, etc.) is gated by `autoRunning`.
  const guardClass = ui.autoRunning ? "" : "flow-locked";
  const layout = el(
    "div",
    { class: `flow-layout ${guardClass}`.trim() },
    el(
      "div",
      { class: "flow-rail-column", "aria-disabled": ui.autoRunning ? "false" : "true" },
      el("h2", {}, "Step Rail"),
      rail,
    ),
    el("div", { class: "flow-detail-column" }, renderSelectedStepDetail(data)),
  );
  return [renderAutoFlowRow(), el("hr", { class: "flow-row-divider" }), layout];
}

function renderAutoFlowRow(): HTMLElement {
  // Two-row control panel:
  //   row 1: Connect (🔌) / Disconnect (⏻) icon buttons.
  //   row 2: `Spec:` label + path input + Browse... button.
  // Connect launches a sim-flow session (or no-ops if one is already
  // up); it does NOT start running a step on its own -- the user
  // drives the step rail explicitly. Disconnect injects `/exit` over
  // the single-session control socket, telling the running claude
  // TUI to leave; the orchestrator then parks at "waiting for the
  // next dashboard command".
  const connectBtn = actionButton(
    "\u{1F50C}", // 🔌
    "run-auto",
    () => {
      ui.autoRunning = true;
      send({ type: "run-auto", specPath: ui.specPath || undefined });
      // Re-render so the rail / detail unlock immediately rather
      // than waiting for the next state-update.
      render();
    },
  );
  connectBtn.title =
    "Connect to a sim-flow session. Launches the flow if none is running. " +
    "After connecting, the step rail and per-step buttons become active so you can drive each step explicitly.";
  connectBtn.classList.add("auto-run-btn", "auto-connect-btn", "auto-icon-btn");
  applyButtonState(
    connectBtn,
    !ui.autoRunning,
    ui.autoRunning
      ? "Connect is disabled while a session is already attached."
      : connectBtn.title,
  );

  const disconnectBtn = actionButton(
    "\u{23FB}", // ⏻
    "stop-auto",
    () => {
      ui.autoRunning = false;
      send({ type: "stop-auto" });
      render();
    },
    "secondary",
  );
  disconnectBtn.title =
    "Disconnect from the sim-flow session by injecting `/exit` over the control socket. " +
    "The running claude TUI exits cleanly; the orchestrator parks waiting for the next command. " +
    "After Disconnect the step rail re-locks until you Connect again.";
  disconnectBtn.classList.add("auto-stop-btn", "auto-disconnect-btn", "auto-icon-btn");
  applyButtonState(
    disconnectBtn,
    ui.autoRunning,
    ui.autoRunning
      ? disconnectBtn.title
      : "Disconnect is disabled because there is no active session.",
  );

  // End-to-end "automated" red play. Hidden unless the user has
  // explicitly enabled it via the Settings tab checkbox -- it kicks
  // off a long unattended run and we don't want a single misclick
  // to start one.
  const buttonRowChildren: HTMLElement[] = [];
  if (ui.data?.fullyAutomatedEnabled) {
    const fullyAutoBtn = actionButton("▶", "run-auto-end-to-end", () => {
      const spec = ui.specPath.trim();
      if (!spec) {
        ui.lastError = {
          message: "Fully-automated flow needs a spec path.",
          detail: "Type or browse to a spec file in the Spec field above, then click the red play.",
        };
        render();
        return;
      }
      ui.autoRunning = true;
      send({ type: "run-auto-end-to-end", specPath: spec });
      render();
    });
    fullyAutoBtn.title =
      "Fully automated: walk every step (DM0 → DM4b) without stopping for review. " +
      "Requires a spec. Long-running and burns LLM credits; the host shows a confirm modal first.";
    fullyAutoBtn.classList.add("auto-fully-automated-btn", "auto-icon-btn");
    buttonRowChildren.push(fullyAutoBtn);
  }
  buttonRowChildren.push(connectBtn, disconnectBtn);

  const specLabel = el(
    "label",
    { class: "auto-spec-label", for: "auto-spec-input" },
    "Specification:",
  );
  const input = document.createElement("input");
  input.type = "text";
  input.id = "auto-spec-input";
  input.placeholder = "Optional spec path (.pdf / .md / .txt)";
  input.value = ui.specPath;
  input.className = "auto-spec-input";
  input.addEventListener("input", () => {
    ui.specPath = input.value;
    // Persist on every keystroke. The host stores in workspaceState
    // keyed by projectDir; the next time the dashboard opens, the
    // persisted value seeds this input.
    send({ type: "set-spec-path", path: input.value });
  });

  const pickBtn = actionButton(
    "Browse...",
    "pick-spec",
    () => send({ type: "pick-spec-file" }),
    "secondary",
  );
  pickBtn.classList.add("auto-spec-pick-btn");

  const specHelp = el(
    "p",
    { class: "auto-flow-spec-help" },
    "User-provided specification that drives the flow toward a Foundation model. If left empty, the agent will prompt you for what to model when DM0 starts. After entering the path to your spec (or leaving it blank) click the Connect button to launch the session, then drive each step from the rail.",
  );

  // Stack the help line directly on top of the input within its own
  // column, so the help spans only the input's width -- not the
  // Spec: label or Browse... button to either side.
  const specInputCol = el("div", { class: "auto-spec-input-col" }, specHelp, input);

  // Single row: [Spec:] [help+input column] [Browse...] [▶?] [🔌] [⏻].
  // The user reads the inline description, enters a spec (or
  // browses to one), then clicks Connect; Disconnect sits at the
  // right edge of the same row so it's always reachable.
  return el(
    "div",
    { class: "auto-flow-row" },
    specLabel,
    specInputCol,
    pickBtn,
    ...buttonRowChildren,
  );
}

function stepBox(data: DashboardState, step: StepDef): HTMLElement {
  const gate = data.flow.gates[step.id];
  const passed = gate?.passed === true;
  const current = data.flow.current_step === step.id;
  const selected = ui.selectedStep === step.id;
  const selectable = isStepSelectableInRail(data, step.id);
  const ahead = isStepAheadOfCurrent(data, step.id);
  const disabledAhead = ahead && !selectable;
  const classes = ["step"];
  if (passed) {
    classes.push("passed");
  }
  if (current) {
    classes.push("current");
  }
  if (disabledAhead) {
    classes.push("disabled-ahead");
  }
  if (selected) {
    classes.push("selected");
  }
  const box = el(
    "div",
    {
      class: classes.join(" "),
      role: "button",
      tabindex: selectable ? "0" : "-1",
      "aria-disabled": selectable ? "false" : "true",
      title: selectable
        ? `Select ${step.id}`
        : "This step is ahead of the current step and has not been visited yet.",
    },
    el("span", { class: "step-id" }, step.id),
    el("span", { class: "step-label" }, step.label),
  );
  box.addEventListener("click", () => {
    if (!selectable) {
      return;
    }
    ui.selectedStep = step.id;
    ui.gateReport = null;
    send({ type: "select-step", step: step.id });
    render();
  });
  box.addEventListener("keydown", (event) => {
    if (!selectable || (event.key !== "Enter" && event.key !== " ")) {
      return;
    }
    event.preventDefault();
    box.click();
  });
  return box;
}

function isStepAheadOfCurrent(data: DashboardState, stepId: string): boolean {
  const order = data.flow.flow === "direct-modeling" ? DM_STEPS : DS_STEPS;
  const currentIndex = order.findIndex((step) => step.id === data.flow.current_step);
  const stepIndex = order.findIndex((step) => step.id === stepId);
  if (currentIndex === -1 || stepIndex === -1) {
    return false;
  }
  return stepIndex > currentIndex;
}

function gateDiamond(data: DashboardState, step: StepDef): HTMLElement {
  const gate = data.flow.gates[step.id];
  const passed = gate?.passed === true;
  const cls = passed ? "gate passed" : "gate";
  return el("div", { class: cls, title: `Gate after ${step.id}` }, el("span", {}, "G"));
}

function renderSelectedStepDetail(data: DashboardState): HTMLElement {
  const stepId = ui.selectedStep;
  if (!stepId) {
    return el("div", { class: "detail" }, el("p", { class: "empty" }, "Select a step above."));
  }
  const flowUnlocked = ui.autoRunning;
  const actions = deriveStepActionState({
    data,
    stepId,
    gateReport: ui.gateReport,
  });
  const runStepBtn = actionButton("Run Step", `run-step-${stepId}`, () =>
    send({ type: "run-step", step: stepId }),
  );
  applyButtonState(
    runStepBtn,
    flowUnlocked && actions.runStepEnabled,
    flowUnlocked ? actions.runStepReason : FLOW_LOCKED_REASON,
  );
  const runCritiqueBtn = actionButton("Run Critique", `run-critique-${stepId}`, () =>
    send({ type: "run-critique", step: stepId }),
  );
  applyButtonState(
    runCritiqueBtn,
    flowUnlocked && actions.runCritiqueEnabled,
    flowUnlocked ? actions.runCritiqueReason : FLOW_LOCKED_REASON,
  );
  const runGateBtn = actionButton(
    "Run Gate",
    `gate-${stepId}`,
    () => send({ type: "gate-step", step: stepId }),
    "secondary",
  );
  applyButtonState(
    runGateBtn,
    flowUnlocked && actions.runGateEnabled,
    flowUnlocked ? actions.runGateReason : FLOW_LOCKED_REASON,
  );
  const advanceBtn = actionButton("Advance", `advance-${stepId}`, () =>
    send({ type: "advance-step", step: stepId }),
  );
  applyButtonState(
    advanceBtn,
    flowUnlocked && actions.advanceEnabled,
    flowUnlocked ? actions.advanceReason : FLOW_LOCKED_REASON,
  );
  const resetBtn = actionButton(
    "Reset",
    `reset-${stepId}`,
    () => send({ type: "reset-step", step: stepId }),
    "secondary",
  );
  applyButtonState(
    resetBtn,
    flowUnlocked && actions.resetEnabled,
    flowUnlocked ? actions.resetReason : FLOW_LOCKED_REASON,
  );
  const generateVerilogBtn = actions.showGenerateVerilog
    ? buildGenerateVerilogButton(stepId, flowUnlocked)
    : null;
  const children: Node[] = [
    el("h3", {}, stepId),
    el(
      "div",
      { class: "actions" },
      runStepBtn,
      runCritiqueBtn,
      runGateBtn,
      advanceBtn,
      resetBtn,
      ...(generateVerilogBtn ? [generateVerilogBtn] : []),
    ),
  ];
  // Plan-execution progress (DM2d / DM3c / DM4b only). For other
  // steps `kind` is "none" and we render nothing -- the rest of the
  // step detail keeps its current look.
  if (stepId === data.flow.current_step && data.planProgress.kind !== "none") {
    children.push(renderPlanProgress(data.planProgress));
  }
  const critique = findCritique(data.critiques, stepId);
  if (critique) {
    children.push(renderCritiqueSummary(critique));
  } else {
    children.push(el("p", { class: "empty" }, "No critique file for this step yet."));
  }
  if (ui.gateReport && ui.gateReport.step === stepId) {
    children.push(renderGateReport(ui.gateReport));
  }
  return el("div", { class: "detail" }, ...children);
}

/**
 * Render the per-step plan-execution progress row.
 *
 * - One box per milestone (DM2d / DM4b) or test category (DM3c).
 * - Box color: gray (no resolved rows), light yellow (some
 *   resolved), light green (all rows resolved -- done OR deferred).
 * - Label: milestone id + percent done. Hover for full title.
 * - Click: opens the milestone / category file in a regular editor
 *   tab at the right line.
 *
 * Beneath the row, a one-line "Current task: ..." surface (best
 * guess based on the most recently modified milestone file). Click
 * to jump to the row.
 */
function renderPlanProgress(progress: import("./messages").PlanProgress): HTMLElement {
  const wrap = el("div", { class: "plan-progress" });
  const heading = (() => {
    switch (progress.kind) {
      case "impl":
        return "Implementation plan progress";
      case "test":
        return "Test plan progress";
      case "perf":
        return "Performance plan progress";
      default:
        return "Plan progress";
    }
  })();
  wrap.appendChild(el("h4", { class: "plan-progress-heading" }, heading));

  if (progress.milestones.length === 0) {
    wrap.appendChild(
      el(
        "p",
        { class: "empty" },
        "No plan files on disk yet -- once the plan-writing step runs, milestones will appear here.",
      ),
    );
    return wrap;
  }

  const row = el("div", { class: "milestone-row" });
  for (const m of progress.milestones) {
    const total = m.done + m.deferred + m.pending;
    const resolved = m.done + m.deferred;
    const pct = total === 0 ? 0 : Math.round((resolved / total) * 100);
    let status: "empty" | "in-progress" | "done";
    if (total === 0) {
      status = "empty";
    } else if (resolved === total) {
      status = "done";
    } else if (resolved === 0) {
      status = "empty";
    } else {
      status = "in-progress";
    }
    const tooltip = `${m.title}\n${m.done} done, ${m.deferred} deferred, ${m.pending} pending\nClick to open the milestone file.`;
    const box = el(
      "button",
      {
        class: `milestone-box milestone-${status}`,
        title: tooltip,
      },
      el("span", { class: "milestone-id" }, m.id),
      el("span", { class: "milestone-pct" }, total === 0 ? "—" : `${pct}%`),
    ) as HTMLButtonElement;
    box.addEventListener("click", () => {
      send({ type: "open-document", path: m.filePath });
    });
    row.appendChild(box);
  }
  wrap.appendChild(row);

  // Current task line. Best-effort: the heuristic in
  // planProgress.ts picks the most-recently-modified milestone's
  // first un-checked row. It can lag if the agent jumped without
  // editing the file yet, hence the "best guess" framing in the
  // tooltip.
  if (progress.currentTask) {
    const taskLine = el("p", { class: "plan-progress-current" });
    taskLine.appendChild(el("span", { class: "muted" }, "Current task (best guess): "));
    const taskBtn = el(
      "button",
      {
        class: "linkish",
        title: "Click to open the plan file at this row",
      },
      progress.currentTask,
    ) as HTMLButtonElement;
    taskBtn.addEventListener("click", () => {
      if (progress.currentTaskFilePath) {
        send({ type: "open-document", path: progress.currentTaskFilePath });
      }
    });
    taskLine.appendChild(taskBtn);
    wrap.appendChild(taskLine);
  } else {
    wrap.appendChild(
      el(
        "p",
        { class: "plan-progress-current muted" },
        "All milestones resolved (done or deferred).",
      ),
    );
  }
  return wrap;
}

function renderCritiqueSummary(critique: CritiqueFile): HTMLElement {
  const blocker = critique.findings.filter((f: Finding) => f.kind === "blocker");
  const unresolved = critique.findings.filter((f: Finding) => f.kind === "unresolved");
  const resolved = critique.findings.filter((f: Finding) => f.kind === "resolved");
  const list = el("ul", { class: "critique-list" });
  for (const f of [...blocker, ...unresolved, ...resolved]) {
    list.appendChild(
      el(
        "li",
        { class: `finding ${f.kind}` },
        `${f.kind.toUpperCase()}: ${f.text} (line ${f.line})`,
      ),
    );
  }
  return el(
    "div",
    {},
    el("strong", {}, critique.hasBlocking ? "Critique: blocking" : "Critique: clean"),
    list,
  );
}

function renderGateReport(report: GateResult): HTMLElement {
  if (report.clean) {
    return el("p", { class: "finding resolved" }, "Gate clean.");
  }
  const list = el("ul", {});
  for (const f of report.failures as GateFailure[]) {
    list.appendChild(el("li", { class: "finding blocker" }, `${f.description}: ${f.reason}`));
  }
  return el("div", {}, el("strong", {}, `Gate failed (${report.failures.length}):`), list);
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
// Documents tab
// --------------------------------------------------------------

function renderDocumentsTab(data: DashboardState): Node[] {
  const docs = data.documents ?? [];
  if (docs.length === 0) {
    return [
      el("h2", {}, "Documents"),
      el(
        "p",
        { class: "empty" },
        "No project documents yet. Run a step or ingest a spec to populate this list.",
      ),
    ];
  }
  // Group by category in display order: source-spec → work artifacts
  // (per step) → critiques.
  const categoryOrder: DocumentEntry["category"][] = [
    "source-spec",
    "work-artifact",
    "critique",
    "spec-page",
    "other",
  ];
  const categoryLabels: Record<DocumentEntry["category"], string> = {
    "source-spec": "Source spec",
    "work-artifact": "Work artifacts",
    critique: "Critiques",
    "spec-page": "Spec pages",
    other: "Other",
  };
  const groups = new Map<DocumentEntry["category"], DocumentEntry[]>();
  for (const d of docs) {
    const list = groups.get(d.category) ?? [];
    list.push(d);
    groups.set(d.category, list);
  }
  const out: Node[] = [el("h2", {}, "Documents")];
  for (const cat of categoryOrder) {
    const rows = groups.get(cat);
    if (!rows || rows.length === 0) {
      continue;
    }
    out.push(el("h3", { class: "doc-group" }, categoryLabels[cat]));
    out.push(renderDocsTable(rows));
  }
  return out;
}

function renderDocsTable(entries: DocumentEntry[]): HTMLElement {
  const table = el("table", { class: "documents-table" });
  table.appendChild(
    el(
      "thead",
      {},
      el(
        "tr",
        {},
        el("th", {}, "Path"),
        el("th", {}, "Step"),
        el("th", {}, "Size"),
        el("th", {}, ""),
      ),
    ),
  );
  const body = el("tbody", {});
  for (const entry of entries) {
    const sizeCell = entry.exists
      ? humanBytes(entry.bytes ?? 0)
      : el("span", { class: "muted" }, "—");
    const pathCell = el("code", {}, entry.relPath);
    if (!entry.exists) {
      pathCell.classList.add("muted");
    }
    const action = entry.exists
      ? actionButton(
          "Open",
          `open-doc-${entry.absPath}`,
          () => send({ type: "open-document", path: entry.absPath }),
          "secondary",
        )
      : el("span", { class: "muted" }, "not yet on disk");
    body.appendChild(
      el(
        "tr",
        { class: entry.exists ? "" : "doc-missing" },
        el("td", {}, pathCell),
        el("td", {}, entry.step ?? ""),
        el("td", {}, sizeCell),
        el("td", { class: "actions" }, action),
      ),
    );
  }
  table.appendChild(body);
  return table;
}

function humanBytes(n: number): string {
  if (n < 1024) {
    return `${n} B`;
  }
  if (n < 1024 * 1024) {
    return `${(n / 1024).toFixed(1)} KB`;
  }
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
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

interface StepDef {
  id: string;
  label: string;
}

const DM_STEPS: StepDef[] = [
  { id: "DM0", label: "Spec" },
  { id: "DM1", label: "Setup" },
  { id: "DM2a", label: "Decomp" },
  { id: "DM2b", label: "Pipeline" },
  { id: "DM2c", label: "ImplPlan" },
  { id: "DM2d", label: "Model" },
  { id: "DM3a", label: "TestPlan" },
  { id: "DM3b", label: "Bench" },
  { id: "DM3c", label: "Tests" },
  { id: "DM4a", label: "PerfPlan" },
  { id: "DM4b", label: "Perf" },
];

const DS_STEPS: StepDef[] = [
  { id: "DS0", label: "Spec" },
  { id: "DS1", label: "Setup" },
  { id: "DS2", label: "Decomp" },
  { id: "DS3", label: "Pipeline" },
  { id: "DS4", label: "Screen" },
  { id: "DS5a", label: "Proto" },
  { id: "DS5b", label: "Smoke" },
  { id: "DS6", label: "Compare" },
  { id: "DS7", label: "Deep" },
  { id: "DS8", label: "Decide" },
  { id: "DS9", label: "Formalize" },
];

function findCritique(critiques: CritiqueFile[], step: string): CritiqueFile | undefined {
  return critiques.find((c) => c.step === step);
}

function shortPath(full: string): string {
  const parts = full.split(/[/\\]/);
  if (parts.length <= 3) {
    return full;
  }
  return `.../${parts.slice(-2).join("/")}`;
}

function buildGenerateVerilogButton(stepId: string, enabled: boolean): HTMLButtonElement {
  const verilogBtn = actionButton(
    "Generate Verilog",
    `generate-verilog-${stepId}`,
    () => send({ type: "generate-verilog" }),
    "secondary",
  );
  verilogBtn.classList.add("generate-verilog-btn");
  applyButtonState(
    verilogBtn,
    enabled,
    enabled
      ? "Emit synthesizable SystemVerilog RTL + UVM testbench from the current Foundation model into the project's `generated/` directory."
      : FLOW_LOCKED_REASON,
  );
  return verilogBtn;
}

function applyButtonState(button: HTMLButtonElement, enabled: boolean, title: string): void {
  button.title = title;
  if (!enabled) {
    button.disabled = true;
  }
}

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
  variant?: "secondary",
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
  b.addEventListener("click", () => {
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
