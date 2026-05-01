import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

interface FakeWorkspaceFolder {
  uri: { fsPath: string };
  name: string;
  index: number;
}

let workspaceFolders: FakeWorkspaceFolder[] = [];
let findFilesResults: Array<{ fsPath: string }> = [];
let quickPickResult: { dir: string } | undefined;
const errorMessages: string[] = [];
let getConfigurationValue: string | undefined;

vi.mock("vscode", () => ({
  workspace: {
    get workspaceFolders() {
      return workspaceFolders;
    },
    findFiles: async () => findFilesResults,
    getConfiguration: () => ({
      get: () => getConfigurationValue,
    }),
  },
  window: {
    get activeTextEditor() {
      return undefined;
    },
    showErrorMessage: (msg: string) => {
      errorMessages.push(msg);
      return Promise.resolve(undefined);
    },
    showQuickPick: async () => quickPickResult,
  },
}));

vi.mock("./cli", () => ({
  resolveBinary: () => "/usr/local/bin/sim-flow",
  bundledCandidates: () => [],
  setBundledRoot: () => {},
  SimFlowCli: class {
    binary: string;
    projectDir: string;
    foundationRoot: string | undefined;
    constructor(opts: { binary: string; projectDir: string; foundationRoot?: string }) {
      this.binary = opts.binary;
      this.projectDir = opts.projectDir;
      this.foundationRoot = opts.foundationRoot;
    }
  },
  SimFlowCliError: class SimFlowCliError extends Error {},
}));

const { findProjectCandidates, pickProject, resolveContext } = await import("./context");

function makeProject(dir: string): void {
  fs.mkdirSync(path.join(dir, ".sim-flow"), { recursive: true });
  fs.writeFileSync(path.join(dir, ".sim-flow", "state.toml"), 'flow = "direct-modeling"\n');
}

let tmpRoot: string;

beforeEach(() => {
  tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), "sim-flow-ctx-"));
  workspaceFolders = [];
  findFilesResults = [];
  quickPickResult = undefined;
  errorMessages.length = 0;
  getConfigurationValue = undefined;
});

afterEach(() => {
  fs.rmSync(tmpRoot, { recursive: true, force: true });
});

describe("resolveContext with --project hint", () => {
  it("trusts a valid hint and skips workspace scanning", async () => {
    const projectA = path.join(tmpRoot, "model-a");
    makeProject(projectA);

    const ctx = await resolveContext({ projectDir: projectA });
    expect(ctx).not.toBeNull();
    expect(ctx!.projectDir).toBe(projectA);
    expect(errorMessages).toEqual([]);
  });

  it("rejects a hint that does not point at a sim-flow project and surfaces a helpful error", async () => {
    const notAProject = path.join(tmpRoot, "random-dir");
    fs.mkdirSync(notAProject, { recursive: true });

    const ctx = await resolveContext({ projectDir: notAProject });
    expect(ctx).toBeNull();
    expect(errorMessages[0]).toContain("--project path");
    expect(errorMessages[0]).toContain(notAProject);
  });
});

describe("findProjectCandidates", () => {
  it("discovers every .sim-flow/state.toml in the workspace and dedupes", async () => {
    const a = path.join(tmpRoot, "model-a");
    const b = path.join(tmpRoot, "studies", "model-b");
    makeProject(a);
    makeProject(b);
    findFilesResults = [
      { fsPath: path.join(a, ".sim-flow", "state.toml") },
      { fsPath: path.join(b, ".sim-flow", "state.toml") },
      { fsPath: path.join(a, ".sim-flow", "state.toml") }, // duplicate, should dedupe
    ];

    const found = await findProjectCandidates();
    expect(found).toEqual([a, b].sort());
  });

  it("backfills workspace folders whose root has .sim-flow even if findFiles missed them", async () => {
    const rootProject = path.join(tmpRoot, "rootish");
    makeProject(rootProject);
    workspaceFolders = [{ uri: { fsPath: rootProject }, name: "rootish", index: 0 }];
    findFilesResults = [];

    const found = await findProjectCandidates();
    expect(found).toEqual([rootProject]);
  });
});

describe("pickProject", () => {
  it("returns undefined when the candidate list is empty", async () => {
    expect(await pickProject([])).toBeUndefined();
  });

  it("auto-selects the single candidate without prompting", async () => {
    expect(await pickProject(["/tmp/only-one"])).toBe("/tmp/only-one");
  });

  it("delegates to QuickPick when multiple candidates exist", async () => {
    quickPickResult = { dir: "/tmp/b" };
    expect(await pickProject(["/tmp/a", "/tmp/b"])).toBe("/tmp/b");
  });
});
