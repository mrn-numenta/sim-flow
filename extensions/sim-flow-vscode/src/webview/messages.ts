// Type-only message protocol between the extension host and the Flow
// Dashboard webview. Both sides import this file; neither compiles it
// to runtime code because every export is a type.
//
// - HostMessage: host -> webview
// - WebviewMessage: webview -> host

import type { BaselineRecord, GateResult, RunRow } from "../cli/types";
import type { StepMode } from "../session/protocol-types";
import type { CritiqueFile, FlowState, PlanProgress } from "../state/types";

export type { StepMode } from "../session/protocol-types";

export type { PlanMilestone, PlanProgress } from "../state/types";

/** Everything the dashboard needs for a render. */
export interface DashboardState {
  /** Absolute path to the project whose state is rendered. */
  projectDir: string;
  /** Parsed state.toml. */
  flow: FlowState;
  /** Critique files present on disk. */
  critiques: CritiqueFile[];
  /** Most-recent runs (capped by the host; see MAX_DASHBOARD_RUNS). */
  runs: RunRow[];
  /** All baselines. */
  baselines: BaselineRecord[];
  /**
   * Optional gate-check report for the currently-selected step. The
   * host populates this on demand when the user selects a step in the
   * rail. Not streamed on every refresh to avoid repeated cargo test
   * runs.
   */
  currentStepGate?: GateResult;
  /** Project documents (generated artifacts + critiques + source spec). */
  documents: DocumentEntry[];
  /**
   * Per-milestone progress for the current step, when that step
   * executes a plan (DM2d, DM3c, DM4b). For other steps `kind`
   * is "none" and the dashboard hides the progress row.
   */
  planProgress: PlanProgress;
  /**
   * Persisted spec-path the user typed into the Spec field on a
   * prior session. Empty string when no spec was ever recorded
   * for this project. The webview seeds its `specPath` input from
   * this value on first state-update; subsequent edits flow back
   * via the `set-spec-path` webview message and are stored in
   * `workspaceState`, scoped per project.
   */
  specPath: string;
  /**
   * Mirrors `sim-flow.dashboard.showFullyAutomated` -- the workspace
   * setting that controls visibility of the red end-to-end button.
   * Hidden by default so casual clicks can't kick off a long
   * unattended run; the user explicitly opts in via the Settings
   * tab checkbox.
   */
  fullyAutomatedEnabled: boolean;
  /**
   * Mirrors `sim-flow.dashboard.verilogSimEnabled`. When true and
   * `verilogSimulatorPath` is non-empty, the host appends a
   * "Simulate and iterate" section to the Generate Verilog prompt
   * before injecting it. Off by default -- emission alone is the
   * baseline behavior.
   */
  verilogSimEnabled: boolean;
  /** Mirrors `sim-flow.dashboard.verilogSimulatorPath`; empty when unset. */
  verilogSimulatorPath: string;
  /**
   * Current step-axis mode. When a manual-mode pump is attached this
   * is the orchestrator's truth (last `StepModeChanged` echo);
   * otherwise it falls back to the `sim-flow.flow.stepMode` setting.
   * The dashboard's toggle between Connect and Disconnect reads this
   * and emits a `set-step-mode` webview message on click.
   */
  stepMode: StepMode;
  /**
   * True when a `SocketSessionPump` is alive for this project. When
   * false, toggle changes only update the persisted setting (no
   * `SetStepMode` round-trip is possible) and the per-step buttons
   * fall back to their legacy chat-tab spawn path.
   */
  sessionActive: boolean;
  /**
   * True while the orchestrator is inside a sub-session (Work or
   * Critique). Driven by the pump's `sub-session-started` /
   * `sub-session-ended` bracket events. Per-step buttons (Run Step,
   * Run Critique, Run Gate, Advance) are disabled while this is
   * true so the user can't queue commands the orchestrator will
   * reject mid-flight. Reset stays enabled — the user may need to
   * recover from a stuck sub-session.
   */
  inSubSession: boolean;
  /** Timestamp of this snapshot (ISO-8601 UTC). */
  generatedAt: string;
  /** Sim-flow CLI version, if resolvable. */
  cliVersion?: string;
}

/**
 * One row in the Documents tab. Covers both per-step generated
 * artifacts (work outputs + critique files) and the ingested source
 * spec. The dashboard renders rows in `category` groups, sorted by
 * step then path.
 */
export interface DocumentEntry {
  /** Absolute path the host can pass to `vscode.window.showTextDocument`. */
  absPath: string;
  /** Project-relative path for display. */
  relPath: string;
  /** Bucket for grouping in the UI. */
  category: "work-artifact" | "critique" | "source-spec" | "spec-page" | "other";
  /** Step id this document is associated with, when applicable. */
  step?: string;
  /** File size in bytes, or null when the file does not exist on disk. */
  bytes: number | null;
  /** Last modification time in ISO-8601 UTC, or null when missing. */
  modifiedAt: string | null;
  /** True if the file is on disk; false rows are placeholders for expected outputs. */
  exists: boolean;
  /**
   * Optional inline preview of the file body, populated by the host
   * for select small artifacts where the per-step view should show
   * content directly (e.g. decomposition.md, pipeline-mapping.md
   * summary tables) so the user gets an overview without clicking
   * Open. Capped at ~4 KB of original markdown; UI renders inside
   * a `<pre>`-style code block.
   */
  previewBody?: string;
  /**
   * Line count when the host computed it (currently only for
   * Rust source files under `src/` and `tests/`). The per-step
   * view summarizes "N files / M lines" for code-touching steps
   * (DM2d / DM3b / DM3c / DM4b).
   */
  lineCount?: number;
}

export type HostMessage =
  | { type: "state-update"; state: DashboardState }
  | { type: "error"; message: string; detail?: string }
  | { type: "gate-result"; step: string; result: GateResult }
  | { type: "spec-path-picked"; path: string }
  | { type: "llm-config"; source: LlmSourceTag; model?: string; verbose: boolean }
  | {
      type: "model-list";
      source: LlmSourceTag;
      models: string[];
      /** Populated when the source returned no models for a non-error reason. */
      emptyReason?: string;
      /** Populated when the enumeration call itself failed. */
      error?: string;
    }
  | { type: "block-diagram"; svg: string | null }
  | { type: "prompts-list-result"; entries: PromptListEntry[] };

/**
 * LLM sources fall into two execution modes:
 *
 * - **API backends** -- driven by the chat participant in the
 *   `@sim-flow` chat pane. Support streaming, multimodal, native
 *   tool-use through the orchestrator-mediated `request-llm-response`
 *   protocol.
 *     - vscode (VS Code Language Model API; usually Copilot)
 *     - anthropic (Anthropic Messages API)
 *     - openai (OpenAI Chat Completions)
 *     - ollama (local OpenAI-compat server)
 *     - lmstudio (local OpenAI-compat server)
 *
 * - **CLI agents** -- driven by `sim-flow auto --llm-backend <name>`
 *   in a VS Code terminal tab. They use whatever auth the user's
 *   CLI is already configured with (claude /login, codex login,
 *   gh auth login). The chat participant is bypassed entirely.
 *     - claude-cli (Anthropic's `claude` CLI; uses Pro/Team)
 *     - codex-cli (OpenAI's `codex` CLI)
 *     - gh-copilot-cli (`gh copilot` CLI)
 */
export type LlmSourceTag =
  | "vscode"
  | "anthropic"
  | "openai"
  | "ollama"
  | "lmstudio"
  | "claude-cli"
  | "codex-cli"
  | "gh-copilot-cli";

export const LLM_SOURCE_LABELS: Record<LlmSourceTag, string> = {
  vscode: "VS Code (Copilot)",
  anthropic: "Anthropic",
  openai: "OpenAI",
  ollama: "Ollama (local)",
  lmstudio: "LM Studio (local)",
  "claude-cli": "Claude CLI (terminal)",
  "codex-cli": "Codex CLI (terminal)",
  "gh-copilot-cli": "GitHub Copilot CLI (terminal)",
};

/** True when the source must be driven via a terminal, not the chat pane. */
export function isTerminalLlmSource(source: LlmSourceTag): boolean {
  return source === "claude-cli" || source === "codex-cli" || source === "gh-copilot-cli";
}

/**
 * Map a `*-cli` picker value to the `--llm-backend` argument the
 * sim-flow CLI expects (its agent registry uses the bare names
 * `claude` / `codex` / `gh-copilot`).
 */
export function cliBackendArgFor(source: LlmSourceTag): string {
  switch (source) {
    case "claude-cli":
      return "claude";
    case "codex-cli":
      return "codex";
    case "gh-copilot-cli":
      return "gh-copilot";
    default:
      return source;
  }
}

export interface PromptListEntry {
  slug: string;
  kind: "work" | "critique";
  active_scope: "project" | "global" | "default";
  project_path: string;
  project_present: boolean;
  global_path: string | null;
  global_present: boolean;
  default_path: string;
}

export type WebviewMessage =
  | { type: "ready" }
  | { type: "refresh" }
  | { type: "select-step"; step: string }
  | { type: "run-step"; step: string }
  | { type: "run-critique"; step: string }
  | { type: "gate-step"; step: string }
  | { type: "advance-step"; step: string }
  | { type: "reset-step"; step: string }
  | { type: "open-document"; path: string }
  | { type: "regenerate-block-diagram" }
  | { type: "run-auto"; specPath?: string }
  | { type: "run-auto-end-to-end"; specPath: string }
  | { type: "stop-auto" }
  | { type: "pick-spec-file" }
  | { type: "set-spec-path"; path: string }
  | { type: "set-fully-auto-enabled"; enabled: boolean }
  | { type: "set-verilog-sim-enabled"; enabled: boolean }
  | { type: "set-verilog-simulator-path"; path: string }
  | { type: "switch-project" }
  | { type: "new-project"; name: string }
  | { type: "rename-project" }
  | { type: "set-llm-source"; source: LlmSourceTag }
  | { type: "set-llm-model"; model: string }
  | { type: "request-model-list"; source: LlmSourceTag }
  | { type: "set-llm-verbose"; verbose: boolean }
  | { type: "prompts-list" }
  /**
   * Open a prompt override in a regular VS Code editor tab. The host
   * resolves the override path for `scope` (project or global), seeds
   * the file with the currently-effective prompt content if it
   * doesn't yet exist, and opens it. Saves go through VS Code's
   * normal file save -- the foundation default is never opened, so
   * it can't be accidentally overwritten.
   */
  | {
      type: "prompt-open-in-editor";
      slug: string;
      kind: "work" | "critique";
      scope: "project" | "global";
    }
  | {
      type: "prompt-reset";
      slug: string;
      kind: "work" | "critique";
      scope: "project" | "global" | "all";
    }
  | { type: "open-critique"; step: string }
  | { type: "open-analysis-report" }
  | { type: "generate-verilog" }
  | { type: "set-step-mode"; mode: StepMode };
