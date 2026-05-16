// TypeScript shapes that mirror the JSON output contract documented in
// docs/architecture/ai-flow/cli-json.md. The Rust crate owns the
// canonical schemas; this file must stay in lock-step. Bump and update
// both if the JSON output changes.

export type Flow = "direct-modeling" | "design-study" | "systemverilog-convert";

export interface Gate {
  passed: boolean;
  timestamp: string | null;
  candidates: Record<string, Gate>;
}

export interface StatusResult {
  flow: Flow;
  current_step: string;
  started: string | null;
  gates: Record<string, Gate>;
  archived_gates: Record<string, Record<string, Gate>>;
}

export interface RunRow {
  id: number;
  run_id: string;
  timestamp: string;
  git_commit: string;
  git_branch: string | null;
  git_dirty: boolean;
  config_fingerprint: string;
  manifest_path: string | null;
  workload: string | null;
  candidate: string | null;
  study: string | null;
  /** JSON-encoded string per cli-json.md; callers must JSON.parse if structured access is needed. */
  metrics_summary: string | null;
  parent_run_id: string | null;
  sweep_parameter: string | null;
  sweep_value: string | null;
  tags: string | null;
  notes: string | null;
  lifecycle: string;
}

export interface RunFilter {
  workload?: string;
  candidate?: string;
  study?: string;
  /** Parent run id when filtering sweep variants. */
  sweep?: string;
  limit?: number;
}

export interface GateFailure {
  description: string;
  reason: string;
}

export interface GateResult {
  step: string;
  clean: boolean;
  failures: GateFailure[];
}

export interface BaselineRecord {
  name: string;
  run_id: string;
  timestamp: string;
}

export interface DeltaEntry {
  metric: string;
  baseline: number | null;
  current: number | null;
  delta: number | null;
  delta_pct: number | null;
}

export interface BaselineDelta {
  baseline_run_id: string;
  current_run_id: string;
  entries: DeltaEntry[];
}

export interface NewModelOptions {
  name: string;
  destination?: string;
  libraryPath?: string;
  skipCargoCheck?: boolean;
}

export interface NewModelResult {
  project_dir: string;
  crate_name: string;
  next_step: string;
}

/** Mirrors the `gate_checks` shape in `sim-flow describe --json`. */
export interface DescribeGateCheck {
  kind: string;
  description: string;
  path?: string;
  pattern?: string;
  cmd?: string;
  args?: string[];
}

/**
 * Step descriptor returned by `sim-flow describe <step>.<kind> --json`.
 * Single source of truth for everything the extension previously
 * duplicated (instruction slug, artifact paths, predecessor inputs,
 * gate checks).
 */
export interface DescribeResult {
  step: string;
  kind: "work" | "critique";
  flow: Flow;
  prerequisite: string | null;
  instruction_path: string;
  instruction_body: string;
  work_artifacts: string[];
  predecessor_inputs: string[];
  per_candidate: boolean;
  gate_checks: DescribeGateCheck[];
}

export interface AdvanceResult {
  step: string;
  clean: boolean;
  advanced: boolean;
  next_step: string | null;
  failures: GateFailure[];
}

/**
 * Per-prompt entry returned by `sim-flow prompts list --json`. Drives
 * the dashboard's Prompts tab: each row shows the active scope plus
 * which scopes currently hold an override file.
 */
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
