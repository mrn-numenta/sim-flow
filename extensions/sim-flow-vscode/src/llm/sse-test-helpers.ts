// Helpers shared by openai/ollama/lmstudio backend tests for building
// fake Server-Sent Events responses. Lives under `src/` so vitest's
// path config picks it up; not exported from the package entry.

/** A single OpenAI-compatible streaming chunk. */
export type SseEvent = Record<string, unknown>;

/**
 * Build a `Response` whose body is an SSE stream of `events`,
 * terminated by `data: [DONE]\n\n`. Use this in fakeFetch handlers
 * to simulate an OpenAI Chat Completions streaming response.
 */
export function sseResponse(events: SseEvent[]): Response {
  const body = events.map((e) => `data: ${JSON.stringify(e)}\n\n`).join("") + "data: [DONE]\n\n";
  return new Response(body, {
    status: 200,
    headers: { "content-type": "text/event-stream" },
  });
}

/** Convenience: build a single content delta event. */
export function contentDelta(content: string): SseEvent {
  return { choices: [{ index: 0, delta: { content } }] };
}

/** Convenience: build the trailing finish_reason marker. */
export function finishReason(reason: "stop" | "tool_calls" = "stop"): SseEvent {
  return { choices: [{ index: 0, delta: {}, finish_reason: reason }] };
}

/**
 * Convenience: streaming response that emits `text` as one content
 * delta and a stop marker. Equivalent to a non-streaming response
 * with `choices[0].message.content = text` for tests that don't
 * care about chunk boundaries.
 */
export function sseSingleResponse(text: string): Response {
  return sseResponse([contentDelta(text), finishReason("stop")]);
}
