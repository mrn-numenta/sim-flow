// Protocol-neutral translation between sim-flow's fenced-block tool
// representation and the structured native shapes used by OpenAI- /
// Anthropic-style tool-calling APIs.
//
// Sim-flow's wire protocol with the orchestrator carries tool calls
// as text:
//
//   - Assistant emits a fenced block whose info-string is
//     `tool:<name>` and whose body is the call's args (JSON or, for
//     a few simple tools, a single bare argument like a path).
//   - Orchestrator runs the tool, then pushes the result back as a
//     User message that begins with `"Tool results:\n\n"` and
//     concatenates per-call sections separated by `\n\n---\n\n`.
//
// Backends that speak a richer API (OpenAI's `tools` /
// `assistant.tool_calls` + `role: tool`, Anthropic's `tool_use` /
// `tool_result` content blocks) need the calls and results in their
// native shape so the model sees a clean conversation. This module
// extracts the pieces from the fenced-block format; each backend
// renders them into its own structure.

/**
 * One parsed tool call extracted from an assistant message. `args` is
 * the raw body the model emitted between the open and close fences,
 * after a trim. For backends that require a JSON-encoded args string
 * (OpenAI), the caller validates / normalizes via `normalizeToolArgs`
 * below.
 */
export interface ParsedToolCall {
  name: string;
  args: string;
}

/**
 * Extract every fenced `tool:<name>` block from `text`. Returns the
 * residual text (with the fences removed) and a list of parsed calls
 * in source order. Mirrors the Rust orchestrator's
 * `extract_tool_calls` so a turn that round-trips through this module
 * picks up the same calls the orchestrator would have parsed.
 *
 * Recognized form (line-based, matching Rust):
 *
 *   ```tool:<name>
 *   <body lines…>
 *   ```
 *
 * The opening fence must START a line, and the closing fence must be
 * exactly three backticks on a line by itself (after trim). Anything
 * outside such a fence is preserved verbatim in `content`.
 */
export function extractToolFences(text: string): {
  content: string;
  toolCalls: ParsedToolCall[];
} {
  const lines = text.split("\n");
  const out: ParsedToolCall[] = [];
  const keep: string[] = [];
  let bodyLines: string[] | null = null;
  let currentName = "";

  for (const line of lines) {
    if (bodyLines !== null) {
      // We're inside an open fence — looking for the close.
      if (line.trim() === "```") {
        out.push({ name: currentName, args: bodyLines.join("\n") });
        bodyLines = null;
        currentName = "";
        continue;
      }
      bodyLines.push(line);
      continue;
    }
    // Outside a fence. Look for an opening `\`\`\`tool:<name>` line.
    if (line.startsWith("```")) {
      const info = line.slice(3).trim();
      if (info.startsWith("tool:")) {
        const name = info.slice("tool:".length).trim();
        if (name.length > 0) {
          bodyLines = [];
          currentName = name;
          continue;
        }
      }
    }
    keep.push(line);
  }
  // If the model emitted an unclosed fence (rare), preserve the
  // partial body in the residual content so nothing is silently lost.
  if (bodyLines !== null) {
    keep.push("```tool:" + currentName, ...bodyLines);
  }
  return { content: keep.join("\n").trim(), toolCalls: out };
}

/**
 * Detect the orchestrator's "Tool results:" user-message envelope and
 * split it into per-call sections.
 *
 * Returns `null` when the message doesn't match the envelope (treat
 * as a regular user message), or a non-empty array of section bodies
 * (in the same order the orchestrator concatenated them) when it
 * does. Each section is the full per-tool body as written by the
 * tool's `display` field — e.g. for a successful read_file:
 *
 *   "[read_file `docs/spec.md`]\n\n# Spec\n…"
 *
 * The caller is responsible for pairing sections with the matching
 * `tool_call_id`s from the most-recent assistant message.
 */
export function parseToolResultsEnvelope(content: string): string[] | null {
  const HEADER = "Tool results:";
  if (!content.startsWith(HEADER)) {
    return null;
  }
  const tail = content.slice(HEADER.length).replace(/^\s*\n+/, "");
  if (tail.length === 0) {
    return null;
  }
  // Split on the per-section separator the orchestrator emits.
  const SEP = "\n\n---\n\n";
  const sections = tail
    .split(SEP)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  return sections.length > 0 ? sections : null;
}

/**
 * Validate / normalize the args body extracted from a fenced block.
 * Returns the args as a JSON-encoded string (what OpenAI's
 * `function.arguments` expects). When the body isn't valid JSON the
 * fallback is `{"raw": "<body>"}` — preserves the data so the
 * downstream tool can still surface a clear "bad arg shape" error,
 * vs. silently dropping a malformed call.
 */
export function normalizeToolArgsForOpenAi(args: string): string {
  const trimmed = args.trim();
  if (trimmed.length === 0) {
    return "{}";
  }
  try {
    JSON.parse(trimmed);
    return trimmed;
  } catch {
    return JSON.stringify({ raw: trimmed });
  }
}

/**
 * Mint a deterministic tool-call id. Both sides of the round-trip
 * (assistant.tool_calls + role: tool result) need to reference the
 * same id. We can't recover the model's original ids when collapsing
 * native calls into fences (the synthesis discards them); the next
 * turn we re-derive ids from the position of the fence in the
 * conversation. Stable across re-runs of the same conversation.
 */
export function makeToolCallId(messageIndex: number, callIndex: number): string {
  return `call_${messageIndex}_${callIndex}`;
}
