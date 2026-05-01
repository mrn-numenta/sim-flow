import { describe, expect, it, vi, beforeEach } from "vitest";

interface FakeTerminal {
  name: string;
  cwd: string;
  env: Record<string, string> | undefined;
  shown: boolean;
  preserveFocus: boolean | undefined;
  sent: Array<{ text: string; newline: boolean }>;
  disposed: boolean;
  exitStatus: { code: number } | undefined;
  show(preserveFocus?: boolean): void;
  sendText(text: string, addNewLine?: boolean): void;
  dispose(): void;
}

const created: FakeTerminal[] = [];
let closeHandlers: Array<(t: FakeTerminal) => void> = [];

function makeFakeTerminal(
  name: string,
  cwd: string,
  env: Record<string, string> | undefined,
): FakeTerminal {
  const t: FakeTerminal = {
    name,
    cwd,
    env,
    shown: false,
    preserveFocus: undefined,
    sent: [],
    disposed: false,
    exitStatus: undefined,
    show(preserveFocus?: boolean) {
      this.shown = true;
      this.preserveFocus = preserveFocus;
    },
    sendText(text, addNewLine) {
      this.sent.push({ text, newline: addNewLine ?? false });
    },
    dispose() {
      this.disposed = true;
      this.exitStatus = { code: 0 };
      for (const h of closeHandlers) {
        h(this);
      }
    },
  };
  return t;
}

vi.mock("vscode", () => ({
  window: {
    createTerminal: (opts: { name: string; cwd: string; env?: Record<string, string> }) => {
      const t = makeFakeTerminal(opts.name, opts.cwd, opts.env);
      created.push(t);
      return t;
    },
    onDidCloseTerminal: (handler: (t: FakeTerminal) => void) => {
      closeHandlers.push(handler);
      return {
        dispose: () => {
          closeHandlers = closeHandlers.filter((h) => h !== handler);
        },
      };
    },
  },
}));

const { SimFlowTerminal } = await import("./terminal");

beforeEach(() => {
  created.length = 0;
  closeHandlers = [];
});

describe("SimFlowTerminal", () => {
  it("creates a named terminal in the project dir on first run() and reuses it", () => {
    const term = new SimFlowTerminal({ projectDir: "/tmp/project" });
    term.run("sim-flow run DM0");
    term.run("sim-flow reset DM0");
    expect(created.length).toBe(1);
    expect(created[0].name).toBe("sim-flow");
    expect(created[0].cwd).toBe("/tmp/project");
    expect(created[0].sent.map((s) => s.text)).toEqual(["sim-flow run DM0", "sim-flow reset DM0"]);
  });

  it("reveals the terminal and takes focus so the user sees the agent", () => {
    const term = new SimFlowTerminal({ projectDir: "/tmp/p" });
    term.run("sim-flow status");
    expect(created[0].shown).toBe(true);
    expect(created[0].preserveFocus).toBe(false);
  });

  it("respects a custom terminal name", () => {
    const term = new SimFlowTerminal({ projectDir: "/p", name: "sim-flow: steps" });
    term.run("sim-flow status");
    expect(created[0].name).toBe("sim-flow: steps");
  });

  it("passes through terminal environment overrides", () => {
    const term = new SimFlowTerminal({
      projectDir: "/tmp/project",
      env: { SIM_FLOW_FRAMEWORK_DOCS_ROOT: "/ext/foundation-docs/rustdoc" },
    });
    term.run("sim-flow status");
    expect(created[0].env).toEqual({
      SIM_FLOW_FRAMEWORK_DOCS_ROOT: "/ext/foundation-docs/rustdoc",
    });
  });

  it("recreates the terminal after the user closes it", () => {
    const term = new SimFlowTerminal({ projectDir: "/tmp/project" });
    term.run("first");
    const firstTerm = created[0];
    firstTerm.dispose();
    term.run("second");
    expect(created.length).toBe(2);
    expect(created[1]).not.toBe(firstTerm);
    expect(created[1].sent.map((s) => s.text)).toEqual(["second"]);
  });

  it("dispose() tears down the terminal and the close subscription", () => {
    const term = new SimFlowTerminal({ projectDir: "/tmp/project" });
    term.run("x");
    expect(closeHandlers.length).toBe(1);
    term.dispose();
    expect(created[0].disposed).toBe(true);
    expect(closeHandlers.length).toBe(0);
  });
});
