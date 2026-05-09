import type {
  LlmStreamChunk,
  ModelFamilyProfile,
  NormalizedLlmChunk,
  ResponseNormalizer,
} from "./types";
import { normalizeLlmChunk } from "./types";

export const DEFAULT_RESPONSE_NORMALIZER: ResponseNormalizer = {
  id: "default",
  normalizeChunk(chunk) {
    return [normalizeLlmChunk(chunk)];
  },
};

export function createResponseNormalizerForFamily(
  family: ModelFamilyProfile,
): ResponseNormalizer {
  switch (family.thoughtMarkerStyle) {
    case "qwen-think-tag":
      return createDelimitedThinkingNormalizer("qwen_think_tag", "<think>", "</think>");
    case "kimi-think-tag":
      return createDelimitedThinkingNormalizer("kimi_think_tag", "◁think▷", "◁/think▷");
    default:
      return DEFAULT_RESPONSE_NORMALIZER;
  }
}

function createDelimitedThinkingNormalizer(
  id: string,
  openTag: string,
  closeTag: string,
): ResponseNormalizer {
  let mode: "content" | "reasoning" = "content";
  let buffer = "";

  return {
    id,
    normalizeChunk(chunk: LlmStreamChunk): NormalizedLlmChunk[] {
      if (chunk.kind === "reasoning" || chunk.kind === "tool_call") {
        return [normalizeLlmChunk(chunk)];
      }
      if (chunk.kind === "content" || chunk.kind === undefined) {
        buffer += chunk.text;
        return drainTaggedBuffer(openTag, closeTag, () => mode, (next) => {
          mode = next;
        }, () => buffer, (next) => {
          buffer = next;
        });
      }
      return [normalizeLlmChunk(chunk)];
    },
    flush(): NormalizedLlmChunk[] {
      if (buffer.length === 0) {
        return [];
      }
      const out: NormalizedLlmChunk[] = [
        {
          kind: mode,
          text: buffer,
        },
      ];
      buffer = "";
      mode = "content";
      return out;
    },
  };
}

function drainTaggedBuffer(
  openTag: string,
  closeTag: string,
  getMode: () => "content" | "reasoning",
  setMode: (mode: "content" | "reasoning") => void,
  getBuffer: () => string,
  setBuffer: (next: string) => void,
): NormalizedLlmChunk[] {
  const out: NormalizedLlmChunk[] = [];

  while (true) {
    const mode = getMode();
    const buffer = getBuffer();
    if (buffer.length === 0) {
      break;
    }

    if (mode === "content") {
      const openIndex = buffer.indexOf(openTag);
      if (openIndex === -1) {
        const suffix = longestSuffixPrefix(buffer, openTag);
        const emit = buffer.slice(0, buffer.length - suffix);
        if (emit.length > 0) {
          out.push({ kind: "content", text: emit });
        }
        setBuffer(buffer.slice(buffer.length - suffix));
        break;
      }
      if (openIndex > 0) {
        out.push({ kind: "content", text: buffer.slice(0, openIndex) });
      }
      setBuffer(buffer.slice(openIndex + openTag.length));
      setMode("reasoning");
      continue;
    }

    const closeIndex = buffer.indexOf(closeTag);
    if (closeIndex === -1) {
      const suffix = longestSuffixPrefix(buffer, closeTag);
      const emit = buffer.slice(0, buffer.length - suffix);
      if (emit.length > 0) {
        out.push({ kind: "reasoning", text: emit });
      }
      setBuffer(buffer.slice(buffer.length - suffix));
      break;
    }
    if (closeIndex > 0) {
      out.push({ kind: "reasoning", text: buffer.slice(0, closeIndex) });
    }
    setBuffer(buffer.slice(closeIndex + closeTag.length));
    setMode("content");
  }

  return out;
}

function longestSuffixPrefix(value: string, marker: string): number {
  const max = Math.min(value.length, marker.length - 1);
  for (let len = max; len > 0; len -= 1) {
    if (value.endsWith(marker.slice(0, len))) {
      return len;
    }
  }
  return 0;
}
