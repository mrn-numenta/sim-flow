// Ollama backend. Ollama exposes an OpenAI-compatible chat
// endpoint at `<host>/v1/chat/completions`. No API key is needed
// for the default local install, but the key lookup is honored so
// users can lock a remote Ollama instance behind a reverse proxy.

import { OpenAiCompatibleBackend } from "./openai-compat";
import type { SecretStorage } from "./types";

export const OLLAMA_KEY_ID = "sim-flow.ollama.apiKey";
export const OLLAMA_DEFAULT_BASE_URL = "http://localhost:11434/v1";

export interface OllamaBackendOptions {
  model?: string;
  secrets?: SecretStorage;
  baseUrl?: string;
  apiUrl?: string;
  fetchImpl?: typeof fetch;
}

export class OllamaBackend extends OpenAiCompatibleBackend {
  constructor(options: OllamaBackendOptions = {}) {
    super({
      name: "ollama",
      baseUrl: options.baseUrl ?? OLLAMA_DEFAULT_BASE_URL,
      defaultModel: "llama3.1",
      keyId: OLLAMA_KEY_ID,
      requireKey: false,
      model: options.model,
      secrets: options.secrets,
      apiUrl: options.apiUrl,
      fetchImpl: options.fetchImpl,
    });
  }
}
