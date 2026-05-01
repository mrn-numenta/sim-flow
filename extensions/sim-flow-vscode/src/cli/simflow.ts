// Typed wrapper around the sim-flow CLI. Every method is a thin
// shell-out: build argv, exec, parse JSON, return a typed result.
// Non-JSON / long-running commands are exposed via `buildArgs()` and
// `buildCommandLine()` so callers can feed them to a VS Code terminal
// or a streaming `child_process.spawn`.

import { type Execute, defaultExecute } from "./executor";
import { SimFlowCliError } from "./errors";
import type {
  AdvanceResult,
  BaselineDelta,
  BaselineRecord,
  DescribeResult,
  GateResult,
  NewModelOptions,
  NewModelResult,
  PromptListEntry,
  RunFilter,
  RunRow,
  StatusResult,
} from "./types";

export interface SimFlowCliOptions {
  /** Absolute path to the resolved sim-flow binary. */
  binary: string;
  /** Absolute path to the sim-flow project (contains `.sim-flow/`). */
  projectDir: string;
  /** Optional `--foundation-root` override; empty string treated as unset. */
  foundationRoot?: string;
}

export class SimFlowCli {
  readonly binary: string;
  readonly projectDir: string;
  readonly foundationRoot?: string;

  constructor(
    options: SimFlowCliOptions,
    private readonly execute: Execute = defaultExecute,
  ) {
    this.binary = options.binary;
    this.projectDir = options.projectDir;
    const root = options.foundationRoot?.trim();
    this.foundationRoot = root && root.length > 0 ? root : undefined;
  }

  // ---------------------------------------------------------------------
  // JSON-producing subcommands (see docs/architecture/ai-flow/cli-json.md)
  // ---------------------------------------------------------------------

  async status(): Promise<StatusResult> {
    return this.execJson<StatusResult>(["status", "--json"]);
  }

  async runs(filter: RunFilter = {}): Promise<RunRow[]> {
    const args: string[] = ["runs", "--json"];
    if (filter.workload) {
      args.push("--workload", filter.workload);
    }
    if (filter.candidate) {
      args.push("--candidate", filter.candidate);
    }
    if (filter.study) {
      args.push("--study", filter.study);
    }
    if (filter.sweep) {
      args.push("--sweep", filter.sweep);
    }
    if (typeof filter.limit === "number") {
      args.push("--limit", String(filter.limit));
    }
    return this.execJson<RunRow[]>(args);
  }

  /**
   * Run the structural gate for `step` (defaults to current step) and
   * return the parsed JSON payload. The CLI emits JSON on stdout even
   * when the gate fails, so this method resolves with the failure
   * report instead of throwing. Callers should inspect `result.clean`.
   */
  async gate(step?: string, candidate?: string): Promise<GateResult> {
    const args: string[] = ["gate"];
    if (step) {
      args.push(step);
    }
    if (candidate) {
      args.push("--candidate", candidate);
    }
    args.push("--json");
    return this.execJsonTolerateGateFailure<GateResult>(args);
  }

  async baselineList(): Promise<BaselineRecord[]> {
    return this.execJson<BaselineRecord[]>(["baseline", "list", "--json"]);
  }

  async baselineCreate(name: string, run?: string, notes?: string): Promise<BaselineRecord> {
    const args: string[] = ["baseline", "create", name];
    if (run) {
      args.push("--run", run);
    }
    if (notes) {
      args.push("--notes", notes);
    }
    args.push("--json");
    return this.execJson<BaselineRecord>(args);
  }

  async baselineCompare(name: string, current?: string): Promise<BaselineDelta> {
    const args: string[] = ["baseline", "compare", name];
    if (current) {
      args.push("--current", current);
    }
    args.push("--json");
    return this.execJson<BaselineDelta>(args);
  }

  /**
   * Phase 9 M1 primitive: ask sim-flow to validate the gate for `step`
   * and, if clean, mark it passed and bump `current_step` to the next
   * step. Returns the structured result either way; the caller
   * inspects `clean` and `advanced` to decide what to render.
   */
  async advance(step?: string): Promise<AdvanceResult> {
    const args: string[] = ["advance"];
    if (step) {
      args.push(step);
    }
    args.push("--json");
    return this.execJsonTolerateGateFailure<AdvanceResult>(args);
  }

  /**
   * Phase 9 M1 primitive: fetch the canonical step descriptor (work
   * artifacts, predecessor inputs, instruction body, gate checks)
   * from sim-flow. Replaces the per-step tables that used to live in
   * the extension.
   */
  /**
   * Run `sim-flow block-diagram` for the project. Produces / refreshes
   * `<project>/.sim-flow/block-diagram.svg`. Slow: shells out to
   * `cargo run -- --dump-netlist-json` first, so the user-visible
   * action button must show a pending state.
   */
  async blockDiagram(): Promise<void> {
    const args = this.buildArgs(["block-diagram"]);
    try {
      await this.execute(this.binary, args);
    } catch (err) {
      throw toCliError(this.binary, args, err, "non-zero-exit");
    }
  }

  async describe(step: string, kind: "work" | "critique"): Promise<DescribeResult> {
    return this.execJson<DescribeResult>(["describe", `${step}.${kind}`, "--json"]);
  }

  // ---------------------------------------------------------------------
  // Prompt-override management (Phase C).
  // ---------------------------------------------------------------------

  async promptsList(): Promise<PromptListEntry[]> {
    return this.execJson<PromptListEntry[]>(["prompts", "list", "--json"]);
  }

  async promptShow(slug: string, kind: "work" | "critique"): Promise<string> {
    const args = this.buildArgs(["prompts", "show", `${slug}.${kind}`]);
    try {
      const result = await this.execute(this.binary, args);
      return result.stdout;
    } catch (err) {
      throw toCliError(this.binary, args, err, "non-zero-exit");
    }
  }

  /**
   * Persist a prompt override at the given scope. We bypass the
   * injectable `Execute` here because the CLI reads the new content
   * from stdin and the standard executor doesn't support stdin
   * piping. Tests that need to mock this should swap the whole
   * `SimFlowCli` instance.
   */
  async promptSave(
    slug: string,
    kind: "work" | "critique",
    scope: "project" | "global",
    content: string,
  ): Promise<void> {
    const args = this.buildArgs(["prompts", "save", `${slug}.${kind}`, "--scope", scope]);
    const { spawn } = await import("node:child_process");
    await new Promise<void>((resolve, reject) => {
      const child = spawn(this.binary, args, { stdio: ["pipe", "pipe", "pipe"] });
      let stderr = "";
      child.stderr.on("data", (b: Buffer) => {
        stderr += b.toString("utf8");
      });
      child.on("error", reject);
      child.on("exit", (code) => {
        if (code === 0) {
          resolve();
        } else {
          reject(
            new SimFlowCliError(`prompts save failed (exit ${code}): ${stderr.trim()}`, {
              kind: "non-zero-exit",
              exitCode: code,
              stderr,
              command: this.binary,
              args,
            }),
          );
        }
      });
      child.stdin.end(content);
    });
  }

  async promptReset(
    slug: string,
    kind: "work" | "critique",
    scope: "project" | "global" | "all",
  ): Promise<void> {
    const args = this.buildArgs(["prompts", "reset", `${slug}.${kind}`, "--scope", scope]);
    try {
      await this.execute(this.binary, args);
    } catch (err) {
      throw toCliError(this.binary, args, err, "non-zero-exit");
    }
  }

  async newModel(options: NewModelOptions): Promise<NewModelResult> {
    const args: string[] = ["new", "model", options.name];
    if (options.destination) {
      args.push("--destination", options.destination);
    }
    if (options.libraryPath) {
      args.push("--library-path", options.libraryPath);
    }
    if (options.skipCargoCheck) {
      args.push("--skip-cargo-check");
    }
    args.push("--json");
    return this.execJson<NewModelResult>(args);
  }

  // ---------------------------------------------------------------------
  // Raw argv construction for commands that stream or are interactive.
  // ---------------------------------------------------------------------

  /**
   * Build the full argv array for a subcommand, including the global
   * `--project` and `--foundation-root` flags. The first element is the
   * binary; feed the remainder to `child_process.spawn` or a VS Code
   * terminal.
   */
  buildArgs(subcommand: string[]): string[] {
    return [...this.globalArgs(), ...subcommand];
  }

  /**
   * Build a shell-ready command string for a terminal that already knows
   * how to quote. VS Code's `Terminal.sendText` takes a single string;
   * this is the quote-safe form for it.
   */
  buildCommandLine(subcommand: string[]): string {
    const pieces = [this.binary, ...this.globalArgs(), ...subcommand];
    return pieces.map(shellQuote).join(" ");
  }

  // ---------------------------------------------------------------------
  // Internals
  // ---------------------------------------------------------------------

  private globalArgs(): string[] {
    const args: string[] = [];
    if (this.foundationRoot) {
      args.push("--foundation-root", this.foundationRoot);
    }
    args.push("--project", this.projectDir);
    return args;
  }

  private async execJson<T>(subcommand: string[]): Promise<T> {
    const args = this.buildArgs(subcommand);
    let result;
    try {
      result = await this.execute(this.binary, args);
    } catch (err) {
      throw toCliError(this.binary, args, err, "spawn-failed");
    }
    return parseJson<T>(this.binary, args, result.stdout);
  }

  /**
   * `gate --json` emits valid JSON on stdout even when the gate fails.
   * execFile reports non-zero exit as a thrown error; we still want the
   * payload so the UI can render failure details.
   */
  private async execJsonTolerateGateFailure<T>(subcommand: string[]): Promise<T> {
    const args = this.buildArgs(subcommand);
    try {
      const result = await this.execute(this.binary, args);
      return parseJson<T>(this.binary, args, result.stdout);
    } catch (err) {
      const stdout = extractStdout(err);
      if (stdout) {
        return parseJson<T>(this.binary, args, stdout);
      }
      throw toCliError(this.binary, args, err, "non-zero-exit");
    }
  }
}

function parseJson<T>(command: string, args: string[], stdout: string): T {
  const trimmed = stdout.trim();
  if (trimmed.length === 0) {
    throw new SimFlowCliError("sim-flow returned empty stdout for a JSON subcommand", {
      kind: "unexpected-stdout",
      command,
      args,
      stdout,
    });
  }
  try {
    return JSON.parse(trimmed) as T;
  } catch (cause) {
    throw new SimFlowCliError(`Failed to parse sim-flow JSON output: ${(cause as Error).message}`, {
      kind: "json-parse-failed",
      command,
      args,
      stdout,
      cause,
    });
  }
}

function shellQuote(value: string): string {
  if (value.length === 0) {
    return "''";
  }
  if (/^[A-Za-z0-9_./:@%^+=-]+$/.test(value)) {
    return value;
  }
  return `'${value.replace(/'/g, "'\\''")}'`;
}

interface ErrorLike {
  code?: number | string | null;
  stdout?: string;
  stderr?: string;
  message?: string;
}

function extractStdout(err: unknown): string | null {
  if (err && typeof err === "object" && "stdout" in err) {
    const v = (err as ErrorLike).stdout;
    return typeof v === "string" && v.trim().length > 0 ? v : null;
  }
  return null;
}

function toCliError(
  command: string,
  args: string[],
  err: unknown,
  fallbackKind: "spawn-failed" | "non-zero-exit",
): SimFlowCliError {
  if (err && typeof err === "object") {
    const e = err as ErrorLike;
    const rawCode = e.code;
    const exitCode = typeof rawCode === "number" ? rawCode : null;
    const message = e.message ?? `sim-flow ${args.join(" ")} failed`;
    return new SimFlowCliError(message, {
      kind: fallbackKind,
      exitCode,
      stdout: e.stdout,
      stderr: e.stderr,
      command,
      args,
      cause: err,
    });
  }
  return new SimFlowCliError(`sim-flow ${args.join(" ")} failed`, {
    kind: fallbackKind,
    command,
    args,
    cause: err,
  });
}
