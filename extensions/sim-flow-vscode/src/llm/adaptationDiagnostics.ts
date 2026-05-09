import type { LlmAdaptationProfile, LlmAdaptationSummary } from "./types";

export function summarizeAdaptation(
  backend: string,
  adaptation: LlmAdaptationProfile,
): LlmAdaptationSummary {
  return {
    backend,
    runtimeId: adaptation.runtime.id,
    modelFamilyId: adaptation.modelFamily.id,
    requestFormat: adaptation.runtime.requestFormat,
    systemPromptMode: adaptation.runtime.systemPromptMode,
    credentialPolicy: adaptation.runtime.credentialPolicy,
    supportsStructuredReasoning: adaptation.runtime.supportsStructuredReasoning ?? false,
    supportsStructuredToolCalls: adaptation.runtime.supportsStructuredToolCalls ?? false,
    supportsThinkingControls: adaptation.modelFamily.supportsThinkingControls ?? false,
  };
}

export function formatAdaptationSummary(summary: LlmAdaptationSummary): string {
  const parts = [
    `backend=${summary.backend}`,
    `runtime=${summary.runtimeId}`,
    `family=${summary.modelFamilyId}`,
  ];
  if (summary.requestFormat) {
    parts.push(`request=${summary.requestFormat}`);
  }
  if (summary.systemPromptMode) {
    parts.push(`system=${summary.systemPromptMode}`);
  }
  if (summary.credentialPolicy) {
    parts.push(`credentials=${summary.credentialPolicy}`);
  }
  parts.push(`structured-reasoning=${summary.supportsStructuredReasoning ? "yes" : "no"}`);
  parts.push(`structured-tools=${summary.supportsStructuredToolCalls ? "yes" : "no"}`);
  parts.push(`thinking-controls=${summary.supportsThinkingControls ? "yes" : "no"}`);
  return parts.join(", ");
}
