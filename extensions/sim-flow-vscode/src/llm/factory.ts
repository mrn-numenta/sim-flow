// Pick the LLM backend based on the `sim-flow.llm.source` setting.
// Phase 9 M5 retired the `cli` backend (the SessionPump now drives
// `sim-flow session ... --jsonl` directly) and the per-session
// metadata; only host-mediated dispatchers live here.

import { AnthropicBackend } from "./anthropic";
import { LMStudioBackend } from "./lmstudio";
import { OllamaBackend } from "./ollama";
import { OpenAiBackend } from "./openai";
import { type LlmBackend, LlmError, type LlmSource, type SecretStorage } from "./types";
import { VSCodeLmBackend } from "./vscode";

export interface FactoryOptions {
  source: LlmSource;
  model?: string;
  /** Optional explicit model-family override; otherwise inferred from `model`. */
  modelFamilyId?: string;
  /** Optional explicit runtime-profile override; otherwise source/backend default. */
  runtimeProfileId?: string;
  secrets?: SecretStorage;
  /** Reserved for future per-session backends (e.g. local model paths). */
  projectDir?: string;
  /** Reserved for future per-session backends. */
  binary?: string;
  /** Reserved for future per-session backends. */
  session?: unknown;
  /** Base URL override for the Ollama backend. */
  ollamaBaseUrl?: string;
  /** Base URL override for the LM Studio backend. */
  lmstudioBaseUrl?: string;
  /** Generic OpenAI-compat base URL override (vllm / openai-compat / user-defined servers). */
  baseUrl?: string;
}

export function createBackend(options: FactoryOptions): LlmBackend {
  switch (options.source) {
    case "vscode":
      return new VSCodeLmBackend({
        model: options.model,
        modelFamilyId: options.modelFamilyId,
        runtimeProfileId: options.runtimeProfileId,
      });
    case "anthropic":
      return new AnthropicBackend({
        model: options.model,
        modelFamilyId: options.modelFamilyId,
        runtimeProfileId: options.runtimeProfileId,
        secrets: options.secrets,
      });
    case "openai":
      return new OpenAiBackend({
        model: options.model,
        modelFamilyId: options.modelFamilyId,
        runtimeProfileId: options.runtimeProfileId,
        secrets: options.secrets,
      });
    case "ollama":
      // The generic `baseUrl` (set when resolving `server:<name>`)
      // wins over the legacy `llm.ollama.baseUrl` setting so a
      // user-defined ollama entry actually points where they say.
      return new OllamaBackend({
        model: options.model,
        modelFamilyId: options.modelFamilyId,
        runtimeProfileId: options.runtimeProfileId,
        secrets: options.secrets,
        baseUrl: options.baseUrl ?? options.ollamaBaseUrl,
      });
    case "lmstudio":
      return new LMStudioBackend({
        model: options.model,
        modelFamilyId: options.modelFamilyId,
        runtimeProfileId: options.runtimeProfileId,
        secrets: options.secrets,
        baseUrl: options.baseUrl ?? options.lmstudioBaseUrl,
      });
    case "vllm":
      // vLLM speaks OpenAI-compat at `:8000/v1` by default. Reuse
      // the LM Studio backend (same wire format); only the
      // default URL differs. Custom servers route here too via
      // `kind: "vllm"` in the user's `sim-flow.llm.servers` array.
      // Pass `name: "vllm"` so error diagnostics name the actual
      // selector the user picked, not "lmstudio".
      return new LMStudioBackend({
        name: "vllm",
        model: options.model,
        modelFamilyId: options.modelFamilyId,
        runtimeProfileId: options.runtimeProfileId,
        secrets: options.secrets,
        baseUrl: options.baseUrl ?? "http://localhost:8000/v1",
      });
    case "openai-compat":
      // Generic openai-compat fallback. Defaults to LM Studio's
      // `:1234/v1` so the conventional case still works without
      // the user typing a base URL.
      return new LMStudioBackend({
        name: "openai-compat",
        model: options.model,
        modelFamilyId: options.modelFamilyId,
        runtimeProfileId: options.runtimeProfileId,
        secrets: options.secrets,
        baseUrl: options.baseUrl ?? options.lmstudioBaseUrl ?? "http://localhost:1234/v1",
      });
    case "claude-cli":
    case "codex-cli":
    case "gh-copilot-cli":
      // CLI-agent sources don't have an HTTP backend the chat
      // participant can drive. They run as `sim-flow auto
      // --llm-backend <name>` in a terminal (see the dashboard's
      // Run/Resume Automated Flow button). If we got here, the user
      // somehow triggered a chat-pane dispatch (e.g. `@sim-flow
      // /step DM2c.work`) while a CLI source is selected -- tell
      // them why nothing's happening and how to recover.
      throw new LlmError(
        "unsupported",
        `LLM source \`${options.source}\` is a CLI agent and runs in a terminal, not the chat pane. Use the dashboard's "Run / Resume Automated Flow" button, or switch the picker to an API backend (vscode / anthropic / openai / ollama / lmstudio) for in-chat use.`,
      );
    default: {
      const _exhaustive: never = options.source;
      throw new LlmError("unsupported", `Unknown LLM source: ${String(_exhaustive)}`);
    }
  }
}
