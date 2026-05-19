import { beforeEach, describe, expect, test, vi } from "vitest";

// All real machinery is in the vscode API itself. Here we mock just
// enough of it to record what patterns the watcher subscribes to and
// to flip each subscription's create/change/delete events so we can
// observe the aggregated `onDidChange` emission shape.

interface FakeWatcher {
  pattern: { base: { fsPath: string }; pattern: string };
  onDidCreate: (cb: (uri: { fsPath: string }) => void) => { dispose: () => void };
  onDidChange: (cb: (uri: { fsPath: string }) => void) => { dispose: () => void };
  onDidDelete: (cb: (uri: { fsPath: string }) => void) => { dispose: () => void };
  dispose: () => void;
  createCbs: Array<(uri: { fsPath: string }) => void>;
  changeCbs: Array<(uri: { fsPath: string }) => void>;
  deleteCbs: Array<(uri: { fsPath: string }) => void>;
  disposed: boolean;
}

let watchers: FakeWatcher[] = [];

function makeWatcher(pattern: { base: { fsPath: string }; pattern: string }): FakeWatcher {
  const w: FakeWatcher = {
    pattern,
    createCbs: [],
    changeCbs: [],
    deleteCbs: [],
    disposed: false,
    onDidCreate(cb) {
      this.createCbs.push(cb);
      return { dispose: () => {} };
    },
    onDidChange(cb) {
      this.changeCbs.push(cb);
      return { dispose: () => {} };
    },
    onDidDelete(cb) {
      this.deleteCbs.push(cb);
      return { dispose: () => {} };
    },
    dispose() {
      this.disposed = true;
    },
  };
  return w;
}

interface FakeEmitter<T> {
  event: (listener: (e: T) => void) => { dispose: () => void };
  fire: (e: T) => void;
  dispose: () => void;
  _listeners: Array<(e: T) => void>;
}

function makeEmitter<T>(): FakeEmitter<T> {
  const em: FakeEmitter<T> = {
    _listeners: [],
    event(listener) {
      this._listeners.push(listener);
      return { dispose: () => {} };
    },
    fire(e) {
      for (const l of this._listeners) l(e);
    },
    dispose() {
      this._listeners.length = 0;
    },
  };
  return em;
}

vi.mock("vscode", () => ({
  Uri: {
    file: (p: string) => ({ fsPath: p }),
  },
  RelativePattern: class {
    base: { fsPath: string };
    pattern: string;
    constructor(base: { fsPath: string }, pattern: string) {
      this.base = base;
      this.pattern = pattern;
    }
  },
  EventEmitter: class<T> {
    private _em = makeEmitter<T>();
    get event() {
      return this._em.event.bind(this._em);
    }
    fire(e: T) {
      this._em.fire(e);
    }
    dispose() {
      this._em.dispose();
    }
  },
  workspace: {
    createFileSystemWatcher: (rel: { base: { fsPath: string }; pattern: string }) => {
      const w = makeWatcher(rel);
      watchers.push(w);
      return w;
    },
  },
}));

const { createStateWatcher } = await import("./watcher");

beforeEach(() => {
  watchers = [];
});

describe("createStateWatcher", () => {
  test("registers a watcher for every state source the dashboard cares about", () => {
    const w = createStateWatcher("/proj");
    // Patterns observed should be a superset of these critical ones.
    const patterns = watchers.map((wr) => wr.pattern.pattern);
    expect(patterns).toContain(".sim-flow/state.toml");
    expect(patterns).toContain("docs/critiques/*.json");
    expect(patterns).toContain("docs/critiques/*.md");
    expect(patterns).toContain(".sim-flow/experiments.db");
    expect(patterns).toContain("docs/impl-plan/*.md");
    expect(patterns).toContain("docs/test-plan/*.md");
    expect(patterns).toContain("docs/perf-plan/*.md");
    expect(patterns).toContain("docs/plan-management.md");
    expect(patterns).toContain(".sim-flow/control.sock");
    // And every watcher is rooted in the project dir.
    for (const wr of watchers) {
      expect(wr.pattern.base.fsPath).toBe("/proj");
    }
    w.dispose();
  });

  test("emits state-toml when state.toml changes", () => {
    const w = createStateWatcher("/proj");
    const events: Array<{ kind: string; uri: { fsPath: string } }> = [];
    w.onDidChange((e) => events.push({ kind: e.kind, uri: e.uri }));
    const stateTomlWatcher = watchers.find(
      (wr) => wr.pattern.pattern === ".sim-flow/state.toml",
    );
    expect(stateTomlWatcher).toBeDefined();
    stateTomlWatcher!.changeCbs[0]({ fsPath: "/proj/.sim-flow/state.toml" });
    expect(events).toEqual([
      { kind: "state-toml", uri: { fsPath: "/proj/.sim-flow/state.toml" } },
    ]);
    w.dispose();
  });

  test("emits critiques for both json and md critique watchers", () => {
    const w = createStateWatcher("/proj");
    const events: string[] = [];
    w.onDidChange((e) => events.push(`${e.kind}:${e.uri.fsPath.split("/").pop()}`));
    const jsonW = watchers.find((wr) => wr.pattern.pattern === "docs/critiques/*.json")!;
    const mdW = watchers.find((wr) => wr.pattern.pattern === "docs/critiques/*.md")!;
    jsonW.createCbs[0]({ fsPath: "/proj/docs/critiques/DM0-critique.json" });
    mdW.changeCbs[0]({ fsPath: "/proj/docs/critiques/DM0-critique.md" });
    expect(events).toEqual([
      "critiques:DM0-critique.json",
      "critiques:DM0-critique.md",
    ]);
    w.dispose();
  });

  test("emits experiments-db for experiments.db changes", () => {
    const w = createStateWatcher("/proj");
    const events: string[] = [];
    w.onDidChange((e) => events.push(e.kind));
    const expW = watchers.find((wr) => wr.pattern.pattern === ".sim-flow/experiments.db")!;
    expW.changeCbs[0]({ fsPath: "/proj/.sim-flow/experiments.db" });
    expect(events).toEqual(["experiments-db"]);
    w.dispose();
  });

  test("emits plan for impl-plan / test-plan / perf-plan / plan-management changes", () => {
    const w = createStateWatcher("/proj");
    const events: string[] = [];
    w.onDidChange((e) => events.push(e.kind));
    const implW = watchers.find((wr) => wr.pattern.pattern === "docs/impl-plan/*.md")!;
    const testW = watchers.find((wr) => wr.pattern.pattern === "docs/test-plan/*.md")!;
    const perfW = watchers.find((wr) => wr.pattern.pattern === "docs/perf-plan/*.md")!;
    const mgmtW = watchers.find((wr) => wr.pattern.pattern === "docs/plan-management.md")!;
    implW.changeCbs[0]({ fsPath: "/proj/docs/impl-plan/milestone-01-foo.md" });
    testW.changeCbs[0]({ fsPath: "/proj/docs/test-plan/tb-milestone-01.md" });
    perfW.changeCbs[0]({ fsPath: "/proj/docs/perf-plan/perf-plan.md" });
    mgmtW.changeCbs[0]({ fsPath: "/proj/docs/plan-management.md" });
    expect(events).toEqual(["plan", "plan", "plan", "plan"]);
    w.dispose();
  });

  test("delete events fire the same emit as create/change", () => {
    const w = createStateWatcher("/proj");
    const events: string[] = [];
    w.onDidChange((e) => events.push(e.kind));
    const sockW = watchers.find((wr) => wr.pattern.pattern === ".sim-flow/control.sock")!;
    sockW.deleteCbs[0]({ fsPath: "/proj/.sim-flow/control.sock" });
    // control.sock is mapped to "state-toml" (intentional coarsening
    // documented in the source).
    expect(events).toEqual(["state-toml"]);
    w.dispose();
  });

  test("dispose() tears down every underlying watcher", () => {
    const w = createStateWatcher("/proj");
    expect(watchers.every((wr) => wr.disposed === false)).toBe(true);
    w.dispose();
    expect(watchers.every((wr) => wr.disposed === true)).toBe(true);
  });
});
