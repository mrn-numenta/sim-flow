import Database from "better-sqlite3";
import { promises as fs } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { ExperimentsReader, experimentsDbPath, openExperiments } from "./experiments";

const SCHEMA = `
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL UNIQUE,
    timestamp TEXT NOT NULL,
    git_commit TEXT NOT NULL,
    git_branch TEXT,
    git_dirty INTEGER NOT NULL DEFAULT 0,
    config_fingerprint TEXT NOT NULL,
    manifest_path TEXT,
    workload TEXT,
    candidate TEXT,
    study TEXT,
    metrics_summary TEXT,
    parent_run_id TEXT,
    sweep_parameter TEXT,
    sweep_value TEXT,
    tags TEXT,
    notes TEXT,
    lifecycle TEXT NOT NULL DEFAULT 'active'
);
CREATE TABLE baselines (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    run_id TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    notes TEXT
);
`;

let tmpRoot: string;
let projectDir: string;
let dbPath: string;

beforeEach(async () => {
  tmpRoot = await fs.mkdtemp(path.join(os.tmpdir(), "sim-flow-exp-"));
  projectDir = path.join(tmpRoot, "proj");
  await fs.mkdir(path.join(projectDir, ".sim-flow"), { recursive: true });
  dbPath = experimentsDbPath(projectDir);
  const db = new Database(dbPath);
  db.exec(SCHEMA);
  db.close();
});

afterEach(async () => {
  await fs.rm(tmpRoot, { recursive: true, force: true });
});

function seedRuns(rows: Array<Partial<Record<string, unknown>> & { run_id: string }>) {
  const db = new Database(dbPath);
  const insert = db.prepare(
    `INSERT INTO runs (
       run_id, timestamp, git_commit, git_branch, git_dirty,
       config_fingerprint, manifest_path, workload, candidate,
       study, metrics_summary, parent_run_id, sweep_parameter,
       sweep_value, tags, notes, lifecycle
     ) VALUES (
       @run_id, @timestamp, @git_commit, @git_branch, @git_dirty,
       @config_fingerprint, @manifest_path, @workload, @candidate,
       @study, @metrics_summary, @parent_run_id, @sweep_parameter,
       @sweep_value, @tags, @notes, @lifecycle
     )`,
  );
  for (const row of rows) {
    insert.run({
      timestamp: "t",
      git_commit: "c",
      git_branch: null,
      git_dirty: 0,
      config_fingerprint: "fp",
      manifest_path: null,
      workload: null,
      candidate: null,
      study: null,
      metrics_summary: null,
      parent_run_id: null,
      sweep_parameter: null,
      sweep_value: null,
      tags: null,
      notes: null,
      lifecycle: "active",
      ...row,
    });
  }
  db.close();
}

function seedBaselines(rows: Array<{ name: string; run_id: string; timestamp?: string }>) {
  const db = new Database(dbPath);
  const insert = db.prepare(
    "INSERT INTO baselines (name, run_id, timestamp) VALUES (@name, @run_id, @timestamp)",
  );
  for (const row of rows) {
    insert.run({ timestamp: "t", ...row });
  }
  db.close();
}

describe("openExperiments", () => {
  it("returns null when the DB does not exist", async () => {
    const emptyProject = path.join(tmpRoot, "no-db");
    await fs.mkdir(path.join(emptyProject, ".sim-flow"), { recursive: true });
    const reader = openExperiments(emptyProject);
    expect(reader).toBeNull();
  });

  it("opens an existing DB and closes cleanly", () => {
    seedRuns([{ run_id: "001-a" }]);
    const reader = openExperiments(projectDir);
    expect(reader).toBeInstanceOf(ExperimentsReader);
    expect(reader?.countRuns()).toBe(1);
    reader?.close();
  });
});

describe("ExperimentsReader.listRuns", () => {
  it("returns rows newest-first", () => {
    seedRuns([
      { run_id: "001-a", workload: "wk" },
      { run_id: "002-b", workload: "wk" },
      { run_id: "003-c", workload: "other" },
    ]);
    const reader = openExperiments(projectDir)!;
    const rows = reader.listRuns();
    expect(rows.map((r) => r.run_id)).toEqual(["003-c", "002-b", "001-a"]);
    reader.close();
  });

  it("filters by workload", () => {
    seedRuns([
      { run_id: "001-a", workload: "wk" },
      { run_id: "002-b", workload: "other" },
    ]);
    const reader = openExperiments(projectDir)!;
    const rows = reader.listRuns({ workload: "wk" });
    expect(rows).toHaveLength(1);
    expect(rows[0].run_id).toBe("001-a");
    reader.close();
  });

  it("filters sweep children by parent_run_id", () => {
    seedRuns([
      { run_id: "001-parent", sweep_parameter: "buffer_depth" },
      { run_id: "002-child-4", parent_run_id: "001-parent", sweep_value: "4" },
      { run_id: "003-child-8", parent_run_id: "001-parent", sweep_value: "8" },
      { run_id: "004-other" },
    ]);
    const reader = openExperiments(projectDir)!;
    const rows = reader.listRuns({ sweep: "001-parent" });
    expect(rows.map((r) => r.run_id)).toEqual(["003-child-8", "002-child-4"]);
    reader.close();
  });

  it("applies the limit", () => {
    seedRuns([{ run_id: "001" }, { run_id: "002" }, { run_id: "003" }]);
    const reader = openExperiments(projectDir)!;
    const rows = reader.listRuns({ limit: 2 });
    expect(rows).toHaveLength(2);
    reader.close();
  });

  it("normalizes git_dirty to a boolean", () => {
    seedRuns([{ run_id: "001", git_dirty: 1 }]);
    const reader = openExperiments(projectDir)!;
    const [row] = reader.listRuns();
    expect(row.git_dirty).toBe(true);
    expect(typeof row.git_dirty).toBe("boolean");
    reader.close();
  });
});

describe("ExperimentsReader.getRun", () => {
  it("returns null for an unknown run_id", () => {
    seedRuns([{ run_id: "001" }]);
    const reader = openExperiments(projectDir)!;
    expect(reader.getRun("999")).toBeNull();
    reader.close();
  });

  it("returns a single row for a known run_id", () => {
    seedRuns([{ run_id: "001-a", workload: "w" }]);
    const reader = openExperiments(projectDir)!;
    const row = reader.getRun("001-a");
    expect(row?.workload).toBe("w");
    reader.close();
  });
});

describe("ExperimentsReader.listBaselines", () => {
  it("returns baselines in insertion order", () => {
    seedRuns([{ run_id: "001" }, { run_id: "002" }]);
    seedBaselines([
      { name: "v1", run_id: "001" },
      { name: "v2", run_id: "002" },
    ]);
    const reader = openExperiments(projectDir)!;
    const baselines = reader.listBaselines();
    expect(baselines.map((b) => b.name)).toEqual(["v1", "v2"]);
    reader.close();
  });
});
