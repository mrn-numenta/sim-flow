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
let quickPickResult: { dir?: string; pickKind?: "project" | "new" } | undefined;
const errorMessages: string[] = [];
let getConfigurationValue: string | undefined;
let activeTextEditorPath: string | undefined;

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
      return activeTextEditorPath
        ? { document: { uri: { fsPath: activeTextEditorPath } } }
        : undefined;
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

const { PICK_PROJECT_NEW, findProjectCandidates, pickProject, resolveContext, resolveProjectDir } =
  await import("./context");

function makeProject(dir: string): void {
  fs.mkdirSync(path.join(dir, ".sim-flow"), { recursive: true });
  fs.writeFileSync(path.join(dir, ".sim-flow", "state.toml"), 'flow = "direct-modeling"\n');
}

function setupSimModelsWorkspace(root: string): string {
  const simModelsRoot = path.join(root, "sim-models");
  fs.mkdirSync(path.join(simModelsRoot, "docs", "modeling-guide"), { recursive: true });
  fs.mkdirSync(path.join(simModelsRoot, "examples"), { recursive: true });
  workspaceFolders = [{ uri: { fsPath: simModelsRoot }, name: "sim-models", index: 0 }];
  return simModelsRoot;
}

let tmpRoot: string;

beforeEach(() => {
  tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), "sim-flow-ctx-"));
  workspaceFolders = [];
  findFilesResults = [];
  quickPickResult = undefined;
  errorMessages.length = 0;
  getConfigurationValue = undefined;
  activeTextEditorPath = undefined;
});

afterEach(() => {
  fs.rmSync(tmpRoot, { recursive: true, force: true });
});

describe("resolveContext with --project hint", () => {
  it("trusts a valid hint and skips workspace scanning", async () => {
    const simModelsRoot = setupSimModelsWorkspace(tmpRoot);
    const projectA = path.join(simModelsRoot, "users", "tester", "model-a");
    makeProject(projectA);

    const ctx = await resolveContext({ projectDir: projectA });
    expect(ctx).not.toBeNull();
    expect(ctx!.projectDir).toBe(projectA);
    expect(errorMessages).toEqual([]);
  });

  it("rejects a hint outside the allowed sim-models project roots", async () => {
    setupSimModelsWorkspace(tmpRoot);
    const notAProject = path.join(tmpRoot, "sim-foundation", "scratch");
    makeProject(notAProject);

    const ctx = await resolveContext({ projectDir: notAProject });
    expect(ctx).toBeNull();
    expect(errorMessages[0]).toContain("--project path");
    expect(errorMessages[0]).toContain(notAProject);
  });
});

describe("findProjectCandidates", () => {
  it("discovers only sim-models user and library projects and dedupes", async () => {
    const simModelsRoot = setupSimModelsWorkspace(tmpRoot);
    const a = path.join(simModelsRoot, "users", "tester", "model-a");
    const b = path.join(simModelsRoot, "library", "model-b");
    const ignored = path.join(tmpRoot, "sim-foundation", "model-c");
    makeProject(a);
    makeProject(b);
    makeProject(ignored);
    findFilesResults = [
      { fsPath: path.join(a, ".sim-flow", "state.toml") },
      { fsPath: path.join(b, ".sim-flow", "state.toml") },
      { fsPath: path.join(ignored, ".sim-flow", "state.toml") },
      { fsPath: path.join(a, ".sim-flow", "state.toml") }, // duplicate, should dedupe
    ];

    const found = await findProjectCandidates();
    expect(found).toEqual([a, b].sort());
  });

  it("returns no candidates when sim-models is not in the workspace", async () => {
    const rootProject = path.join(tmpRoot, "rootish");
    makeProject(rootProject);
    workspaceFolders = [
      { uri: { fsPath: path.join(tmpRoot, "sim-foundation") }, name: "sim-foundation", index: 0 },
    ];
    findFilesResults = [{ fsPath: path.join(rootProject, ".sim-flow", "state.toml") }];

    expect(await findProjectCandidates()).toEqual([]);
  });
});

describe("resolveProjectDir", () => {
  it("walks up from the active editor's file path to find .sim-flow", () => {
    const simModelsRoot = setupSimModelsWorkspace(tmpRoot);
    const proj = path.join(simModelsRoot, "users", "tester", "model-x");
    makeProject(proj);
    // Active editor inside a deeply-nested src/ file.
    const nestedFile = path.join(proj, "src", "deep", "lib.rs");
    fs.mkdirSync(path.dirname(nestedFile), { recursive: true });
    fs.writeFileSync(nestedFile, "// stub\n");
    activeTextEditorPath = nestedFile;
    expect(resolveProjectDir()).toBe(proj);
  });

  it("falls back to the sim-models workspace root when no project is initialized yet", () => {
    const simModelsRoot = setupSimModelsWorkspace(tmpRoot);
    expect(resolveProjectDir()).toBe(simModelsRoot);
  });

  it("does not treat an arbitrary single-root workspace as a project", () => {
    const root = path.join(tmpRoot, "fresh-workspace");
    fs.mkdirSync(root, { recursive: true });
    workspaceFolders = [{ uri: { fsPath: root }, name: "fresh-workspace", index: 0 }];
    expect(resolveProjectDir()).toBeUndefined();
  });

  it("returns undefined with no active editor AND multiple workspace folders", () => {
    const simModelsRoot = setupSimModelsWorkspace(tmpRoot);
    workspaceFolders = [
      { uri: { fsPath: simModelsRoot }, name: "sim-models", index: 0 },
      { uri: { fsPath: path.join(tmpRoot, "sim-foundation") }, name: "sim-foundation", index: 1 },
    ];
    expect(resolveProjectDir()).toBeUndefined();
  });

  it("returns undefined with no editor and no workspace folders", () => {
    expect(resolveProjectDir()).toBeUndefined();
  });

  it("ignores projects outside sim-models when walking up from the active editor", () => {
    const simModelsRoot = setupSimModelsWorkspace(tmpRoot);
    const disallowed = path.join(tmpRoot, "sim-foundation", "scratch");
    makeProject(disallowed);
    workspaceFolders = [
      { uri: { fsPath: simModelsRoot }, name: "sim-models", index: 0 },
      { uri: { fsPath: path.join(tmpRoot, "sim-foundation") }, name: "sim-foundation", index: 1 },
    ];
    const nestedFile = path.join(disallowed, "src", "main.rs");
    fs.mkdirSync(path.dirname(nestedFile), { recursive: true });
    fs.writeFileSync(nestedFile, "// stub\n");
    activeTextEditorPath = nestedFile;

    expect(resolveProjectDir()).toBeUndefined();
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
    quickPickResult = { pickKind: "project", dir: "/tmp/b" };
    expect(await pickProject(["/tmp/a", "/tmp/b"])).toBe("/tmp/b");
  });

  it("returns the new-project sentinel when the user picks $(plus) New project under allowNew", async () => {
    quickPickResult = { pickKind: "new" };
    expect(await pickProject(["/tmp/only-one"], { allowNew: true })).toBe(PICK_PROJECT_NEW);
  });

  it("does not auto-select the single candidate when allowNew is set", async () => {
    quickPickResult = { pickKind: "project", dir: "/tmp/only-one" };
    // Without allowNew this short-circuits with no UI; with allowNew we
    // always show the picker so the user can choose New project.
    expect(await pickProject(["/tmp/only-one"], { allowNew: true })).toBe("/tmp/only-one");
  });
});
