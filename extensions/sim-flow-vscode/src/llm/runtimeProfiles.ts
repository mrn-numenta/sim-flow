import { type LlmMessage, type RuntimeCapabilityProfile, type RuntimePreparedInput } from "./types";

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

/**
 * VS Code's built-in LM API is host-managed rather than plain
 * OpenAI-compatible HTTP, but it still benefits from explicit
 * runtime naming in diagnostics and debug surfaces.
 */
export const VSCODE_LM_RUNTIME: RuntimeCapabilityProfile = {
  id: "vscode_language_model",
  requestFormat: "vscode_language_model",
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

const RUNTIME_PROFILES: Record<string, RuntimeCapabilityProfile> = {
  [OPENAI_COMPAT_GENERIC_RUNTIME.id]: OPENAI_COMPAT_GENERIC_RUNTIME,
  [ANTHROPIC_MESSAGES_RUNTIME.id]: ANTHROPIC_MESSAGES_RUNTIME,
  [PROCESSOR_LOCAL_RUNTIME.id]: PROCESSOR_LOCAL_RUNTIME,
  [VSCODE_LM_RUNTIME.id]: VSCODE_LM_RUNTIME,
};

export const KNOWN_RUNTIME_PROFILE_IDS = Object.freeze(Object.keys(RUNTIME_PROFILES));

export function runtimeProfileById(id: string | undefined): RuntimeCapabilityProfile | undefined {
  if (!id) {
    return undefined;
  }
  return RUNTIME_PROFILES[id];
}

export function resolveRuntimeProfile(
  explicitId: string | undefined,
  fallback: RuntimeCapabilityProfile,
  allowedIds?: readonly string[],
): RuntimeCapabilityProfile {
  if (!explicitId) {
    return fallback;
  }
  const profile = runtimeProfileById(explicitId);
  if (!profile) {
    throw new Error(
      `Unknown runtime capability profile \`${explicitId}\`. Known ids: ${KNOWN_RUNTIME_PROFILE_IDS.join(", ")}.`,
    );
  }
  if (allowedIds && !allowedIds.includes(profile.id)) {
    throw new Error(
      `Runtime capability profile \`${explicitId}\` is not compatible here. Allowed ids: ${allowedIds.join(", ")}.`,
    );
  }
  return profile;
}
