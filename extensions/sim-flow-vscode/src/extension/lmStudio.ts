/**
 * Standalone LM-API probe commands. Used by:
 *   - `sim-flow.testLmModel` (single-model end-to-end diagnostic
 *     comparing stream() vs text() with / without `modelOptions`,
 *     including a Copilot control run).
 *   - `sim-flow.dumpAvailableLmModels` (enumerate every provider
 *     registered via `vscode.lm.registerChatModelProvider`).
 *
 * No state shared with the rest of `extension.ts`; lives here so the
 * top-level activate file stays under the refactor threshold.
 */

import * as vscode from "vscode";

export async function testLmModel(_context: vscode.ExtensionContext): Promise<void> {
  void _context;
  const channel = vscode.window.createOutputChannel("sim-flow: LM test");
  channel.show(true);
  const log = (line: string): void => {
    channel.appendLine(line);
  };
  log(`# LM model probe ${new Date().toISOString()}`);
  log("");

  const config = vscode.workspace.getConfiguration("sim-flow");
  const modelHint = config.get<string>("llm.model")?.trim() ?? "";
  log(`sim-flow.llm.model: \`${modelHint || "(empty -> any)"}\``);

  // Same vendor/family parsing the real backend uses.
  const idx = modelHint.indexOf("/");
  const vendor = idx >= 0 ? modelHint.slice(0, idx).trim() : undefined;
  const family = idx >= 0 ? modelHint.slice(idx + 1).trim() : modelHint || undefined;
  const selector: vscode.LanguageModelChatSelector = {};
  if (vendor) {
    selector.vendor = vendor;
  }
  if (family) {
    selector.family = family;
  }

  let models: vscode.LanguageModelChat[];
  try {
    models = await vscode.lm.selectChatModels(selector);
  } catch (err) {
    log(`selectChatModels threw: ${(err as Error).message ?? String(err)}`);
    return;
  }
  log(`selectChatModels returned ${models.length} model(s).`);
  if (models.length === 0) {
    log(
      "Empty result. Try clearing `sim-flow.llm.model` or running `List Available Language Models`.",
    );
    return;
  }
  const model = models[0];
  log(
    `Using: id=${model.id} vendor=${model.vendor} family=${model.family} maxInputTokens=${model.maxInputTokens}`,
  );
  log("");

  type ProbeResult = { ok: boolean; partCount: number; durationMs: number; sample?: string };
  const probes: Array<{ label: string; run: () => Promise<ProbeResult> }> = [
    {
      label: "A. stream(), no modelOptions",
      run: () => probeStream(model, log, undefined),
    },
    {
      label: "B. text(), no modelOptions",
      run: () => probeText(model, log, undefined),
    },
    {
      label: "C. stream(), modelOptions={max_tokens: 4096}",
      run: () => probeStream(model, log, { max_tokens: 4096 }),
    },
  ];

  // Compare against a known-working copilot model so we can tell
  // whether an empty stream is provider-specific or our plumbing.
  let copilotModel: vscode.LanguageModelChat | undefined;
  try {
    const copilotMatches = await vscode.lm.selectChatModels({ vendor: "copilot" });
    copilotModel = copilotMatches.find((m) => m.family === "gpt-4o") ?? copilotMatches[0];
  } catch {
    // ignore; copilot may not be installed
  }
  if (copilotModel) {
    probes.push({
      label: `D. stream() against ${copilotModel.vendor}/${copilotModel.family} (control)`,
      run: () => probeStream(copilotModel as vscode.LanguageModelChat, log, undefined),
    });
  } else {
    log("(no copilot model found for control probe)");
    log("");
  }

  for (const probe of probes) {
    log(`---- ${probe.label} ----`);
    let result: ProbeResult;
    try {
      result = await probe.run();
    } catch (err) {
      log(`  threw: ${(err as Error).message ?? String(err)}`);
      log("");
      continue;
    }
    log(
      `  ${result.ok ? "OK" : "EMPTY"} -- ${result.partCount} part(s) in ${result.durationMs}ms${result.sample ? ` :: ${result.sample}` : ""}`,
    );
    log("");
  }
  log("Interpretation:");
  log(
    "  * If A and B both empty but D non-empty -> Claude Code's provider rejects sim-flow callers.",
  );
  log(
    "  * If C non-empty but A empty -> provider requires modelOptions.max_tokens; we'll plumb it.",
  );
  log("  * If A and D both empty -> something in our code path is wrong (rebuild and retry).");
  log(
    "  * If everything non-empty -> the failure was specific to a long step prompt (e.g. DM2d); collapse system messages.",
  );
}

async function probeStream(
  model: vscode.LanguageModelChat,
  log: (line: string) => void,
  modelOptions: Record<string, unknown> | undefined,
): Promise<{ ok: boolean; partCount: number; durationMs: number; sample?: string }> {
  const tokenSource = new vscode.CancellationTokenSource();
  const opts: vscode.LanguageModelChatRequestOptions = {
    justification: "sim-flow: LM model probe",
  };
  if (modelOptions) {
    opts.modelOptions = modelOptions;
  }
  const startedAt = Date.now();
  const request = await model.sendRequest(
    [vscode.LanguageModelChatMessage.User("respond with the single word: hi")],
    opts,
    tokenSource.token,
  );
  const streamLike = (request as unknown as { stream?: AsyncIterable<unknown> }).stream;
  let partCount = 0;
  let sample: string | undefined;
  if (!streamLike) {
    log("  (request.stream not present)");
    return { ok: false, partCount: 0, durationMs: Date.now() - startedAt };
  }
  for await (const part of streamLike) {
    partCount++;
    if (partCount <= 2) {
      const ctor = (part as { constructor?: { name?: string } })?.constructor?.name ?? typeof part;
      const keys =
        part && typeof part === "object" ? Object.keys(part as object).join(", ") : "(scalar)";
      let preview = "";
      try {
        preview = JSON.stringify(part)?.slice(0, 96) ?? "";
      } catch {
        preview = "(unserializable)";
      }
      log(`  [${partCount}] ctor=${ctor} keys={${keys}} preview=${preview}`);
    }
    if (partCount === 1 && part && typeof part === "object" && "value" in part) {
      sample = String((part as { value: unknown }).value).slice(0, 32);
    }
  }
  return { ok: partCount > 0, partCount, durationMs: Date.now() - startedAt, sample };
}

async function probeText(
  model: vscode.LanguageModelChat,
  log: (line: string) => void,
  modelOptions: Record<string, unknown> | undefined,
): Promise<{ ok: boolean; partCount: number; durationMs: number; sample?: string }> {
  const tokenSource = new vscode.CancellationTokenSource();
  const opts: vscode.LanguageModelChatRequestOptions = {
    justification: "sim-flow: LM model probe",
  };
  if (modelOptions) {
    opts.modelOptions = modelOptions;
  }
  const startedAt = Date.now();
  const request = await model.sendRequest(
    [vscode.LanguageModelChatMessage.User("respond with the single word: hi")],
    opts,
    tokenSource.token,
  );
  let partCount = 0;
  let sample: string | undefined;
  for await (const fragment of request.text) {
    partCount++;
    if (partCount <= 2) {
      log(`  [${partCount}] text=${JSON.stringify(fragment).slice(0, 96)}`);
    }
    if (partCount === 1) {
      sample = String(fragment).slice(0, 32);
    }
  }
  return { ok: partCount > 0, partCount, durationMs: Date.now() - startedAt, sample };
}

/**
 * Dump every chat model the VS Code Language Model API exposes to a
 * dedicated output channel. Useful for verifying whether an
 * external extension (e.g. Anthropic's Claude Code extension) has
 * registered itself as a chat-model provider via
 * `vscode.lm.registerChatModelProvider`. If it has, the model shows
 * up here and sim-flow's existing `vscode` backend can use it
 * without any code change. If it doesn't, only Copilot models
 * (and possibly nothing) appear.
 */
export async function dumpAvailableLmModels(): Promise<void> {
  const channel = vscode.window.createOutputChannel("sim-flow: LM models");
  channel.show(true);
  channel.appendLine(`# Available chat models (queried at ${new Date().toISOString()})`);
  channel.appendLine("");
  let models: vscode.LanguageModelChat[];
  try {
    models = await vscode.lm.selectChatModels({});
  } catch (err) {
    channel.appendLine(`ERROR: vscode.lm.selectChatModels({}) threw: ${String(err)}`);
    return;
  }
  if (models.length === 0) {
    channel.appendLine(
      "No chat models were returned. Install Copilot (or the Claude Code extension, " +
        "or any other extension that registers a chat-model provider via " +
        "`vscode.lm.registerChatModelProvider`) and re-run this command.",
    );
    return;
  }
  channel.appendLine(`Found ${models.length} model(s):\n`);
  for (const m of models) {
    channel.appendLine(`- id:               ${m.id}`);
    channel.appendLine(`  vendor:           ${m.vendor}`);
    channel.appendLine(`  family:           ${m.family}`);
    channel.appendLine(`  name:             ${m.name}`);
    channel.appendLine(`  version:          ${m.version}`);
    channel.appendLine(`  maxInputTokens:   ${m.maxInputTokens}`);
    channel.appendLine("");
  }
  channel.appendLine(
    "Tip: a `vendor` of `anthropic` (or `claude-code`) here means the Claude Code " +
      "extension is registered as a provider, and sim-flow's `vscode` source will " +
      "pick it up automatically. Constrain via `sim-flow.llm.model` (matches the " +
      "`family` field) if multiple models are available.",
  );
}
