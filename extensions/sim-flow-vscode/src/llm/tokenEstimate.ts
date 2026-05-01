import type { LlmAttachment, LlmMessage } from "./types";

const CHARS_PER_TOKEN_ESTIMATE = 4;
const MESSAGE_OVERHEAD_TOKENS = 4;
const ATTACHMENT_OVERHEAD_TOKENS = 12;

export function estimateTextTokens(text: string): number {
  const normalized = text.replace(/\r\n?/g, "\n").trim();
  if (normalized.length === 0) {
    return 0;
  }
  const byChars = Math.ceil(normalized.length / CHARS_PER_TOKEN_ESTIMATE);
  const words = normalized.split(/\s+/).filter((part) => part.length > 0).length;
  const byWords = Math.ceil(words * 0.75);
  return Math.max(1, byChars, byWords);
}

export function estimateMessageTokens(message: LlmMessage): number {
  let total = MESSAGE_OVERHEAD_TOKENS + estimateTextTokens(message.role) + estimateTextTokens(message.content);
  for (const attachment of message.attachments ?? []) {
    total += estimateAttachmentTokens(attachment);
  }
  return total;
}

export function estimateMessagesTokens(messages: LlmMessage[]): number {
  return messages.reduce((sum, message) => sum + estimateMessageTokens(message), 0);
}

function estimateAttachmentTokens(attachment: LlmAttachment): number {
  return (
    ATTACHMENT_OVERHEAD_TOKENS +
    estimateTextTokens(attachment.mime) +
    estimateTextTokens(attachment.source ?? "") +
    Math.ceil(attachment.data.length / CHARS_PER_TOKEN_ESTIMATE)
  );
}
