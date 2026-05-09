// LM Studio backend. LM Studio's local server hosts an
// OpenAI-compatible endpoint at `<host>/v1/chat/completions`. The
// model id must match whatever is currently loaded in LM Studio,
// so we don't set a meaningful default - callers should supply
// `sim-flow.llm.model`.

import { OpenAiCompatibleBackend } from "./openai-compat";
import type { SecretStorage } from "./types";

export const LMSTUDIO_KEY_ID = "sim-flow.lmstudio.apiKey";
export const LMSTUDIO_DEFAULT_BASE_URL = "http://localhost:1234/v1";

export interface LMStudioBackendOptions {
  model?: string;
  modelFamilyId?: string;
  runtimeProfileId?: string;
  secrets?: SecretStorage;
  baseUrl?: string;
  apiUrl?: string;
  fetchImpl?: typeof fetch;
  /**
   * Override the backend's display name, used in error
   * diagnostics. The factory routes `vllm` and `openai-compat`
   * through this class (same wire format as LM Studio);
   * labeling them "lmstudio" in the error obscures the actual
   * selector the user picked.
   */
  name?: string;
}

export class LMStudioBackend extends OpenAiCompatibleBackend {
  constructor(options: LMStudioBackendOptions = {}) {
    super({
      name: options.name ?? "lmstudio",
      baseUrl: options.baseUrl ?? LMSTUDIO_DEFAULT_BASE_URL,
      defaultModel: "local-model",
      provider: "lmstudio",
      keyId: LMSTUDIO_KEY_ID,
      requireKey: false,
      model: options.model,
      modelFamilyId: options.modelFamilyId,
      runtimeProfileId: options.runtimeProfileId,
      secrets: options.secrets,
      apiUrl: options.apiUrl,
      fetchImpl: options.fetchImpl,
    });
  }
}
