// Per-source model enumeration. The dashboard's model dropdown
// queries this when the user picks a new LLM source (or clicks the
// refresh button); the result populates the dropdown.
//
// Each source has its own enumeration story:
//
// - vscode  -> `vscode.lm.selectChatModels({})` returns every chat
//              model registered via `registerChatModelProvider`. We
//              report unique `family` strings (which is what the
//              `vscode` backend's `selectChatModels({ family })`
//              selector accepts).
// - lmstudio / ollama -> `GET <baseUrl>/models` returns the
//              OpenAI-compatible `{data: [{id, ...}]}` shape; we
//              return each `id`.
// - openai / anthropic -> hardcoded lists. `/v1/models` either
//              requires auth (OpenAI) or doesn't exist (Anthropic),
//              and the lists drift slowly. We bias toward the small
//              set a sim-flow user actually wants to pick from.
// - claude-cli / codex-cli / gh-copilot-cli -> hardcoded short-name
//              aliases each CLI accepts via its `--model` flag (or
//              `(default)` when the CLI doesn't expose model choice).

import * as vscode from "vscode";

import type { LlmSource } from "./types";

export interface EnumerateOptions {
  source: LlmSource;
  ollamaBaseUrl?: string;
  lmstudioBaseUrl?: string;
  /**
   * Generic OpenAI-compat base URL override. When set, takes
   * precedence over `ollamaBaseUrl` / `lmstudioBaseUrl` for the
   * `ollama` / `lmstudio` / `vllm` / `openai-compat` sources.
   * Used when the dashboard resolved a `server:<name>` entry to
   * a `host:port` pair.
   */
  baseUrl?: string;
  /** Injectable for tests. */
  fetchImpl?: typeof fetch;
  /** Injectable for tests; defaults to `vscode.lm.selectChatModels`. */
  vscodeLm?: { selectChatModels: typeof vscode.lm.selectChatModels };
}

export interface EnumerateResult {
  models: string[];
  /** Populated when enumeration succeeded but the source returned no models. */
  emptyReason?: string;
  /** Populated when the call itself failed (network, auth, etc.). */
  error?: string;
}

export async function enumerateModels(opts: EnumerateOptions): Promise<EnumerateResult> {
  switch (opts.source) {
    case "vscode":
      return await enumerateVscode(opts);
    case "lmstudio": {
      const base = opts.baseUrl ?? opts.lmstudioBaseUrl ?? "http://localhost:1234/v1";
      const url = base.replace(/\/+$/, "") + "/models";
      return await fetchOpenAiModels(url, opts.fetchImpl);
    }
    case "ollama": {
      const base = opts.baseUrl ?? opts.ollamaBaseUrl ?? "http://localhost:11434/v1";
      const url = base.replace(/\/+$/, "") + "/models";
      return await fetchOpenAiModels(url, opts.fetchImpl);
    }
    case "vllm": {
      const base = opts.baseUrl ?? "http://localhost:8000/v1";
      const url = base.replace(/\/+$/, "") + "/models";
      return await fetchOpenAiModels(url, opts.fetchImpl);
    }
    case "openai-compat": {
      const base = opts.baseUrl ?? "http://localhost:1234/v1";
      const url = base.replace(/\/+$/, "") + "/models";
      return await fetchOpenAiModels(url, opts.fetchImpl);
    }
    case "openai":
      return {
        models: [
          "gpt-4o",
          "gpt-4o-mini",
          "gpt-4.1",
          "gpt-4.1-mini",
          "o1",
          "o1-preview",
          "o3-mini",
        ],
      };
    case "anthropic":
      return {
        models: [
          "claude-opus-4-7",
          "claude-opus-4-5",
          "claude-sonnet-4-6",
          "claude-sonnet-4-5",
          "claude-haiku-4-5",
        ],
      };
    case "claude-cli":
      // `claude --model <name>` accepts these aliases plus full
      // model ids; aliases are friendlier in a dropdown.
      return { models: ["sonnet", "opus", "haiku"] };
    case "codex-cli":
      return { models: ["o1", "o1-mini"] };
    case "gh-copilot-cli":
      return { models: ["(default)"] };
    default: {
      const _exhaustive: never = opts.source;
      void _exhaustive;
      return { models: [], error: `Unknown source: ${String(opts.source)}` };
    }
  }
}

async function enumerateVscode(opts: EnumerateOptions): Promise<EnumerateResult> {
  const lm = opts.vscodeLm ?? vscode.lm;
  try {
    const models = await lm.selectChatModels({});
    if (models.length === 0) {
      return {
        models: [],
        emptyReason:
          "No chat-model providers are registered. Install Copilot or the Claude Code extension.",
      };
    }
    // Emit `vendor/family` for every (vendor, family) pair the LM
    // API exposes. Copilot and Claude Code both publish `claude-*`
    // family ids, so we have to scope by vendor too -- otherwise
    // `selectChatModels({family})` happily returns Copilot's model
    // and the user's Pro/Team Claude Code subscription stays unused
    // (and Copilot's quota ticks).
    const seen = new Set<string>();
    for (const m of models) {
      const vendor = typeof (m as { vendor?: unknown }).vendor === "string"
        ? (m as { vendor: string }).vendor
        : "";
      const family = typeof (m as { family?: unknown }).family === "string"
        ? (m as { family: string }).family
        : "";
      if (family.length === 0) {continue;}
      const key = vendor.length > 0 ? `${vendor}/${family}` : family;
      seen.add(key);
    }
    return { models: [...seen].sort() };
  } catch (err) {
    return { models: [], error: `vscode.lm.selectChatModels failed: ${(err as Error).message}` };
  }
}

async function fetchOpenAiModels(
  url: string,
  fetchImpl?: typeof fetch,
): Promise<EnumerateResult> {
  const doFetch = fetchImpl ?? globalThis.fetch;
  if (typeof doFetch !== "function") {
    return { models: [], error: "`fetch` is not available; Node 18+ or a polyfill is required." };
  }
  try {
    const res = await doFetch(url, { headers: { accept: "application/json" } });
    if (!res.ok) {
      return {
        models: [],
        error: `${url} returned ${res.status} ${res.statusText}`,
      };
    }
    const json = (await res.json()) as { data?: unknown };
    if (!Array.isArray(json.data)) {
      return { models: [], error: `${url} returned an unexpected payload (no .data array)` };
    }
    const ids: string[] = [];
    for (const entry of json.data) {
      const id = (entry as { id?: unknown }).id;
      if (typeof id === "string" && id.length > 0) {
        ids.push(id);
      }
    }
    if (ids.length === 0) {
      return {
        models: [],
        emptyReason: "Server is reachable but no models are loaded.",
      };
    }
    return { models: ids.sort() };
  } catch (err) {
    return { models: [], error: `${url} fetch failed: ${(err as Error).message}` };
  }
}
