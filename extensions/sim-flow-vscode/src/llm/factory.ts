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
}

export function createBackend(options: FactoryOptions): LlmBackend {
  switch (options.source) {
    case "vscode":
      return new VSCodeLmBackend({ model: options.model });
    case "anthropic":
      return new AnthropicBackend({
        model: options.model,
        secrets: options.secrets,
      });
    case "openai":
      return new OpenAiBackend({
        model: options.model,
        secrets: options.secrets,
      });
    case "ollama":
      return new OllamaBackend({
        model: options.model,
        secrets: options.secrets,
        baseUrl: options.ollamaBaseUrl,
      });
    case "lmstudio":
      return new LMStudioBackend({
        model: options.model,
        secrets: options.secrets,
        baseUrl: options.lmstudioBaseUrl,
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
