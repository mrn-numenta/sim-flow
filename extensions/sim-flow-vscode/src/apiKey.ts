// Prompts the user for an LLM API key and stores it in VS Code's
// SecretStorage. Command id: `sim-flow.setApiKey`.

import * as vscode from "vscode";

import { ANTHROPIC_KEY_ID, LMSTUDIO_KEY_ID, OLLAMA_KEY_ID, OPENAI_KEY_ID } from "./llm";

interface KeyChoice {
  label: string;
  description: string;
  secretId: string;
}

const CHOICES: readonly KeyChoice[] = [
  {
    label: "Anthropic",
    description: "Used by the anthropic source for the sim-flow chat participant.",
    secretId: ANTHROPIC_KEY_ID,
  },
  {
    label: "OpenAI",
    description: "Used by the openai source for the sim-flow chat participant.",
    secretId: OPENAI_KEY_ID,
  },
  {
    label: "Ollama (optional)",
    description:
      "Only needed if your Ollama instance sits behind an auth proxy; the default local install needs no key.",
    secretId: OLLAMA_KEY_ID,
  },
  {
    label: "LM Studio (optional)",
    description:
      "Only needed if your LM Studio server is fronted by an auth proxy; local LM Studio needs no key.",
    secretId: LMSTUDIO_KEY_ID,
  },
];

export async function setApiKey(context: vscode.ExtensionContext): Promise<void> {
  const picked = await vscode.window.showQuickPick(
    CHOICES.map((c) => ({ label: c.label, description: c.description })),
    { placeHolder: "Select the provider whose API key you want to set." },
  );
  if (!picked) {
    return;
  }
  const choice = CHOICES.find((c) => c.label === picked.label);
  if (!choice) {
    return;
  }

  const existing = await context.secrets.get(choice.secretId);
  const value = await vscode.window.showInputBox({
    prompt: `Paste the ${choice.label} API key`,
    password: true,
    placeHolder: existing ? "(a key is already stored; paste to replace)" : "",
    ignoreFocusOut: true,
    validateInput: (v) => (v.trim().length > 0 ? null : "API key cannot be empty"),
  });
  if (!value) {
    return;
  }

  await context.secrets.store(choice.secretId, value.trim());
  void vscode.window.showInformationMessage(
    `sim-flow: ${choice.label} API key saved to SecretStorage as "${choice.secretId}".`,
  );
}

export async function clearApiKey(context: vscode.ExtensionContext): Promise<void> {
  const picked = await vscode.window.showQuickPick(
    CHOICES.map((c) => ({ label: c.label, description: c.description })),
    { placeHolder: "Select the provider whose API key you want to clear." },
  );
  if (!picked) {
    return;
  }
  const choice = CHOICES.find((c) => c.label === picked.label);
  if (!choice) {
    return;
  }
  await context.secrets.delete(choice.secretId);
  void vscode.window.showInformationMessage(`sim-flow: ${choice.label} API key cleared.`);
}
