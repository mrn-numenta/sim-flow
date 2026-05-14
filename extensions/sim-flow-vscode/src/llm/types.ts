// Minimal type surface kept after the orchestrator absorbed all
// LLM dispatch. The TypeScript-side LLM client family (openai,
// anthropic, ollama, lmstudio, vscode, ...) was deleted; the only
// remaining LLM-aware surfaces in the extension are:
//
//   - `llm/keyResolver.ts`  — credentials.toml read/write so the
//     "Set API Key" command can stage a key the orchestrator can
//     read later.
//   - `llm/enumerate.ts`    — populates the dashboard's model
//     dropdown by hitting each backend's `/models` endpoint (or
//     returning a hardcoded list).
//
// Both still reference `LlmSource` as the backend selector value
// the user picked. `SecretStorage` is the small interface the key
// resolver needs from `vscode.ExtensionContext.secrets` so tests
// can pass a mock.

/**
 * Backend selector mirrors the `sim-flow.llm.source` setting. The
 * extension no longer dispatches against this — the value is just
 * passed through to `sim-flow auto --llm-backend` and resolved by
 * the Rust orchestrator.
 *
 * Keep this enum in sync with `LlmSourceTag` in
 * `webview/messages.ts`. They're intentionally separate (one is
 * extension-side, one is webview-message-side) but list the same
 * values.
 */
export type LlmSource =
  | "vscode"
  | "anthropic"
  | "openai"
  | "ollama"
  | "lmstudio"
  | "vllm"
  | "openai-compat"
  | "claude-cli"
  | "codex-cli"
  | "gh-copilot-cli";

/**
 * Subset of `vscode.SecretStorage` the key resolver uses. Lets
 * tests pass a `Map`-backed mock without the full vscode API.
 */
export interface SecretStorage {
  get(key: string): PromiseLike<string | undefined>;
}
