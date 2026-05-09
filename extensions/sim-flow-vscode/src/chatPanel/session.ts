import {
  createBackend,
  DEFAULT_RESPONSE_NORMALIZER,
  LlmError,
  type LlmMessage,
  type LlmSource,
  type SecretStorage,
  type CancellationLike,
} from "../llm";
import { BREVITY_DIRECTIVE } from "../session/pump";
import { isTerminalLlmSource, type LlmSourceTag } from "../webview/messages";

import type { ChatTranscriptEntry } from "./messages";
import { toLlmMessages } from "./state";

export interface ChatPanelTransportConfig {
  /** Resolved backend kind. The host has already mapped any
   *  `server:<name>` reference to the entry's `kind`. */
  source: LlmSourceTag;
  /** Resolved base URL, when the source maps to a custom server.
   *  Wins over the legacy per-backend URL fields. */
  baseUrl?: string;
  model: string;
  modelFamilyId?: string;
  runtimeProfileId?: string;
  verbose: boolean;
  ollamaBaseUrl: string;
  lmstudioBaseUrl: string;
  secrets: SecretStorage;
}

export interface ChatPanelPromptContext {
  projectDir: string | null;
  currentStep: string | null;
  transcript: ChatTranscriptEntry[];
}

export function supportsPanelTransport(source: LlmSourceTag): boolean {
  return !isTerminalLlmSource(source);
}

export async function* streamPanelReply(
  config: ChatPanelTransportConfig,
  context: ChatPanelPromptContext,
  token: CancellationLike,
): AsyncIterable<string> {
  if (!supportsPanelTransport(config.source)) {
    throw new LlmError(
      "unsupported",
      'This panel only supports API backends. Switch `sim-flow.llm.source` to `lmstudio`, `ollama`, `openai`, `anthropic`, or `vscode` to send prompts here.',
    );
  }
  const backend = createBackend({
    source: config.source as LlmSource,
    model: config.model || undefined,
    modelFamilyId: config.modelFamilyId,
    runtimeProfileId: config.runtimeProfileId,
    secrets: config.secrets,
    ollamaBaseUrl: config.ollamaBaseUrl,
    lmstudioBaseUrl: config.lmstudioBaseUrl,
    // Resolved by the host before we get here -- wins over the
    // legacy per-backend URL fields when set.
    baseUrl: config.baseUrl,
  });
  const messages = buildPanelMessages(
    context,
    context.transcript,
    config.verbose,
  );
  const responseNormalizer =
    backend.adaptation?.responseNormalizer ?? DEFAULT_RESPONSE_NORMALIZER;
  for await (const rawChunk of backend.stream(messages, token)) {
    for (const chunk of responseNormalizer.normalizeChunk(rawChunk)) {
      if (chunk.kind === "reasoning" || chunk.text.length === 0) {
        continue;
      }
      yield chunk.text;
    }
  }
  for (const chunk of responseNormalizer.flush?.() ?? []) {
    if (chunk.kind === "reasoning" || chunk.text.length === 0) {
      continue;
    }
    yield chunk.text;
  }
}

export function buildPanelMessages(
  context: Pick<ChatPanelPromptContext, "projectDir" | "currentStep">,
  transcript: ChatTranscriptEntry[],
  verbose: boolean,
): LlmMessage[] {
  return toLlmMessages(transcript, buildSystemPrompt(context, verbose));
}

function buildSystemPrompt(
  context: Pick<ChatPanelPromptContext, "projectDir" | "currentStep">,
  verbose: boolean,
): string {
  const lines = [
    "You are the sim-flow chat panel assistant inside VS Code.",
    "Answer the user's prompt directly and stay grounded in the current workspace.",
    context.projectDir ? `Current project directory: ${context.projectDir}` : "",
    context.currentStep ? `Current sim-flow step: ${context.currentStep}` : "",
    "When you mention files, prefer concise project-relative paths when possible.",
    verbose ? "" : BREVITY_DIRECTIVE,
  ].filter((line) => line.length > 0);
  return lines.join("\n\n");
}
