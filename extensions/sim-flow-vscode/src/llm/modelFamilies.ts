import type {
  LlmAttachment,
  LlmMessage,
  ModelFamilyProfile,
  RuntimePreparedInput,
} from "./types";

export interface ModelFamilyPromptOptions {
  enableThinking?: boolean;
}

export const GENERIC_CHAT_MODEL_FAMILY: ModelFamilyProfile = {
  id: "generic_chat",
  thoughtMarkerStyle: "none",
  prefersMediaBeforeText: false,
  supportsThinkingControls: false,
  thinkingControlMode: "none",
  reasoningHistoryPolicy: "preserve-all",
};

export const GEMMA4_MODEL_FAMILY: ModelFamilyProfile = {
  id: "gemma4",
  thoughtMarkerStyle: "gemma-think-tag",
  prefersMediaBeforeText: true,
  supportsThinkingControls: true,
  thinkingControlMode: "prompt-tag",
  thinkingControlToken: "<|think|>",
  reasoningHistoryPolicy: "drop-prior-reasoning",
  defaultSampling: {
    temperature: 1.0,
    topP: 0.95,
    topK: 64,
  },
};

export const QWEN3_6_MODEL_FAMILY: ModelFamilyProfile = {
  id: "qwen3_6",
  thoughtMarkerStyle: "qwen-think-tag",
  prefersMediaBeforeText: false,
  supportsThinkingControls: true,
  thinkingControlMode: "runtime-flag",
  reasoningHistoryPolicy: "runtime-controlled",
};

export const KIMI_VL_THINKING_MODEL_FAMILY: ModelFamilyProfile = {
  id: "kimi_vl_thinking",
  thoughtMarkerStyle: "kimi-think-tag",
  prefersMediaBeforeText: true,
  supportsThinkingControls: false,
  thinkingControlMode: "none",
  reasoningHistoryPolicy: "preserve-all",
  defaultSampling: {
    temperature: 0.8,
  },
};

export const CLAUDE_MESSAGES_MODEL_FAMILY: ModelFamilyProfile = {
  id: "claude_messages",
  thoughtMarkerStyle: "anthropic-thinking-blocks",
  prefersMediaBeforeText: false,
  supportsThinkingControls: true,
  thinkingControlMode: "runtime-flag",
  reasoningHistoryPolicy: "drop-prior-reasoning",
};

const MODEL_FAMILIES: Record<string, ModelFamilyProfile> = {
  [GENERIC_CHAT_MODEL_FAMILY.id]: GENERIC_CHAT_MODEL_FAMILY,
  [GEMMA4_MODEL_FAMILY.id]: GEMMA4_MODEL_FAMILY,
  [QWEN3_6_MODEL_FAMILY.id]: QWEN3_6_MODEL_FAMILY,
  [KIMI_VL_THINKING_MODEL_FAMILY.id]: KIMI_VL_THINKING_MODEL_FAMILY,
  [CLAUDE_MESSAGES_MODEL_FAMILY.id]: CLAUDE_MESSAGES_MODEL_FAMILY,
};

export function modelFamilyById(id: string | undefined): ModelFamilyProfile | undefined {
  if (!id) {
    return undefined;
  }
  return MODEL_FAMILIES[id];
}

export function inferModelFamily(model: string | undefined): ModelFamilyProfile {
  const lowered = model?.trim().toLowerCase();
  if (!lowered) {
    return GENERIC_CHAT_MODEL_FAMILY;
  }
  if (lowered.includes("gemma-4")) {
    return GEMMA4_MODEL_FAMILY;
  }
  if (lowered.includes("qwen3.6") || lowered.includes("qwen-3.6") || lowered.includes("qwen3-6")) {
    return QWEN3_6_MODEL_FAMILY;
  }
  if (lowered.includes("kimi-vl")) {
    return KIMI_VL_THINKING_MODEL_FAMILY;
  }
  if (lowered.includes("claude")) {
    return CLAUDE_MESSAGES_MODEL_FAMILY;
  }
  return GENERIC_CHAT_MODEL_FAMILY;
}

export function resolveModelFamily(
  explicitId: string | undefined,
  model: string | undefined,
): ModelFamilyProfile {
  return modelFamilyById(explicitId) ?? inferModelFamily(model);
}

/**
 * Apply family-level prompt shaping that is independent of transport.
 * This remains opt-in so Phase 10 can add explicit configuration for
 * enabling thinking controls without silently changing defaults.
 */
export function applyModelFamilyPromptPolicy(
  input: RuntimePreparedInput,
  family: ModelFamilyProfile,
  options: ModelFamilyPromptOptions = {},
): RuntimePreparedInput {
  if (
    !options.enableThinking ||
    family.thinkingControlMode !== "prompt-tag" ||
    !family.thinkingControlToken
  ) {
    return input;
  }

  if (input.system !== undefined) {
    return {
      ...input,
      system: `${family.thinkingControlToken}\n${input.system}`,
    };
  }

  const first = input.messages[0];
  if (first?.role === "system") {
    return {
      ...input,
      messages: [
        {
          ...first,
          content: `${family.thinkingControlToken}\n${first.content}`,
        },
        ...input.messages.slice(1),
      ],
    };
  }

  return {
    ...input,
    messages: [{ role: "system", content: family.thinkingControlToken }, ...input.messages],
  };
}

export function orderAttachmentsByFamily(
  family: ModelFamilyProfile,
  text: string,
  attachments: LlmAttachment[],
): Array<{ kind: "text"; text: string } | { kind: "attachment"; attachment: LlmAttachment }> {
  const textParts =
    text.length > 0 ? ([{ kind: "text", text }] as const) : ([] as const);
  const attachmentParts = attachments.map(
    (attachment) => ({ kind: "attachment", attachment }) as const,
  );
  return family.prefersMediaBeforeText
    ? [...attachmentParts, ...textParts]
    : [...textParts, ...attachmentParts];
}

export function applyReasoningHistoryPolicy(
  messages: LlmMessage[],
  family: ModelFamilyProfile,
): LlmMessage[] {
  if (family.reasoningHistoryPolicy !== "drop-prior-reasoning") {
    return messages;
  }
  return messages.map((message) => ({
    ...message,
    content: stripKnownReasoningMarkers(message.content, family),
  }));
}

function stripKnownReasoningMarkers(content: string, family: ModelFamilyProfile): string {
  switch (family.thoughtMarkerStyle) {
    case "qwen-think-tag":
      return content.replace(/<think>[\s\S]*?<\/think>/g, "").trim();
    case "kimi-think-tag":
      return content.replace(/◁think▷[\s\S]*?◁\/think▷/g, "").trim();
    default:
      return content;
  }
}
