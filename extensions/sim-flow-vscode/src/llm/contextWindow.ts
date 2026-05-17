// Query each LLM backend for the actual context window of the model
// in use. Used by the chat panel host to seed the toolbar's "%
// context used" pie with a real denominator instead of the cosmetic
// 128k constant the webview used to fall back on.
//
// Best-effort: any failure (no API key, unreachable backend, missing
// field, schema drift on the server side) returns null. The webview
// falls back to the legacy 128k constant in that case so the pie
// still renders something.

import { llmServerBaseUrl, type LlmServerEntry, type LlmSourceTag } from "../webview/messages";

export interface ContextWindowQuery {
  /** Resolved backend kind. Server:* references already mapped. */
  source: LlmSourceTag;
  /** Base URL for OpenAI-compat backends. Ignored for `anthropic`,
   *  `vscode`, and terminal-only sources. */
  baseUrl?: string;
  /** Model id the orchestrator will dispatch with. */
  model: string;
  /** API key for sources that need one (`anthropic`). */
  apiKey?: string;
}

const FETCH_TIMEOUT_MS = 4000;

/**
 * Resolve the LLM's actual context window. Returns `null` on any
 * failure; callers fall back to a UI default. The query never
 * throws -- network errors / schema mismatches / unknown sources
 * all resolve to `null`.
 */
export async function queryContextWindow(
  q: ContextWindowQuery,
): Promise<number | null> {
  try {
    switch (q.source) {
      case "anthropic":
        return await queryAnthropic(q);
      case "lmstudio":
        return await queryLmStudio(q);
      case "ollama":
        return await queryOllama(q);
      case "vllm":
      case "openai":
        return await queryOpenAiCompatModels(q);
      default:
        return null;
    }
  } catch {
    return null;
  }
}

/**
 * Same as `queryContextWindow` but routed via an explicit
 * user-defined server entry (`server:<name>` source). The entry's
 * `kind` determines which transport to use.
 */
export async function queryContextWindowForServer(
  entry: LlmServerEntry,
  model: string,
): Promise<number | null> {
  const baseUrl = llmServerBaseUrl(entry);
  switch (entry.kind) {
    case "lmstudio":
      return queryLmStudio({ source: "lmstudio", baseUrl, model });
    case "ollama":
      return queryOllama({ source: "ollama", baseUrl, model });
    case "vllm":
    case "openai-compat":
      return queryOpenAiCompatModels({ source: "vllm", baseUrl, model });
    default:
      return null;
  }
}

async function queryAnthropic(q: ContextWindowQuery): Promise<number | null> {
  if (!q.apiKey) {
    return null;
  }
  const model = encodeURIComponent(q.model);
  const url = `https://api.anthropic.com/v1/models/${model}`;
  const body = await fetchJson(url, {
    headers: {
      "x-api-key": q.apiKey,
      "anthropic-version": "2023-06-01",
    },
  });
  if (!body || typeof body !== "object") return null;
  const ctx = (body as Record<string, unknown>).context_window;
  return typeof ctx === "number" && ctx > 0 ? ctx : null;
}

async function queryLmStudio(q: ContextWindowQuery): Promise<number | null> {
  if (!q.baseUrl) return null;
  // LM Studio's OpenAI-compat URL ends in `/v1`; the native API
  // lives alongside at `/api/v0`. Strip a trailing `/v1` (with or
  // without trailing slash) and append the native path.
  const root = q.baseUrl.replace(/\/v1\/?$/, "");
  const model = encodeURIComponent(q.model);
  const url = `${root}/api/v0/models/${model}`;
  const body = await fetchJson(url);
  if (!body || typeof body !== "object") return null;
  const obj = body as Record<string, unknown>;
  const loaded = obj.loaded_context_length;
  if (typeof loaded === "number" && loaded > 0) {
    return loaded;
  }
  const max = obj.max_context_length;
  return typeof max === "number" && max > 0 ? max : null;
}

async function queryOllama(q: ContextWindowQuery): Promise<number | null> {
  if (!q.baseUrl) return null;
  // Ollama's OpenAI-compat sits at /v1; the native API is at /api.
  const root = q.baseUrl.replace(/\/v1\/?$/, "");
  const url = `${root}/api/show`;
  const body = await fetchJson(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name: q.model }),
  });
  if (!body || typeof body !== "object") return null;
  const info = (body as Record<string, unknown>).model_info;
  if (!info || typeof info !== "object") return null;
  // The key is `<arch>.context_length` and `<arch>` varies per model
  // (e.g. `llama.context_length`, `qwen2.context_length`). Take the
  // first matching key.
  for (const [key, val] of Object.entries(info as Record<string, unknown>)) {
    if (key.endsWith(".context_length") && typeof val === "number" && val > 0) {
      return val;
    }
  }
  return null;
}

async function queryOpenAiCompatModels(
  q: ContextWindowQuery,
): Promise<number | null> {
  if (!q.baseUrl) return null;
  // The OpenAI spec doesn't standardise context size, but vLLM adds
  // `max_model_len` to each model entry under /v1/models. Some
  // OpenAI-compat servers expose the same field.
  const url = `${q.baseUrl.replace(/\/$/, "")}/models`;
  const body = await fetchJson(url);
  if (!body || typeof body !== "object") return null;
  const data = (body as Record<string, unknown>).data;
  if (!Array.isArray(data)) return null;
  const match = data.find(
    (entry) =>
      entry && typeof entry === "object" &&
      (entry as Record<string, unknown>).id === q.model,
  );
  const obj = match as Record<string, unknown> | undefined;
  if (!obj) return null;
  const v = obj.max_model_len;
  return typeof v === "number" && v > 0 ? v : null;
}

async function fetchJson(
  url: string,
  init?: RequestInit,
): Promise<unknown | null> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), FETCH_TIMEOUT_MS);
  try {
    const res = await fetch(url, { ...init, signal: controller.signal });
    if (!res.ok) {
      return null;
    }
    return await res.json();
  } finally {
    clearTimeout(timer);
  }
}
