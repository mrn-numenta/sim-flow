// OpenAI Chat Completions backend. Thin subclass of the shared
// OpenAI-compatible base: fixes the production API URL, default
// model, and requires an API key from SecretStorage.

import { OpenAiCompatibleBackend, extractOpenAiText } from "./openai-compat";
import type { SecretStorage } from "./types";

export const OPENAI_KEY_ID = "sim-flow.openai.apiKey";

export interface OpenAiBackendOptions {
  model?: string;
  secrets?: SecretStorage;
  apiUrl?: string;
  baseUrl?: string;
  fetchImpl?: typeof fetch;
}

export class OpenAiBackend extends OpenAiCompatibleBackend {
  constructor(options: OpenAiBackendOptions = {}) {
    super({
      name: "openai",
      baseUrl: options.baseUrl ?? "https://api.openai.com/v1",
      defaultModel: "gpt-4o-mini",
      provider: "openai",
      keyId: OPENAI_KEY_ID,
      requireKey: true,
      model: options.model,
      secrets: options.secrets,
      apiUrl: options.apiUrl,
      fetchImpl: options.fetchImpl,
    });
  }
}

export { extractOpenAiText };
