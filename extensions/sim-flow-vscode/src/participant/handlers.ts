// Per-command handler implementations for the @sim-flow chat
// participant. Each handler receives an already-resolved
// ProjectContext and the VS Code chat primitives. Phase 9 M5 trimmed
// this module to the simple "shell out to sim-flow CLI" handlers;
// the orchestration-heavy `/step` path lives in `session/pump.ts` +
// `session/registry.ts` now.

import * as vscode from "vscode";

import { SimFlowCliError } from "../cli";
import type { ProjectContext } from "../context";
import type { SecretStorage } from "../llm";
import { parseGateArgs, parseResetArgs, parseRunsArgs } from "./args";
import { formatGateMarkdown, formatRunsMarkdown, formatStatusMarkdown } from "./format";

export interface HandlerArgs {
  context: ProjectContext;
  request: vscode.ChatRequest;
  /**
   * Prompt with `--project <path>` already stripped by the
   * dispatcher. Handlers parse this instead of `request.prompt`.
   */
  prompt: string;
  stream: vscode.ChatResponseStream;
  token: vscode.CancellationToken;
  /** Available for handlers that may dispatch to a backend; M5 keeps it for forward-compat. */
  secrets?: SecretStorage;
  chatHistory?: readonly (vscode.ChatRequestTurn | vscode.ChatResponseTurn)[];
}

export async function handleStatus({ context, stream }: HandlerArgs): Promise<void> {
  const status = await context.cli.status();
  stream.markdown(formatStatusMarkdown(status) + "\n");
}

export async function handleRuns({ context, prompt, stream }: HandlerArgs): Promise<void> {
  const filter = parseRunsArgs(prompt);
  const rows = await context.cli.runs(filter);
  stream.markdown(formatRunsMarkdown(rows) + "\n");
}

export async function handleGate({ context, prompt, stream }: HandlerArgs): Promise<void> {
  const { step, candidate } = parseGateArgs(prompt);
  const result = await context.cli.gate(step, candidate);
  stream.markdown(formatGateMarkdown(result) + "\n");
}

export async function handleAdvance({ context, prompt, stream }: HandlerArgs): Promise<void> {
  const tokens = prompt.split(/\s+/).filter((t) => t.length > 0);
  const step = tokens[0]?.length ? tokens[0] : undefined;
  try {
    const result = await context.cli.advance(step);
    if (result.clean && result.advanced) {
      stream.markdown(
        `**\`${result.step}\` advanced.** Current step is now \`${result.next_step}\`.\n`,
      );
    } else if (result.clean) {
      stream.markdown(`**\`${result.step}\` passed.** No further steps in this flow.\n`);
    } else {
      const lines = result.failures.map((f) => `- ${f.description}: ${f.reason}`).join("\n");
      stream.markdown(
        `**\`${result.step}\` cannot advance — gate has ${result.failures.length} failure(s).**\n\n${lines}\n`,
      );
    }
  } catch (err) {
    stream.markdown(errorMarkdown(err));
  }
}

export async function handleReset({ context, prompt, stream }: HandlerArgs): Promise<void> {
  const parsed = parseResetArgs(prompt);
  if ("error" in parsed) {
    stream.markdown(`**Error:** ${parsed.error}\n`);
    return;
  }
  const args = context.cli.buildArgs(["reset", parsed.step]);
  stream.markdown(
    `Resetting \`${parsed.step}\` and cascading every downstream gate to "not passed".\n`,
  );
  try {
    await runQuiet(context.cli.binary, args);
  } catch (err) {
    stream.markdown(errorMarkdown(err));
    return;
  }
  stream.markdown(
    [
      "",
      `**Reset complete.** \`${parsed.step}\` is now current again.`,
      "",
      "Existing chat tabs for this step stay visible; scroll back to review prior work. To continue, start a new session with `/step " +
        parsed.step +
        ".work`.",
    ].join("\n") + "\n",
  );
}

export async function handleInit({ context, stream }: HandlerArgs): Promise<void> {
  stream.markdown(
    [
      `Running \`sim-flow init\` in \`${context.projectDir}\`.`,
      "",
      "*If the project already has `.sim-flow/state.toml`, the CLI will refuse and leave existing state alone.*",
    ].join("\n") + "\n",
  );
  const args = context.cli.buildArgs(["init", "--flow", "direct-modeling"]);
  try {
    await runQuiet(context.cli.binary, args);
    stream.markdown(`Initialized. Run \`/status\` to see the starting state.\n`);
  } catch (err) {
    stream.markdown(errorMarkdown(err));
  }
}

// --------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------

function errorMarkdown(err: unknown): string {
  if (err instanceof SimFlowCliError) {
    const detail = err.stderr?.trim() || err.message;
    return `**Error** (${err.kind}): ${detail}\n`;
  }
  return `**Error:** ${String(err)}\n`;
}

async function runQuiet(bin: string, args: string[]): Promise<void> {
  const { execFile } = await import("node:child_process");
  const { promisify } = await import("node:util");
  const run = promisify(execFile);
  await run(bin, args, { maxBuffer: 4 * 1024 * 1024 });
}
