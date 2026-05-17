// Injectable process executor. Production uses `execFile` via promisify;
// tests swap in a deterministic stub so the full pipeline can be
// exercised without spawning child processes.

import { execFile as nodeExecFile } from "node:child_process";
import { promisify } from "node:util";

import { augmentPathForCargo } from "../childEnv";

export interface ExecResult {
  stdout: string;
  stderr: string;
}

/**
 * Run `bin` with the supplied argv and return captured stdout / stderr.
 *
 * Throws if the child exits non-zero; the caller inspects the error's
 * `.code`, `.stderr`, and `.stdout` to decide how to surface it.
 */
export type Execute = (bin: string, args: string[]) => Promise<ExecResult>;

const execFileAsync = promisify(nodeExecFile);

export const defaultExecute: Execute = async (bin, args) => {
  const { stdout, stderr } = await execFileAsync(bin, args, {
    // Generous buffer so long `runs --json` outputs do not truncate.
    maxBuffer: 16 * 1024 * 1024,
    windowsHide: true,
    // sim-flow's `new model` / `gate` / `record-run` paths shell out
    // to cargo. VS Code launched from Finder/Dock inherits a GUI
    // environment whose PATH lacks ~/.cargo/bin, so the child's
    // `Command::new("cargo")` fails with ENOENT. Augmenting PATH here
    // makes one-shot CLI invocations succeed the same way the
    // orchestrator pump's spawn does (session/socketPump.ts).
    env: { ...process.env, PATH: augmentPathForCargo(process.env.PATH) },
  });
  return { stdout, stderr };
};
