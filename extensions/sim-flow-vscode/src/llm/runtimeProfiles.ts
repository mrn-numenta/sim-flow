import {
  normalizeLlmChunk,
  type LlmMessage,
  type ModelFamilyProfile,
  type ResponseNormalizer,
  type RuntimeCapabilityProfile,
  type RuntimePreparedInput,
} from "./types";

/**
 * Collapse a run of leading `role: "system"` messages into one,
 * joining their bodies with a blank-line separator. Subsequent
 * non-leading system messages (rare; defensive) are left in place
 * so we don't mask higher-up bugs by silently rewriting them.
 *
 * Why we need this: the orchestrator emits up to five separate
 * system messages at the head of every prompt (combined-system,
 * tool-notice, spec TOC, framework-API TOC, session-inputs
 * stable + volatile) so vLLM's prefix cache can reuse the long
 * stable prefix across milestone advances and critique retries.
 * That split works on most servers, but the default vLLM chat
 * template enforces "exactly one system message at the beginning"
 * and rejects the request. Merging on the wire keeps the prefix-
 * cache benefit while satisfying strict OpenAI-compatible servers.
 */
export function mergeLeadingSystemMessages(messages: LlmMessage[]): LlmMessage[] {
  let leading = 0;
  while (leading < messages.length && messages[leading].role === "system") {
    leading += 1;
  }
  if (leading <= 1) {
    return messages;
  }
  const head = messages.slice(0, leading);
  const mergedAttachments = head.flatMap((m) => m.attachments ?? []);
  const mergedContent = head.map((m) => m.content).join("\n\n");
  const tail = messages.slice(leading);
  return [
    {
      role: "system",
      content: mergedContent,
      ...(mergedAttachments.length > 0 ? { attachments: mergedAttachments } : {}),
    },
    ...tail,
  ];
}

/**
 * Anthropic's Messages API lifts system prompt text into a dedicated
 * request field and leaves only user/assistant turns in the message
 * array. All system messages are collapsed in-order so the runtime
 * owns the split consistently for every caller.
 */
export function prepareAnthropicMessages(messages: LlmMessage[]): RuntimePreparedInput {
  const system = messages
    .filter((m) => m.role === "system")
    .map((m) => m.content)
    .join("\n\n");
  return {
    system: system.length > 0 ? system : undefined,
    messages: messages.filter((m) => m.role !== "system"),
  };
}

/** Default response normalizer until model-family specializations land. */
export const DEFAULT_RESPONSE_NORMALIZER: ResponseNormalizer = {
  id: "default",
  normalizeChunk: normalizeLlmChunk,
};

/**
 * Generic placeholder family for backends that have runtime profiles
 * but do not yet opt into model-family specialization. Phase 10 M3
 * replaces this with concrete Gemma/Qwen/Kimi/Claude families.
 */
export const GENERIC_MODEL_FAMILY: ModelFamilyProfile = {
  id: "generic_chat",
  thoughtMarkerStyle: "none",
  supportsThinkingControls: false,
};

export const OPENAI_COMPAT_GENERIC_RUNTIME: RuntimeCapabilityProfile = {
  id: "openai_compat_generic",
  requestFormat: "openai_chat_completions",
  credentialPolicy: "shared-provider-chain",
  systemPromptMode: "collapsed-leading-message",
  collapseLeadingSystemMessages: true,
  supportsStructuredReasoning: true,
  supportsStructuredToolCalls: true,
  supportsSharedCredentialChain: true,
  prepareInput(messages) {
    return { messages: mergeLeadingSystemMessages(messages) };
  },
};

export const ANTHROPIC_MESSAGES_RUNTIME: RuntimeCapabilityProfile = {
  id: "anthropic_messages",
  requestFormat: "anthropic_messages",
  credentialPolicy: "shared-provider-chain",
  systemPromptMode: "dedicated-field",
  collapseLeadingSystemMessages: false,
  supportsStructuredReasoning: true,
  supportsStructuredToolCalls: true,
  supportsSharedCredentialChain: true,
  prepareInput: prepareAnthropicMessages,
};

/**
 * Placeholder for processor-centric local inference stacks such as
 * Kimi-VL or Gemma flows driven through `AutoProcessor` rather than a
 * chat-completions API. Exposed now so the runtime layer can name the
 * category before a concrete backend lands.
 */
export const PROCESSOR_LOCAL_RUNTIME: RuntimeCapabilityProfile = {
  id: "processor_local",
  requestFormat: "processor_local",
  credentialPolicy: "host-managed",
  systemPromptMode: "message-array",
  collapseLeadingSystemMessages: false,
  supportsStructuredReasoning: false,
  supportsStructuredToolCalls: false,
  supportsSharedCredentialChain: false,
  prepareInput(messages) {
    return { messages };
  },
};
