// @sim-flow chat participant. Phase 9 M5: the `/step` path is now a
// thin pump over `sim-flow session ... --jsonl` (see
// `session/pump.ts`); other slash commands shell out to the CLI as
// before.

import * as vscode from "vscode";

import { findProjectCandidates, resolveContext } from "../context";
import type { SimFlowCliError } from "../cli";
import { type LlmSource, type SecretStorage } from "../llm";
import {
  handleAdvance,
  handleGate,
  handleInit,
  handleReset,
  handleRuns,
  handleStatus,
} from "./handlers";
import { extractProjectHint, extractSpecPath, parseStepRef } from "./args";
import { flowOrderFor, suggestFollowups } from "./followups";
import { readFlowState } from "../state/flowState";
import { PumpLlmConfig, rendererFromChatStream, SessionPump } from "../session/pump";
import { freshSessionKey, type SessionKey, SessionRegistry } from "../session/registry";

interface SessionMetadata {
  tag: "sim-flow";
  pumpKey: SessionKey;
  step: string;
  kind: "work" | "critique";
  projectDir?: string;
}

export function registerChatParticipant(extensionContext: vscode.ExtensionContext): void {
  const registry = new SessionRegistry();
  extensionContext.subscriptions.push({ dispose: () => registry.disposeAll() });

  const handler: vscode.ChatRequestHandler = async (request, chatContext, stream, token) => {
    const { hint, stripped } = extractProjectHint(request.prompt);
    const inheritedProjectDir = hint ? undefined : inheritProjectDir(chatContext.history);
    const ctx = await resolveContext({
      projectDir: hint ?? inheritedProjectDir,
      showErrors: false,
    });
    if (!ctx) {
      await renderResolutionError(stream, hint);
      return {};
    }

    // /step launches a fresh SessionPump.
    if (request.command === "step") {
      return runStepCommand(registry, ctx, stripped, stream, extensionContext.secrets, token);
    }

    // /auto launches a long-lived SessionPump driving the entire
    // remaining flow unattended.
    if (request.command === "auto") {
      return runAutoCommand(registry, ctx, stripped, stream, extensionContext.secrets, token);
    }

    // Free-form follow-up: route to the active pump for this chat,
    // if any. If chat history names a prior session but the pump is
    // gone (e.g. after an extension reload — SessionRegistry is
    // in-memory and resets while chat history persists), tell the
    // user the session ended instead of silently rendering the help
    // card. The help card only appears when there's no prior session
    // tagged in this chat at all.
    if (!request.command) {
      const inherited = latestSessionMetadata(chatContext.history);
      if (inherited) {
        if (registry.has(inherited.pumpKey)) {
          return runStepFollowUp(registry, inherited, stripped, stream, token);
        }
        stream.markdown(
          [
            `_The previous \`${inherited.step}.${inherited.kind}\` session is no longer running` +
              " (the orchestrator subprocess was likely terminated by an extension reload)._",
            "",
            `Start a fresh session with \`/step ${inherited.step}.${inherited.kind}\` to continue.`,
            "",
          ].join("\n"),
        );
        return {};
      }
      renderHelp(stream);
      return {};
    }

    // Other slash commands: simple CLI shell-outs.
    const args = {
      context: ctx,
      request,
      prompt: stripped,
      stream,
      token,
      secrets: extensionContext.secrets,
      chatHistory: chatContext.history,
    };
    try {
      switch (request.command) {
        case "status":
          await handleStatus(args);
          break;
        case "runs":
          await handleRuns(args);
          break;
        case "gate":
          await handleGate(args);
          break;
        case "advance":
          await handleAdvance(args);
          break;
        case "reset":
          await handleReset(args);
          break;
        case "init":
          await handleInit(args);
          break;
        default:
          renderHelp(stream);
      }
    } catch (err) {
      stream.markdown(formatError(err));
    }
    return {};
  };

  const participant = vscode.chat.createChatParticipant("sim-flow", handler);
  participant.iconPath = new vscode.ThemeIcon("flame");
  participant.followupProvider = {
    provideFollowups: async () => {
      const ctx = await resolveContext({ showErrors: false });
      if (!ctx) {
        return [];
      }
      try {
        const state = await readFlowState(ctx.projectDir);
        const order = flowOrderFor(state.flow);
        return suggestFollowups(state, order).map((f) => ({
          prompt: f.prompt,
          label: f.label,
          command: f.command,
          participant: "sim-flow",
        }));
      } catch {
        return [];
      }
    },
  };

  extensionContext.subscriptions.push(participant);
}

// ---------------------------------------------------------------------
// /step + follow-up runners
// ---------------------------------------------------------------------

async function runStepCommand(
  registry: SessionRegistry,
  ctx: { projectDir: string; cli: { binary: string; foundationRoot?: string } },
  prompt: string,
  stream: vscode.ChatResponseStream,
  secrets: SecretStorage,
  _token: vscode.CancellationToken,
): Promise<vscode.ChatResult> {
  const parsed = parseStepRef(prompt);
  if ("error" in parsed) {
    stream.markdown(`**Error:** ${parsed.error}\n`);
    return {};
  }
  const { step, kind, candidate } = parsed;

  const config = vscode.workspace.getConfiguration("sim-flow");
  const llmConfig = buildPumpLlmConfig(ctx, secrets, config);

  const args = ["session", `${step}.${kind}`, "--jsonl"];
  if (ctx.cli.foundationRoot) {
    args.push("--foundation-root", ctx.cli.foundationRoot);
  }
  args.push("--project", ctx.projectDir);
  args.push("--llm-backend", llmConfig.source);
  if (llmConfig.model) {
    args.push("--llm-model", llmConfig.model);
  }
  if (candidate) {
    args.push("--candidate", candidate);
  }

  const pump = new SessionPump(
    {
      binary: ctx.cli.binary,
      args,
      cwd: ctx.projectDir,
    },
    llmConfig,
  );
  const key = freshSessionKey();
  registry.insert(key, pump);

  const renderer = rendererFromChatStream(stream);
  const result = await pump.settle(renderer);
  if (result.status === "ended") {
    registry.remove(key);
    return {};
  }

  const meta: SessionMetadata = {
    tag: "sim-flow",
    pumpKey: key,
    step,
    kind,
    projectDir: ctx.projectDir,
  };
  return { metadata: meta as unknown as Record<string, unknown> };
}

async function runAutoCommand(
  registry: SessionRegistry,
  ctx: { projectDir: string; cli: { binary: string; foundationRoot?: string } },
  prompt: string,
  stream: vscode.ChatResponseStream,
  secrets: SecretStorage,
  _token: vscode.CancellationToken,
): Promise<vscode.ChatResult> {
  const config = vscode.workspace.getConfiguration("sim-flow");
  const llmConfig = buildPumpLlmConfig(ctx, secrets, config);
  const maxWorkIters = config.get<number>("auto.maxWorkIterations") ?? 3;
  const maxCritiqueIters = config.get<number>("auto.maxCritiqueIterations") ?? 3;
  const maxLlmRequests = config.get<number>("auto.maxLlmRequests") ?? 500;
  const noPreamble = config.get<boolean>("auto.noPreamble") ?? true;
  const cargoTimeoutSecs = config.get<number>("auto.cargoTimeoutSeconds") ?? 300;
  const { specPath } = extractSpecPath(prompt);

  const args = ["auto"];
  if (ctx.cli.foundationRoot) {
    args.push("--foundation-root", ctx.cli.foundationRoot);
  }
  args.push("--project", ctx.projectDir);
  args.push("--llm-backend", llmConfig.source);
  if (llmConfig.model) {
    args.push("--llm-model", llmConfig.model);
  }
  args.push("--max-auto-iters", String(maxWorkIters));
  args.push("--max-critique-iters", String(maxCritiqueIters));
  args.push("--max-llm-requests", String(maxLlmRequests));
  args.push("--no-preamble", String(noPreamble));
  if (specPath) {
    args.push("--spec", specPath);
  } else {
    // No spec on disk: DM0 has nothing to derive `spec.md` from, so
    // we explicitly drop into interactive mode for the DM0.work
    // session. The agent's instructions in
    // `instructions/dm0-specification.md` say "if no spec.md, draft
    // a skeleton and walk the user through filling it in" -- but
    // that branch only fires when the orchestrator's auto flag is
    // false. Without `--dm0-interactive` here we'd run DM0 fully
    // unattended and the agent would fabricate a spec from thin
    // air. Subsequent steps still run unattended.
    args.push("--dm0-interactive");
  }

  const specLine = specPath
    ? `Spec: \`${specPath}\` (will be ingested into \`.sim-flow/spec-pages/\` and indexed in a TOC).`
    : "_No spec provided; DM0.work runs interactively (the agent will ask you what to build), the rest of the flow runs unattended._";
  stream.markdown(
    [
      `**Starting automated flow.** Settings: \`maxWorkIterations\`=${maxWorkIters}, \`maxCritiqueIterations\`=${maxCritiqueIters}, \`maxLlmRequests\`=${maxLlmRequests}, \`noPreamble\`=${noPreamble}, \`cargoTimeoutSeconds\`=${cargoTimeoutSecs}. Backend: \`${llmConfig.source}\`.`,
      "",
      specLine,
      "",
    ].join("\n"),
  );

  const pump = new SessionPump(
    {
      binary: ctx.cli.binary,
      args,
      cwd: ctx.projectDir,
      env: {
        ...process.env,
        SIM_FLOW_CARGO_TIMEOUT_SECS: String(cargoTimeoutSecs),
      },
    },
    llmConfig,
  );
  const key = freshSessionKey();
  registry.insert(key, pump);

  const renderer = rendererFromChatStream(stream);
  const result = await pump.settle(renderer);
  if (result.status === "ended") {
    registry.remove(key);
    return {};
  }

  // The auto pump is allowed to settle on awaiting-input only when
  // it has dropped to interactive after a cap. Reuse the same
  // SessionMetadata machinery so free-form follow-ups route back to
  // this pump just like a /step session would.
  const meta: SessionMetadata = {
    tag: "sim-flow",
    pumpKey: key,
    step: "auto",
    kind: "work",
    projectDir: ctx.projectDir,
  };
  return { metadata: meta as unknown as Record<string, unknown> };
}

async function runStepFollowUp(
  registry: SessionRegistry,
  meta: SessionMetadata,
  prompt: string,
  stream: vscode.ChatResponseStream,
  _token: vscode.CancellationToken,
): Promise<vscode.ChatResult> {
  const pump = registry.get(meta.pumpKey);
  if (!pump) {
    stream.markdown(
      "_Previous session is no longer running. Start a fresh `/step <step>.<kind>` to continue._\n",
    );
    return {};
  }
  pump.sendUserMessage(prompt);
  const renderer = rendererFromChatStream(stream);
  const result = await pump.settle(renderer);
  if (result.status === "ended") {
    registry.remove(meta.pumpKey);
    return {};
  }
  return { metadata: meta as unknown as Record<string, unknown> };
}

function buildPumpLlmConfig(
  ctx: { projectDir: string; cli: { binary: string } },
  secrets: SecretStorage,
  config: vscode.WorkspaceConfiguration,
): PumpLlmConfig {
  const source = (config.get<LlmSource>("llm.source") ?? "vscode") as LlmSource;
  const model = (config.get<string>("llm.model") ?? "").trim() || undefined;
  const ollamaBaseUrl = (config.get<string>("llm.ollama.baseUrl") ?? "").trim() || undefined;
  const lmstudioBaseUrl = (config.get<string>("llm.lmstudio.baseUrl") ?? "").trim() || undefined;
  // Setting takes precedence; fall back to the env var so a CLI
  // smoke test from a shell still works without poking at settings.
  const settingTokens = (config.get<string[]>("debug") ?? []).join(",");
  const envTokens = (process.env["SIM_FOUNDATION_DEBUG"] ?? "").trim();
  const debugTokens = settingTokens.length > 0 ? settingTokens : envTokens;
  return {
    source,
    model,
    ollamaBaseUrl,
    lmstudioBaseUrl,
    secrets,
    projectDir: ctx.projectDir,
    binary: ctx.cli.binary,
    debugTokens,
  };
}

// ---------------------------------------------------------------------
// History helpers (unchanged from M2 except for the `pumpKey` field
// the SessionMetadata now carries).
// ---------------------------------------------------------------------

function inheritProjectDir(
  history: readonly (vscode.ChatRequestTurn | vscode.ChatResponseTurn)[],
): string | undefined {
  for (let i = history.length - 1; i >= 0; i--) {
    const turn = history[i];
    if (turn instanceof vscode.ChatResponseTurn) {
      const meta = (turn.result as { metadata?: unknown } | undefined)?.metadata;
      if (meta && typeof meta === "object") {
        const candidate = (meta as { projectDir?: unknown }).projectDir;
        if (typeof candidate === "string" && candidate.length > 0) {
          return candidate;
        }
      }
      continue;
    }
    if (turn instanceof vscode.ChatRequestTurn) {
      const { hint } = extractProjectHint(turn.prompt);
      if (hint) {
        return hint;
      }
    }
  }
  return undefined;
}

function latestSessionMetadata(
  history: readonly (vscode.ChatRequestTurn | vscode.ChatResponseTurn)[],
): SessionMetadata | undefined {
  for (let i = history.length - 1; i >= 0; i--) {
    const turn = history[i];
    if (!(turn instanceof vscode.ChatResponseTurn)) {
      continue;
    }
    const meta = (turn.result as { metadata?: unknown } | undefined)?.metadata;
    if (meta && typeof meta === "object" && (meta as { tag?: unknown }).tag === "sim-flow") {
      const m = meta as Partial<SessionMetadata>;
      if (
        typeof m.pumpKey === "string" &&
        typeof m.step === "string" &&
        (m.kind === "work" || m.kind === "critique")
      ) {
        return {
          tag: "sim-flow",
          pumpKey: m.pumpKey,
          step: m.step,
          kind: m.kind,
          projectDir: typeof m.projectDir === "string" ? m.projectDir : undefined,
        };
      }
    }
  }
  return undefined;
}

async function renderResolutionError(
  stream: vscode.ChatResponseStream,
  hint: string | undefined,
): Promise<void> {
  if (hint) {
    stream.markdown(
      `**Cannot resolve \`${hint}\`.** That path does not contain a \`.sim-flow/state.toml\`. Pass a different \`--project <path>\` or run \`/init\` to initialize a project there.\n`,
    );
    return;
  }
  const candidates = await findProjectCandidates();
  if (candidates.length === 0) {
    stream.markdown(
      "**Cannot run sim-flow here.** Open a workspace that contains `.sim-flow/state.toml` or initialize one with `/init`.\n",
    );
    return;
  }
  const list = candidates.map((c) => `- \`${c}\``).join("\n");
  stream.markdown(
    [
      "**Multiple sim-flow projects found.** Pass `--project <path>` to target one:",
      "",
      list,
      "",
      "Example: `@sim-flow /status --project " + candidates[0] + "`",
    ].join("\n") + "\n",
  );
}

function renderHelp(stream: vscode.ChatResponseStream): void {
  stream.markdown(
    [
      "**@sim-flow** — orchestrator commands:",
      "",
      "- `/status` — show flow + gate status",
      "- `/runs [--workload W] [--candidate C] [--study S] [--sweep ID] [--limit N]`",
      "- `/gate [step] [--candidate C]` — structural gate check (read-only)",
      "- `/advance [step]` — gate-validate and advance current_step",
      "- `/reset <step>` — reset a step, cascading downstream gates",
      "- `/step <step>.work` or `/step <step>.critique` — start an interactive session",
      "- `/auto` — drive the entire flow unattended from the current step (work → critique → advance per step). Settings: `sim-flow.auto.maxWorkIterations`, `sim-flow.auto.maxCritiqueIterations`, `sim-flow.auto.maxLlmRequests`, `sim-flow.auto.noPreamble`, `sim-flow.auto.cargoTimeoutSeconds`, `sim-flow.auto.healthcheck`.",
      "- `/init` — initialize sim-flow state in this workspace",
      "",
      "Any command accepts `--project <path>` to target a specific sim-flow project.",
    ].join("\n") + "\n",
  );
}

function formatError(err: unknown): string {
  if (err && typeof err === "object" && "kind" in err) {
    const e = err as SimFlowCliError;
    const detail = e.stderr?.trim() || e.message;
    return `**Error** (${e.kind}): ${detail}\n`;
  }
  return `**Error:** ${String((err as Error).message ?? err)}\n`;
}
