// Surface re-exports for the slim post-orchestrator-takeover LLM
// module. Only two pieces of LLM-adjacent code remain in the
// extension:
//
//   - `enumerateModels` — used by the dashboard to populate the
//     model dropdown.
//   - The `LlmSource` / `SecretStorage` types — used by callers
//     that need to label a backend selection or look up a key.
//
// All actual LLM dispatch (openai-compat, anthropic, ollama,
// lmstudio, vscode chat models) was deleted; the Rust orchestrator
// drives those backends directly now.

export { enumerateModels } from "./enumerate";
export type { LlmSource, SecretStorage } from "./types";
