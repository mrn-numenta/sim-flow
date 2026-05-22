/**
 * UI-side dashboard helpers: model enumeration, prompt-list +
 * override editing, spec / critique / document opening, and the
 * block-diagram regenerate hook. Extracted from `host.ts` to keep
 * that file under the 1000-line refactor threshold.
 */

import * as path from "node:path";
import * as vscode from "vscode";

import { enumerateModels } from "../../llm/enumerate";
import type { LlmSource } from "../../llm/types";
import type { SimFlowCli } from "../../cli/simflow";
import type { HostMessage, LlmServerEntry, LlmSourceTag } from "../messages";

export interface UiContext {
  readonly options: { projectDir: string; cli: SimFlowCli };
  readonly panel: vscode.WebviewPanel | undefined;
  post(msg: HostMessage): Promise<boolean>;
  sendPromptsList(): Promise<void>;
}

export async function sendModelList(ctx: UiContext, source: LlmSourceTag | string): Promise<void> {
  const config = vscode.workspace.getConfiguration("sim-flow");
  const ollamaBaseUrl = config.get<string>("llm.ollama.baseUrl") ?? undefined;
  const lmstudioBaseUrl = config.get<string>("llm.lmstudio.baseUrl") ?? undefined;
  let resolvedSource: LlmSource = source as LlmSource;
  let baseUrl: string | undefined;
  if (typeof source === "string" && source.startsWith("server:")) {
    const name = source.slice("server:".length);
    const servers = (config.get<unknown>("llm.servers") as LlmServerEntry[] | undefined) ?? [];
    const entry = servers.find((s) => s.name === name);
    if (entry) {
      resolvedSource = entry.kind as LlmSource;
      const entryPath = entry.path && entry.path.length > 0 ? entry.path : "/v1";
      const normalisedPath = entryPath.startsWith("/") ? entryPath : `/${entryPath}`;
      baseUrl = `http://${entry.host}:${entry.port}${normalisedPath}`;
    }
  }
  const result = await enumerateModels({
    source: resolvedSource,
    ollamaBaseUrl,
    lmstudioBaseUrl,
    baseUrl,
  });
  await ctx.post({
    type: "model-list",
    source,
    models: result.models,
    emptyReason: result.emptyReason,
    error: result.error,
  });
}

export async function sendPromptsList(ctx: UiContext): Promise<void> {
  try {
    const entries = await ctx.options.cli.promptsList();
    await ctx.post({ type: "prompts-list-result", entries });
  } catch (err) {
    await ctx.post({
      type: "error",
      message: "Failed to list prompts",
      detail: String((err as Error).message ?? err),
    });
  }
}

/**
 * Open a prompt override in a regular VS Code editor tab.
 *
 * The foundation-default prompt is intentionally never opened from
 * here -- the user can only edit at the `project` or `global`
 * override scope, which means VS Code's normal save flow can't
 * write back to the foundation tree. If the chosen override file
 * doesn't yet exist, we seed it with the currently-effective
 * resolved content (foundation default OR whatever the active
 * scope is) so the user has a working starting point rather than
 * an empty buffer.
 *
 * Saves use VS Code's standard file save -- nothing extra to do
 * on this side. The prompt resolver inside the orchestrator
 * already prefers project > global > default, so saving the file
 * is sufficient to make the override active.
 */
export async function openPromptInEditor(
  ctx: UiContext,
  slug: string,
  kind: "work" | "critique",
  scope: "project" | "global",
): Promise<void> {
  try {
    const entries = await ctx.options.cli.promptsList();
    const entry = entries.find((e) => e.slug === slug && e.kind === kind);
    if (!entry) {
      await ctx.post({
        type: "error",
        message: `Prompt ${slug}.${kind} is not in the registry.`,
      });
      return;
    }
    const target = scope === "project" ? entry.project_path : entry.global_path;
    if (!target) {
      await ctx.post({
        type: "error",
        message: `No global prompt path is configured.`,
        detail:
          "The CLI did not return a global override location for this prompt. " +
          'Pick "Edit (project)" instead, or set up a user-config directory ' +
          "before retrying.",
      });
      return;
    }
    const targetUri = vscode.Uri.file(target);
    let exists = true;
    try {
      await vscode.workspace.fs.stat(targetUri);
    } catch {
      exists = false;
    }
    if (!exists) {
      // Seed with the current effective content so the editor opens
      // on a meaningful starting point rather than an empty buffer.
      const seed = await ctx.options.cli.promptShow(slug, kind);
      const parent = vscode.Uri.file(path.dirname(target));
      await vscode.workspace.fs.createDirectory(parent);
      await vscode.workspace.fs.writeFile(targetUri, Buffer.from(seed, "utf8"));
      await ctx.sendPromptsList();
    }
    const doc = await vscode.workspace.openTextDocument(targetUri);
    await vscode.window.showTextDocument(doc, { preview: false });
  } catch (err) {
    await ctx.post({
      type: "error",
      message: `Failed to open ${slug}.${kind} (${scope})`,
      detail: String((err as Error).message ?? err),
    });
  }
}

export async function resetPromptOverride(
  ctx: UiContext,
  slug: string,
  kind: "work" | "critique",
  scope: "project" | "global" | "all",
): Promise<void> {
  try {
    await ctx.options.cli.promptReset(slug, kind, scope);
    void vscode.window.showInformationMessage(`Reset ${slug}.${kind} override (${scope}).`);
    await ctx.sendPromptsList();
  } catch (err) {
    await ctx.post({
      type: "error",
      message: `Failed to reset ${slug}.${kind} (${scope})`,
      detail: String((err as Error).message ?? err),
    });
  }
}

export async function pickSpecFile(ctx: UiContext): Promise<void> {
  const picked = await vscode.window.showOpenDialog({
    canSelectFiles: true,
    canSelectFolders: false,
    canSelectMany: false,
    openLabel: "Select spec",
    filters: {
      Spec: ["pdf", "md", "txt"],
      "All files": ["*"],
    },
  });
  if (!picked || picked.length === 0) {
    return;
  }
  await ctx.post({ type: "spec-path-picked", path: picked[0]!.fsPath });
}

export async function regenerateBlockDiagram(ctx: UiContext): Promise<void> {
  try {
    await ctx.options.cli.blockDiagram();
  } catch (err) {
    await ctx.post({
      type: "error",
      message: "Block diagram generation failed",
      detail: String((err as Error).message ?? err),
    });
    return;
  }
  await postBlockDiagram(ctx);
}

export async function postBlockDiagram(ctx: UiContext): Promise<void> {
  const svgPath = path.join(ctx.options.projectDir, ".sim-flow", "block-diagram.svg");
  let svg: string | null = null;
  try {
    svg = await import("node:fs").then((fs) => fs.readFileSync(svgPath, "utf8"));
  } catch {
    svg = null;
  }
  await ctx.post({ type: "block-diagram", svg });
}

export async function openDocumentInEditor(absPath: string): Promise<void> {
  const uri = vscode.Uri.file(absPath);
  try {
    await vscode.window.showTextDocument(uri, { preview: false });
  } catch (err) {
    void vscode.window.showWarningMessage(
      `sim-flow: cannot open ${absPath}: ${(err as Error).message ?? String(err)}`,
    );
  }
}

export async function openCritiqueInEditor(ctx: UiContext, stepId: string): Promise<void> {
  const critique = path.join(
    ctx.options.projectDir,
    ".sim-flow",
    "critiques",
    `${stepId}-critique.md`,
  );
  const uri = vscode.Uri.file(critique);
  try {
    await vscode.window.showTextDocument(uri, { preview: true });
  } catch {
    void vscode.window.showWarningMessage(`No critique file found for ${stepId} at ${critique}`);
  }
}

export async function openAnalysisFolder(ctx: UiContext): Promise<void> {
  const dir = path.join(ctx.options.projectDir, "docs", "analysis");
  const uri = vscode.Uri.file(dir);
  try {
    await vscode.commands.executeCommand("revealInExplorer", uri);
  } catch {
    void vscode.window.showWarningMessage(`Could not open ${dir}`);
  }
}
