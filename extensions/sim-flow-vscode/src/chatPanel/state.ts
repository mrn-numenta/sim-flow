import type { LlmMessage } from "../llm/types";
import { estimateTextTokens } from "../llm/tokenEstimate";

import type { ChatTranscriptEntry } from "./messages";

export interface ChatConversationState {
  transcript: ChatTranscriptEntry[];
  nextId: number;
}

export interface StoredChatConversation {
  transcript?: ChatTranscriptEntry[];
  nextId?: number;
}

export function createConversationState(
  stored?: StoredChatConversation,
): ChatConversationState {
  const transcript = Array.isArray(stored?.transcript)
    ? filterPresentationEntries(stored.transcript)
    : [];
  const nextId = Math.max(stored?.nextId ?? 1, inferNextId(transcript));
  return { transcript, nextId };
}

export function clearConversationState(): ChatConversationState {
  return { transcript: [], nextId: 1 };
}

export function appendNote(
  state: ChatConversationState,
  title: string,
  body: string,
  tone: "info" | "error" = "info",
): ChatConversationState {
  return {
    transcript: [
      ...state.transcript,
      {
        id: entryId(state.nextId),
        kind: "note",
        title,
        body,
        tone,
      },
    ],
    nextId: state.nextId + 1,
  };
}

export function appendUserPrompt(
  state: ChatConversationState,
  prompt: string,
  userMeta: string | undefined,
  assistantMeta: string | undefined,
  requestTokensEstimate?: number,
): { state: ChatConversationState; userId: string; assistantId: string } {
  const userId = entryId(state.nextId);
  const assistantId = entryId(state.nextId + 1);
  return {
    userId,
    assistantId,
    state: {
      transcript: [
        ...state.transcript,
        {
          id: userId,
          kind: "user",
          title: "You",
          body: prompt,
          meta: userMeta,
          requestTokensEstimate,
        },
        {
          id: assistantId,
          kind: "assistant",
          title: "Assistant",
          body: "",
          meta: assistantMeta,
          streaming: true,
        },
      ],
      nextId: state.nextId + 2,
    },
  };
}

export function appendAssistantPlaceholder(
  state: ChatConversationState,
  title: string,
  assistantMeta: string | undefined,
  requestTokensEstimate?: number,
): { state: ChatConversationState; assistantId: string } {
  const assistantId = entryId(state.nextId);
  return {
    assistantId,
    state: {
      transcript: [
        ...state.transcript,
        {
          id: assistantId,
          kind: "assistant",
          title,
          body: "",
          meta: assistantMeta,
          requestTokensEstimate,
          streaming: true,
        },
      ],
      nextId: state.nextId + 1,
    },
  };
}

export function appendAssistantChunk(
  state: ChatConversationState,
  assistantId: string,
  text: string,
): ChatConversationState {
  if (text.length === 0) {
    return state;
  }
  return {
    transcript: state.transcript.map((entry) => {
      if (entry.kind !== "assistant" || entry.id !== assistantId) {
        return entry;
      }
      return {
        ...entry,
        body: entry.body + text,
        responseTokensEstimate: estimateTextTokens(entry.body + text),
      };
    }),
    nextId: state.nextId,
  };
}

export function completeAssistantTurn(
  state: ChatConversationState,
  assistantId: string,
  fallbackText = "No response received.",
): ChatConversationState {
  return {
    transcript: state.transcript.map((entry) => {
      if (entry.kind !== "assistant" || entry.id !== assistantId) {
        return entry;
      }
      return {
        ...entry,
        body: entry.body.trim().length > 0 ? entry.body : fallbackText,
        streaming: false,
      };
    }),
    nextId: state.nextId,
  };
}

export function setEntryRequestTokensEstimate(
  state: ChatConversationState,
  entryId: string,
  requestTokensEstimate: number,
): ChatConversationState {
  return {
    transcript: state.transcript.map((entry) => {
      if ((entry.kind !== "assistant" && entry.kind !== "user") || entry.id !== entryId) {
        return entry;
      }
      return {
        ...entry,
        requestTokensEstimate,
      };
    }),
    nextId: state.nextId,
  };
}

export function toStoredConversation(
  state: ChatConversationState,
): StoredChatConversation {
  return {
    transcript: filterPresentationEntries(state.transcript)
      .map((entry) => {
        if (entry.kind !== "assistant") {
          return entry;
        }
        return {
          ...entry,
          body: isOrchestratorAssistantEntry(entry)
            ? stripProtocolFences(entry.body)
            : entry.body,
          streaming: false,
        };
      }),
    nextId: state.nextId,
  };
}

export function toLlmMessages(
  transcript: ChatTranscriptEntry[],
  systemPrompt?: string,
): LlmMessage[] {
  const messages: LlmMessage[] = [];
  const trimmedSystem = systemPrompt?.trim() ?? "";
  if (trimmedSystem.length > 0) {
    messages.push({ role: "system", content: trimmedSystem });
  }
  for (const entry of transcript) {
    if (entry.kind !== "assistant" && entry.kind !== "user") {
      continue;
    }
    if (entry.body.trim().length === 0) {
      continue;
    }
    messages.push({
      role: entry.kind,
      content:
        entry.kind === "assistant"
          ? isOrchestratorAssistantEntry(entry)
            ? stripProtocolFences(entry.body)
            : stripToolCallFences(entry.body)
          : entry.body,
    });
  }
  return messages;
}

export function stripToolCallFences(text: string): string {
  if (text.length === 0) {
    return text;
  }
  return normalizeToolFenceText(text).trim();
}

export function stripToolCallFencesForDisplay(text: string): string {
  return stripToolCallFences(text).replace(/\n?```tool:[\s\S]*$/, "").trim();
}

export function stripToolCallFencesForStreaming(text: string): string {
  if (text.length === 0) {
    return text;
  }
  return normalizeToolFenceText(text).replace(/\n?```tool:[\s\S]*$/, "");
}

/**
 * Strip hidden protocol blocks from orchestrator transcript entries.
 *
 * Tool-call fences are never user-facing, and artifact-write fences
 * duplicate file contents that the dashboard already surfaces via the
 * artifact list / write notifications.
 */
export function stripProtocolFences(text: string): string {
  if (text.length === 0) {
    return text;
  }
  return normalizeProtocolFenceText(text).trim();
}

export function summarizeTokenEstimates(
  transcript: ChatTranscriptEntry[],
): { input: number; output: number } {
  return transcript.reduce(
    (totals, entry) => {
      if (entry.kind === "assistant" || entry.kind === "user") {
        totals.input += entry.requestTokensEstimate ?? 0;
        totals.output += entry.responseTokensEstimate ?? 0;
      }
      return totals;
    },
    { input: 0, output: 0 },
  );
}

export function filterPresentationEntries(
  transcript: ChatTranscriptEntry[],
): ChatTranscriptEntry[] {
  return transcript.filter(
    (entry) =>
      !isLegacyPresentationNote(entry) &&
      !isHiddenOrchestratorAssistantEntry(entry),
  );
}

function entryId(id: number): string {
  return `entry-${id}`;
}

function inferNextId(transcript: ChatTranscriptEntry[]): number {
  let maxId = 0;
  for (const entry of transcript) {
    const match = /^entry-(\d+)$/.exec(entry.id);
    if (!match) {
      continue;
    }
    maxId = Math.max(maxId, Number(match[1]));
  }
  return maxId + 1;
}

function isLegacyPresentationNote(entry: ChatTranscriptEntry): boolean {
  return (
    entry.kind === "note" &&
    (entry.title === "Tool activity" || entry.title === "Artifact written")
  );
}

function normalizeToolFenceText(text: string): string {
  return text
    .replace(/(?:^|\n)```tool:[^\n]*\n[\s\S]*?\n```[ \t]*(?=\n|$)/g, "\n")
    .replace(/\n{3,}/g, "\n\n");
}

function normalizeProtocolFenceText(text: string): string {
  return normalizeToolFenceText(text)
    .replace(/(?:^|\n)```[^\s`]*[/.][^\s`]*\n[\s\S]*?\n```[ \t]*(?=\n|$)/g, "\n")
    .replace(/\n?```[^\s`]*[/.][^\s`]*[\s\S]*$/, "")
    .replace(/\n{3,}/g, "\n\n");
}

function isOrchestratorAssistantEntry(
  entry: ChatTranscriptEntry,
): boolean {
  return entry.kind === "assistant" && entry.meta === "orchestrator";
}

function isHiddenOrchestratorAssistantEntry(entry: ChatTranscriptEntry): boolean {
  return (
    entry.kind === "assistant" &&
    isOrchestratorAssistantEntry(entry) &&
    stripProtocolFences(entry.body).length === 0
  );
}
