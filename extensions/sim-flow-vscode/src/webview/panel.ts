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
  LLM_SERVER_DEFAULT_PORT,
  LLM_SOURCE_LABELS,
  type DocumentEntry,
  type LlmServerEntry,
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
  llmSource: string | null;
  /** Active `sim-flow.llm.model` setting; empty string means "any". */
  llmModel: string;
  /** Active explicit model-family override; empty string means infer from model id. */
  llmModelFamilyId: string;
  /** Active explicit runtime-profile override; empty string means use source default. */
  llmRuntimeProfileId: string;
  /** Active `sim-flow.llm.verbose` setting. When false the pump prepends a "be concise" system message. */
  llmVerbose: boolean;
  /** When true, show adaptation diagnostics around backend dispatches. */
  llmDebugAdaptation: boolean;
  /**
   * Cached list of models for `llmModelListSource`. We refetch on
   * source change and on a manual refresh click; otherwise we keep
   * showing the cached list.
   */
  llmModelList: string[];
  /** Source the cached model list was fetched for. */
  llmModelListSource: string | null;
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
  llmModelFamilyId: "",
  llmRuntimeProfileId: "",
  llmVerbose: true,
  llmDebugAdaptation: false,
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
  "Click Play / Resume first to unlock the step controls for this dashboard session.";

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
      const prevInSub = ui.data?.inSubSession ?? false;
      const prevSessionActive = ui.data?.sessionActive ?? false;
      const wasTracking = !ui.selectedStep || ui.selectedStep === prevCurrent;
      ui.data = msg.state;
      ui.lastError = null;
      // Reconcile the local Connect/Disconnect flag with the host's
      // truth. Two desync sources:
      //   (a) the orchestrator died on its own (transport-error,
      //       runaway-guard, child-exit) -- `autoRunning` would
      //       otherwise stay stuck `true` and Connect stay disabled
      //       until the user reloaded the window.
      //   (b) the webview reloaded while a pump is alive --
      //       `autoRunning` resets to `false` and the per-step
      //       buttons render `flow-locked` despite a live session.
      ui.autoRunning = msg.state.sessionActive;
      // Discriminating pendingAction clears. Previously we cleared
      // every entry on every `state-update`, which clobbered the
      // optimistic disable from a freshly-clicked button whenever an
      // unrelated trigger (file-watcher tick, viewState refresh,
      // llm-config arrival, ...) caused a state-update to land in
      // the gap between click and the orchestrator's acknowledgement.
      // The 5s `actionButton` failsafe still ensures nothing stays
      // stuck forever; here we just clear when we have positive
      // evidence the action resolved.
      if (prevInSub && !msg.state.inSubSession) {
        // A sub-session just released -- run-step / run-critique
        // for any step have settled.
        for (const id of Array.from(ui.pendingActions)) {
          if (
            id.startsWith("run-step-") ||
            id.startsWith("run-critique-") ||
            id.startsWith("reset-")
          ) {
            ui.pendingActions.delete(id);
          }
        }
      }
      if (prevCurrent !== null && prevCurrent !== msg.state.flow.current_step) {
        // /advance bumped the current step -- the previous step's
        // advance pending entry settled.
        ui.pendingActions.delete(`advance-${prevCurrent}`);
      }
      if (prevSessionActive !== msg.state.sessionActive) {
        // Connect / Disconnect resolved on the host side.
        ui.pendingActions.delete("run-auto");
        ui.pendingActions.delete("stop-auto");
      }
      // The selected step changed: drop any stale gate report from
      // the previous step regardless of whether the user was
      // tracking. A user inspecting an old step shouldn't see a
      // gate report cached from before /advance bumped the flow.
      if (prevCurrent !== null && prevCurrent !== msg.state.flow.current_step) {
        ui.gateReport = null;
      }
      if (wasTracking) {
        ui.selectedStep = msg.state.flow.current_step;
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
      ui.llmModelFamilyId = msg.modelFamilyId ?? "";
      ui.llmRuntimeProfileId = msg.runtimeProfileId ?? "";
      ui.llmVerbose = msg.verbose;
      ui.llmDebugAdaptation = msg.debugAdaptation;
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
      // Errors no longer clear EVERY pending entry -- doing so wipes
      // unrelated in-flight actions (e.g. a Run Critique still
      // running while a `regenerate-block-diagram` fails). The 5s
      // failsafe in `actionButton` still releases anything that
      // never gets a discrete settled signal, so users aren't left
      // with permanently-disabled buttons.
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
      el(
        "div",
        { class: "settings-row" },
        renderLlmModelFamilyPicker(),
        renderLlmRuntimeProfilePicker(),
      ),
    ),
    el(
      "div",
      { class: "settings-section" },
      el("h3", {}, "Custom LLM servers"),
      el(
        "p",
        { class: "muted" },
        "Add OpenAI-compat servers (vLLM / Ollama / LM Studio / generic) by hostname + port. Saved entries appear in the Source dropdown above; pick one and the dashboard dispatches against it. Empty list = use built-in defaults (localhost + each kind's conventional port).",
      ),
      renderLlmServersTable(),
    ),
    el(
      "div",
      { class: "settings-section" },
      el("h3", {}, "Output style"),
      el("div", { class: "settings-row" }, renderVerboseToggle(), renderDebugAdaptationToggle()),
    ),
    el(
      "div",
      { class: "settings-section" },
      el("h3", {}, "Dashboard"),
      el(
        "p",
        { class: "muted" },
        "Show the red end-to-end automated-flow button next to Play / Stop on the Flow tab. Hidden by default because the automated flow walks every step without stopping for review and can burn meaningful LLM credits.",
      ),
      el("div", { class: "settings-row" }, renderFullyAutomatedToggle()),
    ),
    el(
      "div",
      { class: "settings-section" },
      el("h3", {}, "Coverage acceptance"),
      el(
        "p",
        { class: "muted" },
        "DM3c gates a flow on `cargo llvm-cov`. Threshold is the minimum required line-coverage percentage. Level controls whether every reported module must hit the bar (`module`) or only the project-wide total (`total`). Stored in `.sim-flow/config.toml::coverage` and round-trippable from the CLI via `sim-flow coverage show / set`.",
      ),
      el("div", { class: "settings-row" }, renderCoverageThreshold(), renderCoverageLevel()),
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

/**
 * Render the user-defined LLM servers table. Each row shows the
 * entry's name / kind / host / port / model and exposes a Remove
 * button. A trailing "Add server" button appends a new row with
 * the kind's conventional default port. Edits flow through
 * `set-llm-servers` which writes the whole array back to the
 * `sim-flow.llm.servers` workspace setting.
 */
function renderLlmServersTable(): HTMLElement {
  const wrap = el("div", { class: "llm-servers" });
  const servers: LlmServerEntry[] = (ui.data?.llmServers ?? []).slice();

  const commit = (next: LlmServerEntry[]): void => {
    // Optimistic update: write the new array straight into the
    // local snapshot before the round-trip lands. This stops the
    // table from snapping back when the host's
    // `onDidChangeConfiguration` -> `postLlmConfig` -> webview
    // `render()` chain re-reads `ui.data.llmServers` before the
    // post-write `state-update` arrives. Once the host's
    // `set-llm-servers` handler fires `refresh()` and the fresh
    // state arrives, this same array gets written into `ui.data`
    // again -- a no-op visually.
    if (ui.data) {
      ui.data.llmServers = next;
    }
    send({ type: "set-llm-servers", servers: next });
  };

  if (servers.length === 0) {
    wrap.appendChild(
      el(
        "p",
        { class: "muted" },
        "No custom servers configured. Click Add server to point sim-flow at a remote vLLM / Ollama / LM Studio host or a non-default port.",
      ),
    );
  } else {
    const table = el("table", { class: "llm-servers-table" });
    table.appendChild(
      el(
        "thead",
        {},
        el(
          "tr",
          {},
          el("th", {}, "Name"),
          el("th", {}, "Kind"),
          el("th", {}, "Host"),
          el("th", {}, "Port"),
          el("th", {}, "Model (optional)"),
          el("th", {}, "Family (optional)"),
          el("th", {}, "Runtime (optional)"),
          el("th", {}, ""),
        ),
      ),
    );
    const body = el("tbody", {});
    for (let i = 0; i < servers.length; i++) {
      const entry = servers[i];
      const updateAt = (patch: Partial<LlmServerEntry>): void => {
        const next = servers.slice();
        next[i] = { ...next[i], ...patch };
        commit(next);
      };
      const removeAt = (): void => {
        commit(servers.filter((_, idx) => idx !== i));
      };
      body.appendChild(
        el(
          "tr",
          {},
          el(
            "td",
            {},
            llmServerTextInput(entry.name, "name (e.g. vllm-bigbox)", (v) => updateAt({ name: v })),
          ),
          el(
            "td",
            {},
            llmServerKindSelect(entry.kind, (kind) => {
              const patch: Partial<LlmServerEntry> = { kind };
              if (entry.port === LLM_SERVER_DEFAULT_PORT[entry.kind]) {
                patch.port = LLM_SERVER_DEFAULT_PORT[kind];
              }
              updateAt(patch);
            }),
          ),
          el(
            "td",
            {},
            llmServerTextInput(entry.host, "host or IP", (v) =>
              updateAt({ host: v.length === 0 ? "localhost" : v }),
            ),
          ),
          el(
            "td",
            {},
            llmServerNumberInput(entry.port, (n) => updateAt({ port: n })),
          ),
          el(
            "td",
            {},
            llmServerTextInput(entry.model ?? "", "default", (v) =>
              updateAt({ model: v.length === 0 ? undefined : v }),
            ),
          ),
          el(
            "td",
            {},
            llmServerTextInput(entry.modelFamilyId ?? "", "infer", (v) =>
              updateAt({ modelFamilyId: v.length === 0 ? undefined : v }),
            ),
          ),
          el(
            "td",
            {},
            llmServerTextInput(entry.runtimeProfileId ?? "", "source default", (v) =>
              updateAt({ runtimeProfileId: v.length === 0 ? undefined : v }),
            ),
          ),
          el(
            "td",
            { class: "actions" },
            actionButton("Remove", `llm-server-remove-${i}`, removeAt, "secondary"),
          ),
        ),
      );
    }
    table.appendChild(body);
    wrap.appendChild(table);
  }

  const addBtn = actionButton(
    "Add server",
    "llm-server-add",
    () => {
      const next: LlmServerEntry = {
        name: `server-${servers.length + 1}`,
        kind: "vllm",
        host: "localhost",
        port: LLM_SERVER_DEFAULT_PORT.vllm,
      };
      commit(servers.concat(next));
    },
    "secondary",
  );
  addBtn.title =
    "Append a new row. Defaults to vLLM on localhost:8000; edit in place to point elsewhere.";
  wrap.appendChild(el("div", { class: "llm-servers-actions" }, addBtn));
  return wrap;
}

function llmServerTextInput(
  value: string,
  placeholder: string,
  onCommit: (value: string) => void,
): HTMLInputElement {
  const input = document.createElement("input");
  input.type = "text";
  input.className = "llm-server-input";
  input.value = value;
  input.placeholder = placeholder;
  const commit = (): void => {
    if (input.value.trim() !== value.trim()) {
      onCommit(input.value.trim());
    }
  };
  input.addEventListener("blur", commit);
  input.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      commit();
    }
  });
  return input;
}

function llmServerNumberInput(value: number, onCommit: (value: number) => void): HTMLInputElement {
  const input = document.createElement("input");
  input.type = "number";
  input.className = "llm-server-input llm-server-port";
  input.min = "1";
  input.max = "65535";
  input.value = String(value);
  const commit = (): void => {
    const parsed = parseInt(input.value, 10);
    if (Number.isFinite(parsed) && parsed > 0 && parsed <= 65535 && parsed !== value) {
      onCommit(parsed);
    } else if (!Number.isFinite(parsed) || parsed <= 0 || parsed > 65535) {
      // Reset on invalid input so the user sees the rejection.
      input.value = String(value);
    }
  };
  input.addEventListener("blur", commit);
  input.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      commit();
    }
  });
  return input;
}

function llmServerKindSelect(
  current: LlmServerEntry["kind"],
  onChange: (value: LlmServerEntry["kind"]) => void,
): HTMLSelectElement {
  const select = document.createElement("select");
  select.className = "llm-server-input";
  const kinds: ReadonlyArray<{ id: LlmServerEntry["kind"]; label: string }> = [
    { id: "vllm", label: "vLLM" },
    { id: "ollama", label: "Ollama" },
    { id: "lmstudio", label: "LM Studio" },
    { id: "openai-compat", label: "OpenAI-compat" },
  ];
  for (const kind of kinds) {
    const opt = document.createElement("option");
    opt.value = kind.id;
    opt.textContent = kind.label;
    if (kind.id === current) {
      opt.selected = true;
    }
    select.appendChild(opt);
  }
  select.addEventListener("change", () => {
    onChange(select.value as LlmServerEntry["kind"]);
  });
  return select;
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

function renderCoverageThreshold(): HTMLElement {
  const wrap = el("label", { class: "llm-source" }, "Threshold (%): ");
  const input = document.createElement("input");
  input.type = "number";
  input.min = "0";
  input.max = "100";
  input.step = "0.1";
  input.className = "coverage-threshold";
  input.value = (ui.data?.coverage?.thresholdPct ?? 90).toString();
  input.title =
    "Minimum required line-coverage percentage. The DM3c critique fails when measured coverage is below this value. Persist on blur / Enter to avoid a round-trip per keystroke.";
  const commit = (): void => {
    const parsed = Number.parseFloat(input.value);
    if (!Number.isFinite(parsed)) {
      // Reset to the last-known value when the user types
      // something unparseable rather than silently writing NaN.
      input.value = (ui.data?.coverage?.thresholdPct ?? 90).toString();
      return;
    }
    send({
      type: "set-coverage",
      coverage: {
        thresholdPct: parsed,
        level: ui.data?.coverage?.level ?? "total",
      },
    });
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

function renderCoverageLevel(): HTMLElement {
  const wrap = el("label", { class: "llm-source" }, "Level: ");
  const select = document.createElement("select");
  select.className = "coverage-level-select";
  const current = ui.data?.coverage?.level ?? "total";
  for (const value of ["total", "module"] as const) {
    const opt = document.createElement("option");
    opt.value = value;
    opt.textContent =
      value === "total"
        ? "total -- only the project total must meet the threshold"
        : "module -- every module must meet the threshold";
    if (value === current) {
      opt.selected = true;
    }
    select.appendChild(opt);
  }
  select.title =
    "`total` lets heavily-tested modules offset thinly-tested ones; `module` is strict and surfaces gaps in any one file.";
  select.addEventListener("change", () => {
    const level = select.value === "module" ? "module" : "total";
    send({
      type: "set-coverage",
      coverage: {
        thresholdPct: ui.data?.coverage?.thresholdPct ?? 90,
        level,
      },
    });
  });
  wrap.appendChild(select);
  return wrap;
}

function renderDebugAdaptationToggle(): HTMLElement {
  const wrap = el("label", { class: "llm-verbose" });
  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = ui.llmDebugAdaptation;
  input.title =
    "When ON, sim-flow prints the active backend, runtime profile, model-family profile, and key adaptation capabilities around LLM dispatches and failures.";
  input.addEventListener("change", () => {
    ui.llmDebugAdaptation = input.checked;
    send({ type: "set-llm-debug-adaptation", debugAdaptation: input.checked });
  });
  wrap.appendChild(input);
  wrap.appendChild(document.createTextNode(" Adaptation diagnostics"));
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

function renderLlmModelFamilyPicker(): HTMLElement {
  const wrap = el("label", { class: "llm-source" }, "Model family: ");
  const select = document.createElement("select");
  select.className = "llm-source-select";
  const options = [
    { id: "", label: "(infer from model)" },
    { id: "generic_chat", label: "generic_chat" },
    { id: "gemma4", label: "gemma4" },
    { id: "qwen3_6", label: "qwen3_6" },
    { id: "kimi_vl_thinking", label: "kimi_vl_thinking" },
    { id: "claude_messages", label: "claude_messages" },
  ];
  for (const option of options) {
    const opt = document.createElement("option");
    opt.value = option.id;
    opt.textContent = option.label;
    if (ui.llmModelFamilyId === option.id) {
      opt.selected = true;
    }
    select.appendChild(opt);
  }
  if (
    ui.llmModelFamilyId.length > 0 &&
    !options.some((option) => option.id === ui.llmModelFamilyId)
  ) {
    const custom = document.createElement("option");
    custom.value = ui.llmModelFamilyId;
    custom.textContent = `${ui.llmModelFamilyId} (custom)`;
    custom.selected = true;
    select.appendChild(custom);
  }
  select.title =
    "Explicit model-family override. Leave on `(infer from model)` for normal use; pin a family here when a runtime serves an ambiguous model id or when you want deterministic adaptation during debugging.";
  select.addEventListener("change", () => {
    ui.llmModelFamilyId = select.value;
    send({ type: "set-llm-model-family", modelFamilyId: select.value });
  });
  wrap.appendChild(select);
  return wrap;
}

function renderLlmRuntimeProfilePicker(): HTMLElement {
  const wrap = el("label", { class: "llm-source" }, "Runtime profile: ");
  const select = document.createElement("select");
  select.className = "llm-source-select";
  const options = [
    { id: "", label: "(source default)" },
    { id: "openai_compat_generic", label: "openai_compat_generic" },
    { id: "anthropic_messages", label: "anthropic_messages" },
    { id: "processor_local", label: "processor_local" },
    { id: "vscode_language_model", label: "vscode_language_model" },
  ];
  for (const option of options) {
    const opt = document.createElement("option");
    opt.value = option.id;
    opt.textContent = option.label;
    if (ui.llmRuntimeProfileId === option.id) {
      opt.selected = true;
    }
    select.appendChild(opt);
  }
  if (
    ui.llmRuntimeProfileId.length > 0 &&
    !options.some((option) => option.id === ui.llmRuntimeProfileId)
  ) {
    const custom = document.createElement("option");
    custom.value = ui.llmRuntimeProfileId;
    custom.textContent = `${ui.llmRuntimeProfileId} (custom)`;
    custom.selected = true;
    select.appendChild(custom);
  }
  select.title =
    "Explicit runtime-profile override. Leave on `(source default)` unless you need to pin a serving/runtime contract for debugging or compatibility triage.";
  select.addEventListener("change", () => {
    ui.llmRuntimeProfileId = select.value;
    send({ type: "set-llm-runtime-profile", runtimeProfileId: select.value });
  });
  wrap.appendChild(select);
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
  // Built-in tags first, then a divider + user-defined servers
  // from `sim-flow.llm.servers`. Matching by `name` instead of
  // tag means the dashboard remembers the chosen server across
  // reloads (the persisted value is the entry name).
  for (const id of Object.keys(LLM_SOURCE_LABELS) as LlmSourceTag[]) {
    const opt = document.createElement("option");
    opt.value = id;
    opt.textContent = LLM_SOURCE_LABELS[id];
    if (ui.llmSource === id) {
      opt.selected = true;
    }
    select.appendChild(opt);
  }
  const userServers = ui.data?.llmServers ?? [];
  if (userServers.length > 0) {
    const sep = document.createElement("option");
    sep.disabled = true;
    sep.textContent = "─── custom servers ───";
    select.appendChild(sep);
    for (const entry of userServers) {
      const opt = document.createElement("option");
      // Prefix the value so the host can route by name rather
      // than colliding with a built-in tag of the same string.
      opt.value = `server:${entry.name}`;
      opt.textContent = `${entry.name} (${entry.kind} @ ${entry.host}:${entry.port})`;
      if (ui.llmSource === `server:${entry.name}`) {
        opt.selected = true;
      }
      select.appendChild(opt);
    }
  }
  if (ui.llmSource === null) {
    // Loading state -- disable until the host posts the live value.
    select.disabled = true;
  }
  select.title =
    "Active LLM backend. Changing this here writes through to `sim-flow.llm.source` (workspace scope) and takes effect on the next LLM call -- you can switch mid-run if e.g. tokens are exhausted.";
  select.addEventListener("change", () => {
    const value = select.value;
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
  for (const step of flowSteps) {
    rail.appendChild(stepBox(data, step));
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
  // One-row control panel:
  //   [Spec:] [help+input col] [Browse...] [🔌 Connect] [Manual ⇄ Auto] [⏻ Disconnect]
  // Connect launches the orchestrator (or no-ops when one is already
  // attached); the step-mode toggle picks the initial mode and can
  // also flip live mid-session via SetStepMode; Disconnect tells the
  // orchestrator to shut down cleanly. In viewer mode (read-only
  // attach to a `--watch-socket` tap) Connect is replaced by a
  // "VIEWING" badge and Disconnect detaches the observer without
  // sending Shutdown to the orchestrator.
  const isViewer = ui.data?.isViewer ?? false;
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
    "Connect to a sim-flow session. Launches the orchestrator with the current step-mode setting; " +
    "the step rail and per-step buttons become active afterward.";
  connectBtn.classList.add("auto-run-btn", "auto-icon-btn");
  applyButtonState(
    connectBtn,
    !ui.autoRunning && !isViewer,
    isViewer
      ? "Viewing a run driven by another host -- Connect is disabled. Detach first."
      : ui.autoRunning
        ? "Connect is disabled while a session is already attached."
        : connectBtn.title,
  );

  const disconnectLabel = isViewer ? "Detach" : "\u{23FB}";
  const disconnectBtn = actionButton(
    disconnectLabel,
    "stop-auto",
    () => {
      ui.autoRunning = false;
      send({ type: "stop-auto" });
      render();
    },
    "secondary",
  );
  disconnectBtn.title = isViewer
    ? "Detach from the run you're observing. The orchestrator keeps running; only this dashboard's connection closes."
    : "Disconnect from the sim-flow session. The orchestrator shuts down cleanly; " +
      "after Disconnect the step rail re-locks until you Connect again.";
  disconnectBtn.classList.add("auto-stop-btn", "auto-icon-btn");
  applyButtonState(
    disconnectBtn,
    ui.autoRunning,
    ui.autoRunning
      ? disconnectBtn.title
      : "Disconnect is disabled because there is no active session.",
  );

  const buttonRowChildren: HTMLElement[] = [];
  if (isViewer) {
    const badge = el("span", { class: "viewer-badge" }, "VIEWING");
    badge.setAttribute(
      "title",
      "Read-only viewer attached to another host's --watch-socket. The composer / per-step buttons are disabled while the other host drives.",
    );
    buttonRowChildren.push(badge);
  } else {
    buttonRowChildren.push(connectBtn);
  }
  buttonRowChildren.push(renderStepModeToggle());
  buttonRowChildren.push(disconnectBtn);

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
    "User-provided specification that drives the flow toward a Foundation model. If left empty, the agent will prompt you for what to model when DM0 starts. After entering the path to your spec (or leaving it blank) click Connect to launch the session.",
  );

  // Stack the help line directly on top of the input within its own
  // column, so the help spans only the input's width -- not the
  // Spec: label or Browse... button to either side.
  const specInputCol = el("div", { class: "auto-spec-input-col" }, specHelp, input);

  // Single row: [Spec:] [help+input column] [Browse...] [🔌] [Manual⇄Auto] [⏻].
  return el(
    "div",
    { class: "auto-flow-row" },
    specLabel,
    specInputCol,
    pickBtn,
    ...buttonRowChildren,
  );
}

/**
 * Step-axis mode toggle (Manual ⇄ Auto). Two segmented buttons; the
 * active one is highlighted and disabled, the inactive one posts a
 * `set-step-mode` message on click. Tooltips on each option carry
 * the explanation; we don't render a separate label or live/setting
 * indicator since the visual state of the buttons already conveys
 * which mode is active.
 */
function renderStepModeToggle(): HTMLElement {
  const current = ui.data?.stepMode ?? "manual";
  return el(
    "div",
    { class: "step-mode-toggle", role: "group", "aria-label": "Step mode" },
    renderStepModeOption("manual", current),
    renderStepModeOption("auto", current),
  );
}

function renderStepModeOption(mode: "manual" | "auto", current: string): HTMLButtonElement {
  const isActive = mode === current;
  const btn = document.createElement("button");
  btn.type = "button";
  // Mode class lets CSS paint Auto-active in the warning palette
  // (matches Reset's destructive cue) while Manual-active stays in
  // the primary palette.
  btn.className =
    `step-mode-option step-mode-option-${mode}` + (isActive ? " step-mode-option-active" : "");
  btn.textContent = mode === "manual" ? "Manual" : "Auto";
  btn.setAttribute("aria-pressed", isActive ? "true" : "false");
  btn.title =
    mode === "manual"
      ? "Manual: orchestrator parks between sub-sessions; per-step buttons drive the flow."
      : "Auto: orchestrator walks current_step through end of flow without user input.";
  if (isActive) {
    btn.disabled = true;
  } else {
    btn.addEventListener("click", () => {
      send({ type: "set-step-mode", mode });
    });
  }
  return btn;
}

function stepBox(data: DashboardState, step: StepDef): HTMLElement {
  // Three visual states only:
  //   - `current`  → primary (the orchestrator's current_step)
  //   - `passed`   → green (gate flag is set)
  //   - default    → primary-disabled (everything else, including
  //                  steps ahead of current and steps before current
  //                  whose gate flag was cleared by Reset)
  // Selection adds an accent ring on top of any of the three.
  const gate = data.flow.gates[step.id];
  const passed = gate?.passed === true;
  const current = data.flow.current_step === step.id;
  const selected = ui.selectedStep === step.id;
  const selectable = isStepSelectableInRail(data, step.id);
  const classes = ["step"];
  if (passed) {
    classes.push("passed");
  }
  if (current) {
    classes.push("current");
  }
  if (!selectable) {
    classes.push("step-locked");
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
  // Per-step buttons (Run Step / Run Critique / Run Gate / Advance /
  // Reset) only fire in manual step mode. In auto mode the
  // orchestrator owns step execution and rejects these commands with
  // a Diagnostic; disabling here surfaces that ownership clearly so
  // the user toggles to manual first.
  const manualMode = (data.stepMode ?? "manual") === "manual";
  const autoModeReason =
    "Per-step controls are disabled while step mode is `auto`. " +
    "Toggle Step mode to `Manual` (between Play and Stop) to drive each step explicitly.";
  // While the orchestrator is inside a sub-session (Work / Critique
  // streaming, tool calls, gate evaluation, advance) the per-step
  // buttons must be disabled regardless of which step the user has
  // selected in the rail. Two reasons:
  //   1. Within-step ordering: if Run Step is mid-flight, Run
  //      Critique / Run Gate / Advance can't be sensibly clicked
  //      yet — the artifacts they read are still being produced.
  //   2. Cross-step lockout: clicking Run Gate on step Y while
  //      step X is running is a user mistake; the orchestrator
  //      would reject it but the post-click Diagnostic is jarring.
  // Reset is the recovery action — it stays enabled so the user
  // can recover from a stuck or mis-queued sub-session.
  const inSubSession = data.inSubSession ?? false;
  const isViewer = data.isViewer ?? false;
  const subSessionReason =
    "A sub-session is in flight; wait for it to complete before issuing the next command.";
  const viewerReason =
    "This dashboard is viewing a run driven by another host. Detach to drive your own session.";
  const stepGate = (enabled: boolean, reason: string): { enabled: boolean; reason: string } =>
    isViewer
      ? { enabled: false, reason: viewerReason }
      : !flowUnlocked
        ? { enabled: false, reason: FLOW_LOCKED_REASON }
        : !manualMode
          ? { enabled: false, reason: autoModeReason }
          : inSubSession
            ? { enabled: false, reason: subSessionReason }
            : { enabled, reason };
  const runStepBtn = actionButton("Run Step", `run-step-${stepId}`, () =>
    send({ type: "run-step", step: stepId }),
  );
  {
    const g = stepGate(actions.runStepEnabled, actions.runStepReason);
    applyButtonState(runStepBtn, g.enabled, g.reason);
  }
  const runCritiqueBtn = actionButton("Run Critique", `run-critique-${stepId}`, () =>
    send({ type: "run-critique", step: stepId }),
  );
  {
    const g = stepGate(actions.runCritiqueEnabled, actions.runCritiqueReason);
    applyButtonState(runCritiqueBtn, g.enabled, g.reason);
  }
  const runGateBtn = actionButton(
    "Run Gate",
    `gate-${stepId}`,
    () => send({ type: "gate-step", step: stepId }),
    "secondary",
  );
  {
    const g = stepGate(actions.runGateEnabled, actions.runGateReason);
    applyButtonState(runGateBtn, g.enabled, g.reason);
  }
  const advanceBtn = actionButton("Advance", `advance-${stepId}`, () =>
    send({ type: "advance-step", step: stepId }),
  );
  {
    const g = stepGate(actions.advanceEnabled, actions.advanceReason);
    applyButtonState(advanceBtn, g.enabled, g.reason);
  }
  // Reset is recovery, not a step action — it isn't owned by the
  // orchestrator's auto loop. It only makes sense in manual mode and
  // is hidden entirely in auto. In manual mode it's always enabled
  // (no flowUnlocked / `actions.resetEnabled` gating); the user may
  // need to recover before they connect, after a crash, or while a
  // sub-session is in flight.
  const resetBtn: HTMLButtonElement | null = manualMode
    ? actionButton(
        "Reset",
        `reset-${stepId}`,
        () => send({ type: "reset-step", step: stepId }),
        "warning",
      )
    : null;
  if (resetBtn) {
    resetBtn.classList.add("step-action-reset");
    resetBtn.title =
      "Reset this step and every downstream step. " +
      "Deletes generated work artifacts and critique files (when an orchestrator is attached) " +
      "and clears the matching gate flags. Confirmation is required.";
  }
  const generateVerilogBtn = actions.showGenerateVerilog
    ? buildGenerateVerilogButton(stepId, flowUnlocked)
    : null;
  // Layout: per-step actions left-to-right, optional Generate Verilog
  // immediately after Advance, then Reset (manual mode only) pinned
  // to the far right via CSS `margin-left: auto`
  // (`.step-action-reset`). Reset is the destructive action; the
  // visual gap reinforces that.
  const children: Node[] = [
    el("h3", {}, stepId),
    el(
      "div",
      { class: "actions" },
      runStepBtn,
      runCritiqueBtn,
      runGateBtn,
      advanceBtn,
      ...(generateVerilogBtn ? [generateVerilogBtn] : []),
      ...(resetBtn ? [resetBtn] : []),
    ),
  ];
  // Critique sits FIRST (above plan progress + artifacts) so the
  // user sees the gate-relevant outcome at a glance when clicking
  // back into a completed step. Plan progress + artifact list are
  // supporting context below.
  const critique = findCritique(data.critiques, stepId);
  if (critique) {
    children.push(renderCritiqueSummary(critique));
  } else {
    children.push(el("p", { class: "empty" }, "No critique file for this step yet."));
  }
  if (ui.gateReport && ui.gateReport.step === stepId) {
    children.push(renderGateReport(ui.gateReport));
  }
  // Plan-execution progress for any step whose flow phase has a
  // plan: DM2c / DM2d (impl), DM3a / DM3b / DM3c (test),
  // DM4a / DM4b (perf). The host ships a per-kind plan progress
  // record so the user gets the milestone pipeline view even
  // after the step has advanced past `current_step`.
  const planForStep = planProgressForStep(data, stepId);
  if (planForStep) {
    children.push(renderPlanProgress(planForStep));
  }
  // Per-step artifact list with sizes + estimated tokens + click-to-
  // open. For DM2d / DM3b / DM3c (code-touching steps) also surfaces
  // a one-line code summary (file count + total lines).
  children.push(renderStepArtifacts(data, stepId));
  return el("div", { class: "detail" }, ...children);
}

/**
 * Pick which plan progress to render under a given step. Outline +
 * detail steps share the same plan and milestone files, so they
 * map to the same plan-progress kind. Steps with no plan return
 * `null`.
 */
function planProgressForStep(
  data: DashboardState,
  stepId: string,
): import("./messages").PlanProgress | null {
  const planKindForStep: Record<string, "impl" | "test" | "perf"> = {
    DM2c: "impl",
    DM2cd: "impl",
    DM2d: "impl",
    DM3a: "test",
    DM3ad: "test",
    DM3b: "test",
    DM3c: "test",
    DM4a: "perf",
    DM4ad: "perf",
    DM4b: "perf",
  };
  const kind = planKindForStep[stepId];
  if (!kind) {
    return null;
  }
  // Backwards-compat: when the host hasn't shipped the per-kind
  // map yet, fall back to the single planProgress field that's
  // tied to current_step. After the host upgrades, every plan
  // step gets its own progress regardless of which step is current.
  const map = data.planProgressByKind;
  if (map && map[kind] && map[kind].kind !== "none") {
    return map[kind];
  }
  if (data.planProgress.kind === kind) {
    return data.planProgress;
  }
  return null;
}

/**
 * Per-step artifacts overview. Lists every DocumentEntry whose
 * `step === stepId` (plus the source-spec rows under DM0) with
 * size + estimated tokens + Open button. Files marked with
 * `previews` (decomposition.md / pipeline-mapping.md / etc.)
 * inline a rendered table or markdown body under the row so the
 * user gets the summary without an Open round-trip. For
 * code-touching steps, surfaces a one-line "N files / M lines"
 * code summary above the table.
 *
 * Hierarchical-planning detail steps (DM2cd / DM3ad / DM4ad) are
 * NOT surfaced as separate rail buttons -- they share artifact
 * directories with their outline parent (DM2c / DM3a / DM4a) so
 * we fold their step-tagged documents into the parent's view.
 * Same for DM3a / DM3b / DM3c which all live under tests/ +
 * docs/test-plan/; selecting any of them shows the union.
 */
function renderStepArtifacts(data: DashboardState, stepId: string): HTMLElement {
  const wrap = el("div", { class: "step-artifacts" });
  wrap.appendChild(el("h4", { class: "step-artifacts-heading" }, "Artifacts"));

  // Step-id grouping: an outline step claims its detail step's
  // documents so the user sees the full picture under one rail
  // entry. The detail steps don't appear in DM_STEPS, but the
  // host's enumeration tags documents with the detail step's id;
  // pull both buckets here.
  const outlineToDetail: Record<string, string[]> = {
    DM2c: ["DM2cd"],
    DM3a: ["DM3ad"],
    DM4a: ["DM4ad"],
  };
  const acceptedSteps = new Set<string>([stepId, ...(outlineToDetail[stepId] ?? [])]);
  // Filter: step-tagged rows + source-spec rows (which are
  // step-less but conceptually belong to DM0's input surface).
  const entries = data.documents.filter((d) => {
    if (d.step !== undefined && acceptedSteps.has(d.step)) {
      return true;
    }
    if (stepId === "DM0" && d.category === "source-spec") {
      return true;
    }
    return false;
  });

  if (entries.length === 0) {
    wrap.appendChild(el("p", { class: "empty" }, "No artifacts on disk yet for this step."));
    return wrap;
  }

  // Code summary for code-touching steps: count rows whose
  // lineCount is set (i.e., Rust source / tests). Bytes-only
  // artifacts (markdown plans) don't contribute -- they're docs
  // not code.
  const codeRows = entries.filter((d) => d.exists && d.lineCount !== undefined);
  if (codeRows.length > 0) {
    const totalLines = codeRows.reduce((acc, d) => acc + (d.lineCount ?? 0), 0);
    const totalBytes = codeRows.reduce((acc, d) => acc + (d.bytes ?? 0), 0);
    wrap.appendChild(
      el(
        "p",
        { class: "step-artifacts-summary muted" },
        `Code summary: ${codeRows.length} file${codeRows.length === 1 ? "" : "s"}, ` +
          `${totalLines.toLocaleString()} lines, ${humanBytes(totalBytes)}.`,
      ),
    );
  }

  const table = el("table", { class: "documents-table step-artifacts-table" });
  table.appendChild(
    el(
      "thead",
      {},
      el(
        "tr",
        {},
        el("th", {}, "Path"),
        el("th", {}, "Size"),
        el("th", {}, "~Tokens"),
        el("th", {}, ""),
      ),
    ),
  );
  const body = el("tbody", {});
  for (const entry of entries) {
    const sizeCell = entry.exists
      ? humanBytes(entry.bytes ?? 0)
      : el("span", { class: "muted" }, "—");
    const tokenCell = entry.exists
      ? approxTokens(entry.bytes ?? 0)
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
        el("td", {}, sizeCell),
        el("td", {}, tokenCell),
        el("td", { class: "actions" }, action),
      ),
    );
    if (entry.previews !== undefined && entry.exists) {
      for (const preview of entry.previews) {
        body.appendChild(
          el(
            "tr",
            { class: "step-artifacts-preview-row" },
            el(
              "td",
              { colspan: "4" },
              el("div", { class: "step-artifacts-rendered" }, ...renderPreview(preview)),
            ),
          ),
        );
      }
    }
  }
  table.appendChild(body);
  wrap.appendChild(table);
  return wrap;
}

type ArtifactPreview = NonNullable<DocumentEntry["previews"]>[number];

/**
 * Render an `ArtifactPreview` to DOM nodes. Tables get rendered as
 * real `<table>` elements with caption + headers + rows. Markdown
 * bodies go through `renderMarkdownBlocks` which handles headings,
 * paragraphs, lists, code, and inline tables -- enough for the
 * structured docs we ship as previews (testbench.md is the main
 * case; agents may add `## Sequencers` / `## Drivers` sections).
 */
function renderPreview(preview: ArtifactPreview): Node[] {
  if (preview.kind === "table") {
    const out: Node[] = [];
    if (preview.caption && preview.caption.trim().length > 0) {
      out.push(el("h4", {}, preview.caption));
    }
    const tbl = el("table", {});
    const thead = el("thead", {}, el("tr", {}, ...preview.headers.map((h) => el("th", {}, h))));
    tbl.appendChild(thead);
    const tbody = el("tbody", {});
    for (const row of preview.rows) {
      tbody.appendChild(el("tr", {}, ...row.map((cell) => el("td", {}, cell))));
    }
    tbl.appendChild(tbody);
    out.push(tbl);
    return out;
  }
  return renderMarkdownBlocks(preview.body);
}

/**
 * Minimal markdown -> DOM renderer covering what our generated
 * docs actually use: ATX headings (`#`..`######`), paragraphs,
 * bullet lists (`-` / `*`), numbered lists (`1.`), tables, fenced
 * code blocks, inline `code`, `**bold**`, `*italic*`. NOT a
 * spec-conformant renderer -- nested lists, links, blockquotes,
 * setext headings, etc. fall through as plain text. We never ship
 * untrusted markdown so the simplification is OK; the goal is
 * "what does decomposition.md look like" not "render the GFM
 * spec". HTML-escaping happens via `document.createTextNode` /
 * the `el` helper, never `innerHTML`.
 */
function renderMarkdownBlocks(source: string): Node[] {
  const lines = source.split("\n");
  const out: Node[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const trimmed = line.trim();

    if (trimmed === "") {
      i++;
      continue;
    }

    const headingMatch = /^(#{1,6})\s+(.+?)\s*#*\s*$/.exec(trimmed);
    if (headingMatch) {
      const level = Math.min(6, headingMatch[1].length);
      const tag = `h${Math.min(4, level)}` as "h1" | "h2" | "h3" | "h4";
      out.push(el(tag, {}, ...renderInlineMd(headingMatch[2])));
      i++;
      continue;
    }

    if (/^```/.test(trimmed)) {
      const fenceLines: string[] = [];
      i++;
      while (i < lines.length && !/^```/.test(lines[i].trim())) {
        fenceLines.push(lines[i]);
        i++;
      }
      if (i < lines.length) {
        i++;
      }
      out.push(el("pre", {}, el("code", {}, fenceLines.join("\n"))));
      continue;
    }

    // Table block: header + separator + rows.
    if (
      /^\|.*\|$/.test(trimmed) &&
      i + 1 < lines.length &&
      /^\|[\s|:-]+\|$/.test(lines[i + 1].trim())
    ) {
      const headers = splitMdRow(trimmed);
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && /^\|.*\|$/.test(lines[i].trim())) {
        rows.push(splitMdRow(lines[i].trim()));
        i++;
      }
      const tbl = el(
        "table",
        {},
        el("thead", {}, el("tr", {}, ...headers.map((h) => el("th", {}, ...renderInlineMd(h))))),
        el(
          "tbody",
          {},
          ...rows.map((r) =>
            el("tr", {}, ...r.map((cell) => el("td", {}, ...renderInlineMd(cell)))),
          ),
        ),
      );
      out.push(tbl);
      continue;
    }

    // List block (bullet or numbered). Consume contiguous list lines.
    const bulletRe = /^\s*[-*]\s+(.+)$/;
    const numRe = /^\s*\d+\.\s+(.+)$/;
    if (bulletRe.test(line) || numRe.test(line)) {
      const ordered = numRe.test(line) && !bulletRe.test(line);
      const items: string[] = [];
      while (i < lines.length) {
        const m = (ordered ? numRe : bulletRe).exec(lines[i]);
        if (!m) {
          break;
        }
        items.push(m[1]);
        i++;
      }
      const tag = ordered ? "ol" : "ul";
      out.push(el(tag, {}, ...items.map((it) => el("li", {}, ...renderInlineMd(it)))));
      continue;
    }

    // Paragraph: gather contiguous non-blank, non-block lines.
    const paragraphLines: string[] = [line];
    i++;
    while (i < lines.length) {
      const next = lines[i];
      if (next.trim() === "") {
        break;
      }
      if (/^#{1,6}\s+/.test(next.trim())) {
        break;
      }
      if (/^```/.test(next.trim())) {
        break;
      }
      if (/^\s*[-*]\s+/.test(next) || /^\s*\d+\.\s+/.test(next)) {
        break;
      }
      paragraphLines.push(next);
      i++;
    }
    out.push(el("p", {}, ...renderInlineMd(paragraphLines.join(" "))));
  }
  return out;
}

function splitMdRow(line: string): string[] {
  return line
    .trim()
    .replace(/^\||\|$/g, "")
    .split("|")
    .map((c) => c.trim());
}

/**
 * Inline markdown -> DOM node array. Handles `**bold**`,
 * `*italic*`, `` `code` ``. Order matters: code first (so its
 * contents aren't re-parsed for emphasis), then bold (longest
 * delimiter), then italic. Anything we can't parse falls through
 * as a text node so the output is always safe to embed.
 */
function renderInlineMd(text: string): Node[] {
  const out: Node[] = [];
  let rest = text;
  // Each iteration consumes one syntactic element from the head.
  while (rest.length > 0) {
    // Inline code: backticks. Greedy match for `` `text` `` only;
    // doubled backticks are rare in our generated docs.
    const codeMatch = /^`([^`]+)`/.exec(rest);
    if (codeMatch) {
      out.push(el("code", {}, codeMatch[1]));
      rest = rest.slice(codeMatch[0].length);
      continue;
    }
    const boldMatch = /^\*\*([^*]+)\*\*/.exec(rest);
    if (boldMatch) {
      out.push(el("strong", {}, ...renderInlineMd(boldMatch[1])));
      rest = rest.slice(boldMatch[0].length);
      continue;
    }
    const italicMatch = /^\*([^*]+)\*/.exec(rest);
    if (italicMatch) {
      out.push(el("em", {}, ...renderInlineMd(italicMatch[1])));
      rest = rest.slice(italicMatch[0].length);
      continue;
    }
    // Consume up to the next inline marker (or to end of string)
    // as a literal text node. The lookbehind on `**` prevents the
    // `*` branch from also matching it twice.
    const nextMarker = rest.search(/[`*]/);
    if (nextMarker < 0) {
      out.push(document.createTextNode(rest));
      rest = "";
    } else if (nextMarker === 0) {
      // Unmatched `*` / `` ` `` -- emit it as plain text and
      // advance one char so we don't loop forever.
      out.push(document.createTextNode(rest[0]));
      rest = rest.slice(1);
    } else {
      out.push(document.createTextNode(rest.slice(0, nextMarker)));
      rest = rest.slice(nextMarker);
    }
  }
  return out;
}

/** Estimate tokens at ~4 characters per token. Rough but
 *  consistent across model families; what we want is a relative
 *  signal so the user can spot a 100K-token spec next to a
 *  4K-token plan, not a precise count. */
function approxTokens(bytes: number): string {
  if (bytes === 0) {
    return "0";
  }
  const tokens = Math.round(bytes / 4);
  if (tokens < 1000) {
    return `~${tokens}`;
  }
  if (tokens < 1_000_000) {
    return `~${(tokens / 1000).toFixed(1)}K`;
  }
  return `~${(tokens / 1_000_000).toFixed(1)}M`;
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
  const wrap = el("div", { class: "critique-summary" });
  const headline = critique.hasBlocking ? "Critique: blocking" : "Critique: clean";
  const counts: string[] = [];
  if (blocker.length > 0) {
    counts.push(`${blocker.length} BLOCKER`);
  }
  if (unresolved.length > 0) {
    counts.push(`${unresolved.length} UNRESOLVED`);
  }
  if (resolved.length > 0) {
    counts.push(`${resolved.length} RESOLVED`);
  }
  const countSuffix = counts.length > 0 ? ` (${counts.join(", ")})` : "";
  wrap.appendChild(el("strong", {}, headline + countSuffix));
  if (critique.findings.length === 0) {
    wrap.appendChild(
      el(
        "p",
        { class: "empty" },
        "Critique recorded no findings -- this step had nothing to flag.",
      ),
    );
    return wrap;
  }
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
  wrap.appendChild(list);
  return wrap;
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
