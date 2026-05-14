// Typed message protocol for the perf-data panel.
//
// The panel watches `.sim-flow/experiments.db` and presents a live
// view of recorded runs grouped by study. The host polls the DB on
// any `experiments-db` watcher change and posts a fresh snapshot to
// the webview; the webview is a pure renderer driven by the most
// recent state update.

/** One run row as displayed in the panel. Mirrors the subset of
 * `RunRow` the UI actually needs. Keep it narrow so the message
 * payload stays small.
 */
export interface PerfRunRow {
  run_id: string;
  timestamp: string;
  workload: string | null;
  candidate: string | null;
  study: string | null;
  parent_run_id: string | null;
  manifest_path: string | null;
  notes: string | null;
  tags: string | null;
}

/** A study group: every run that recorded `study = <name>`. */
export interface PerfStudyGroup {
  name: string;
  runs: PerfRunRow[];
}

/** State the host posts to the webview on every refresh. */
export interface PerfPanelState {
  projectDir: string;
  /** ISO-8601 timestamp the host last polled the DB. */
  lastUpdated: string;
  /** Total number of recorded runs in `experiments.db`. */
  totalRuns: number;
  /** Runs grouped by `study`. Runs with no study land in `null`'s group. */
  groups: PerfStudyGroup[];
  /** Runs that don't belong to any study (null `study` column). */
  ungrouped: PerfRunRow[];
  /** True when `.sim-flow/experiments.db` doesn't exist yet. */
  databaseAbsent: boolean;
  /** Last diff result rendered in the panel (markdown), if any. */
  diffMarkdown: string | null;
  /** Status string for the diff invocation (running / ok / error). */
  diffStatus: string | null;
}

export type HostMessage = { type: "state-update"; state: PerfPanelState };

export type WebviewMessage =
  | { type: "request-refresh" }
  | { type: "run-diff"; lhs: string; rhs: string };
