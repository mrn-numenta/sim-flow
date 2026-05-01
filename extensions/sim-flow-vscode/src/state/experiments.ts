// Read-only access to `.sim-flow/experiments.db` for the dashboard's
// Experiments and Baselines tabs. The Rust side owns writes; this
// module opens the DB in read-only mode and mirrors the RunFilter /
// RunRow / BaselineRecord shapes from src/cli/types.ts so the UI can
// use the same types whether data came from the CLI (via
// `SimFlowCli.runs()`) or directly from disk.

import Database, { type Database as BetterSqliteDatabase } from "better-sqlite3";
import * as path from "node:path";
import { existsSync } from "node:fs";

import type { BaselineRecord, RunFilter, RunRow } from "../cli/types";

export const EXPERIMENTS_DB = path.join(".sim-flow", "experiments.db");

export function experimentsDbPath(projectDir: string): string {
  return path.join(projectDir, EXPERIMENTS_DB);
}

/**
 * Open a read-only handle to the experiments DB. Callers MUST call
 * `.close()` when finished (use the {@link withExperiments} helper to
 * scope the handle to a callback automatically).
 *
 * Returns `null` if the file does not exist (normal for a freshly-init
 * project that has not recorded any runs yet).
 */
export function openExperiments(projectDir: string): ExperimentsReader | null {
  const file = experimentsDbPath(projectDir);
  if (!existsSync(file)) {
    return null;
  }
  const db = new Database(file, { readonly: true, fileMustExist: true });
  return new ExperimentsReader(db);
}

/** Scoped open: pass the reader to a callback, close it on exit. */
export async function withExperiments<T>(
  projectDir: string,
  callback: (reader: ExperimentsReader | null) => Promise<T> | T,
): Promise<T> {
  const reader = openExperiments(projectDir);
  try {
    return await callback(reader);
  } finally {
    reader?.close();
  }
}

export class ExperimentsReader {
  constructor(private readonly db: BetterSqliteDatabase) {}

  close(): void {
    this.db.close();
  }

  /** Count of rows in the `runs` table, ignoring filters. */
  countRuns(): number {
    const row = this.db.prepare("SELECT COUNT(*) AS n FROM runs").get() as { n: number };
    return row.n;
  }

  /** Apply the same filter surface as {@link SimFlowCli.runs}. */
  listRuns(filter: RunFilter = {}): RunRow[] {
    const clauses: string[] = [];
    const params: Record<string, string | number> = {};
    if (filter.workload) {
      clauses.push("workload = @workload");
      params.workload = filter.workload;
    }
    if (filter.candidate) {
      clauses.push("candidate = @candidate");
      params.candidate = filter.candidate;
    }
    if (filter.study) {
      clauses.push("study = @study");
      params.study = filter.study;
    }
    if (filter.sweep) {
      clauses.push("parent_run_id = @sweep");
      params.sweep = filter.sweep;
    }
    const where = clauses.length > 0 ? `WHERE ${clauses.join(" AND ")}` : "";
    const limit = typeof filter.limit === "number" ? `LIMIT ${filter.limit | 0}` : "";
    const sql = `SELECT ${RUN_COLUMNS} FROM runs ${where} ORDER BY id DESC ${limit}`;
    const rows = this.db.prepare(sql).all(params) as RawRunRow[];
    return rows.map(normalizeRunRow);
  }

  /** Fetch a run by its run_id. Returns `null` if not found. */
  getRun(runId: string): RunRow | null {
    const sql = `SELECT ${RUN_COLUMNS} FROM runs WHERE run_id = @run_id LIMIT 1`;
    const row = this.db.prepare(sql).get({ run_id: runId }) as RawRunRow | undefined;
    return row ? normalizeRunRow(row) : null;
  }

  /** List every baseline in insertion order. */
  listBaselines(): BaselineRecord[] {
    const rows = this.db
      .prepare("SELECT name, run_id, timestamp FROM baselines ORDER BY id ASC")
      .all() as BaselineRecord[];
    return rows;
  }
}

// -------------------------------------------------------------
// Internal normalization
// -------------------------------------------------------------

/**
 * Columns selected from the `runs` table. Keep this list in sync with
 * the schema defined in `tools/sim-flow/src/tracking/index.rs`.
 */
const RUN_COLUMNS = [
  "id",
  "run_id",
  "timestamp",
  "git_commit",
  "git_branch",
  "git_dirty",
  "config_fingerprint",
  "manifest_path",
  "workload",
  "candidate",
  "study",
  "metrics_summary",
  "parent_run_id",
  "sweep_parameter",
  "sweep_value",
  "tags",
  "notes",
  "lifecycle",
].join(", ");

interface RawRunRow {
  id: number;
  run_id: string;
  timestamp: string;
  git_commit: string;
  git_branch: string | null;
  git_dirty: number | bigint;
  config_fingerprint: string;
  manifest_path: string | null;
  workload: string | null;
  candidate: string | null;
  study: string | null;
  metrics_summary: string | null;
  parent_run_id: string | null;
  sweep_parameter: string | null;
  sweep_value: string | null;
  tags: string | null;
  notes: string | null;
  lifecycle: string;
}

function normalizeRunRow(raw: RawRunRow): RunRow {
  return {
    id: Number(raw.id),
    run_id: raw.run_id,
    timestamp: raw.timestamp,
    git_commit: raw.git_commit,
    git_branch: raw.git_branch,
    git_dirty: Number(raw.git_dirty) !== 0,
    config_fingerprint: raw.config_fingerprint,
    manifest_path: raw.manifest_path,
    workload: raw.workload,
    candidate: raw.candidate,
    study: raw.study,
    metrics_summary: raw.metrics_summary,
    parent_run_id: raw.parent_run_id,
    sweep_parameter: raw.sweep_parameter,
    sweep_value: raw.sweep_value,
    tags: raw.tags,
    notes: raw.notes,
    lifecycle: raw.lifecycle,
  };
}
