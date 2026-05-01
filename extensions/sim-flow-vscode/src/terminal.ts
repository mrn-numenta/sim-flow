// Reusable named VS Code terminal for long-running `sim-flow` CLI
// invocations (runs, resets, sweeps, baseline create, etc.). The
// dashboard's in-process CLI wrapper handles the short JSON-returning
// subcommands; anything that streams output or takes more than a
// second belongs here so users can watch it live.
//
// The terminal is reused across invocations. If the user manually
// closes it, the next `run()` recreates it. The state watcher picks
// up any `.sim-flow/` writes the command makes, so callers do not
// need to explicitly refresh the dashboard afterward.

import * as vscode from "vscode";

export interface SimFlowTerminalOptions {
  /** Absolute project directory. Used as the terminal's cwd. */
  projectDir: string;
  /** Label shown in the VS Code terminal dropdown. */
  name?: string;
  /** Extra environment variables for the shell session. */
  env?: Record<string, string>;
}

export class SimFlowTerminal {
  readonly projectDir: string;
  private readonly name: string;
  private readonly env: Record<string, string> | undefined;
  private terminal: vscode.Terminal | undefined;
  private readonly closeSubscription: vscode.Disposable;

  constructor(options: SimFlowTerminalOptions) {
    this.projectDir = options.projectDir;
    this.name = options.name ?? "sim-flow";
    this.env = options.env;
    this.closeSubscription = vscode.window.onDidCloseTerminal((t) => {
      if (t === this.terminal) {
        this.terminal = undefined;
      }
    });
  }

  /**
   * Send a fully-formed shell command to the terminal, creating the
   * terminal on first use or if the user closed the previous one.
   * Brings the terminal forward AND moves focus to it -- callers of
   * this method are dashboard buttons that fire the agent or run a
   * sim-flow CLI; the user just clicked something they want to watch
   * or interact with, so the terminal should win the foreground.
   * `preserveFocus=true` left the terminal hidden when another panel
   * (Problems / Output / etc.) was active in the same panel area.
   * Does not wait for the command to finish.
   */
  run(commandLine: string): void {
    const term = this.ensure();
    term.show(false);
    term.sendText(commandLine, true);
  }

  dispose(): void {
    this.closeSubscription.dispose();
    this.terminal?.dispose();
    this.terminal = undefined;
  }

  private ensure(): vscode.Terminal {
    const existing = this.terminal;
    if (existing && existing.exitStatus === undefined) {
      return existing;
    }
    this.terminal = vscode.window.createTerminal({
      name: this.name,
      cwd: this.projectDir,
      env: this.env,
    });
    return this.terminal;
  }
}
