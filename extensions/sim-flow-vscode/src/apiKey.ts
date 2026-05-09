// Prompts the user for an LLM API key and stores it in their chosen
// destination. Command ids: `sim-flow.setApiKey` / `sim-flow.clearApiKey`.
//
// Two storage destinations:
//
//   - Shared credentials file at `<config>/sim-flow/credentials.toml`.
//     Both this extension AND the `sim-flow` CLI read from this file
//     so a key set once works in both contexts. Plaintext on disk
//     under user-only `0600` permissions; same convention as
//     `~/.aws/credentials`, `~/.npmrc`, and Anthropic's own `claude`
//     CLI. Recommended default.
//   - VS Code SecretStorage (OS keychain). Encrypted at rest, but
//     only readable inside the VS Code extension host -- a key
//     stored here doesn't help when running `sim-flow auto` from a
//     terminal. Useful when the user explicitly doesn't want a
//     plaintext credentials file on disk.
//
// Resolution at read time always tries the env var first
// (`ANTHROPIC_API_KEY` etc.), then the shared file, then
// SecretStorage. See `llm/keyResolver.ts`.

import * as vscode from "vscode";

import {
  clearApiKeyFromConfigFile,
  credentialsFilePath,
  envVarFor,
  type ProviderId,
  providerHasConfigFileEntry,
  secretIdFor,
  writeApiKeyToConfigFile,
} from "./llm/keyResolver";

interface ProviderChoice {
  label: string;
  description: string;
  provider: ProviderId;
}

const PROVIDERS: readonly ProviderChoice[] = [
  {
    label: "Anthropic",
    description: "Required when sim-flow.llm.source = anthropic.",
    provider: "anthropic",
  },
  {
    label: "OpenAI",
    description: "Required when sim-flow.llm.source = openai.",
    provider: "openai",
  },
  {
    label: "Ollama (optional)",
    description:
      "Only needed if your Ollama instance is behind an auth proxy; default local install needs no key.",
    provider: "ollama",
  },
  {
    label: "LM Studio (optional)",
    description:
      "Only needed if your LM Studio server is behind an auth proxy; default local install needs no key.",
    provider: "lmstudio",
  },
];

type StoreTarget = "shared-file" | "vscode-keychain" | "env-only";

export async function setApiKey(context: vscode.ExtensionContext): Promise<void> {
  const pickedProvider = await vscode.window.showQuickPick(
    PROVIDERS.map((c) => ({ label: c.label, description: c.description })),
    { placeHolder: "Select the provider whose API key you want to set." },
  );
  if (!pickedProvider) {
    return;
  }
  const choice = PROVIDERS.find((c) => c.label === pickedProvider.label);
  if (!choice) {
    return;
  }

  const target = await pickStoreTarget(choice);
  if (target === undefined) {
    return;
  }
  if (target === "env-only") {
    showEnvVarHowto(choice.provider);
    return;
  }

  const existingMessage = await summarizeExisting(context, choice.provider);
  const value = await vscode.window.showInputBox({
    prompt: `Paste the ${choice.label} API key`,
    password: true,
    placeHolder: existingMessage,
    ignoreFocusOut: true,
    validateInput: (v) => (v.trim().length > 0 ? null : "API key cannot be empty"),
  });
  if (!value) {
    return;
  }

  if (target === "shared-file") {
    let filePath: string;
    try {
      filePath = writeApiKeyToConfigFile(choice.provider, value);
    } catch (err) {
      void vscode.window.showErrorMessage(
        `sim-flow: failed to write credentials file: ${(err as Error).message}`,
      );
      return;
    }
    void vscode.window.showInformationMessage(
      `sim-flow: ${choice.label} API key saved to ${filePath} (also readable by \`sim-flow\` CLI).`,
    );
    return;
  }

  // vscode-keychain
  await context.secrets.store(secretIdFor(choice.provider), value.trim());
  void vscode.window.showInformationMessage(
    `sim-flow: ${choice.label} API key saved to VS Code SecretStorage. Note: only the extension can read this; ` +
      `running \`sim-flow auto\` outside VS Code won't see it. Use "Shared with CLI" if you also need terminal access.`,
  );
}

export async function clearApiKey(context: vscode.ExtensionContext): Promise<void> {
  const picked = await vscode.window.showQuickPick(
    PROVIDERS.map((c) => ({ label: c.label, description: c.description })),
    { placeHolder: "Select the provider whose API key you want to clear." },
  );
  if (!picked) {
    return;
  }
  const choice = PROVIDERS.find((c) => c.label === picked.label);
  if (!choice) {
    return;
  }
  // Clear both storage destinations so the user doesn't have to do
  // two passes. The env var (if any) is left alone -- we can't edit
  // the user's shell rc.
  let removedFromFile = false;
  try {
    removedFromFile = clearApiKeyFromConfigFile(choice.provider);
  } catch (err) {
    void vscode.window.showWarningMessage(
      `sim-flow: failed to clear ${choice.label} from credentials.toml: ${(err as Error).message}`,
    );
  }
  await context.secrets.delete(secretIdFor(choice.provider));
  const detail: string[] = [];
  if (removedFromFile) {
    detail.push(`removed from ${credentialsFilePath() ?? "credentials.toml"}`);
  }
  detail.push("removed from VS Code SecretStorage (if any)");
  if (process.env[envVarFor(choice.provider)]) {
    detail.push(
      `note: ${envVarFor(choice.provider)} is still set in this shell; unset it in your shell rc to fully clear`,
    );
  }
  void vscode.window.showInformationMessage(
    `sim-flow: ${choice.label} API key cleared (${detail.join("; ")}).`,
  );
}

async function pickStoreTarget(choice: ProviderChoice): Promise<StoreTarget | undefined> {
  const filePath = credentialsFilePath();
  const items: Array<{ label: string; description: string; target: StoreTarget }> = [
    {
      label: "Shared with CLI (recommended)",
      description: filePath
        ? `Plaintext 0600 file at ${filePath}; usable by \`sim-flow\` CLI too.`
        : "Plaintext owner-only file under your platform's config dir.",
      target: "shared-file",
    },
    {
      label: "VS Code keychain (this machine only)",
      description: `OS keychain via SecretStorage; not readable by \`sim-flow\` CLI.`,
      target: "vscode-keychain",
    },
    {
      label: "I'll set the env var myself",
      description: `Show me the \`export ${envVarFor(choice.provider)}=...\` snippet.`,
      target: "env-only",
    },
  ];
  const picked = await vscode.window.showQuickPick(items, {
    placeHolder: `Where should the ${choice.label} API key be stored?`,
    ignoreFocusOut: true,
  });
  return picked?.target;
}

async function summarizeExisting(
  context: vscode.ExtensionContext,
  provider: ProviderId,
): Promise<string> {
  const present: string[] = [];
  if (process.env[envVarFor(provider)]) {
    present.push(`${envVarFor(provider)} env var`);
  }
  if (providerHasConfigFileEntry(provider)) {
    present.push("shared credentials.toml");
  }
  const inSecretStorage = await context.secrets.get(secretIdFor(provider));
  if (inSecretStorage) {
    present.push("VS Code keychain");
  }
  if (present.length === 0) {
    return "";
  }
  return `(currently configured via: ${present.join(", ")}; paste to overwrite the new target)`;
}

function showEnvVarHowto(provider: ProviderId): void {
  const varName = envVarFor(provider);
  const isWindows = process.platform === "win32";
  const snippet = isWindows
    ? `setx ${varName} "sk-..."`
    : `export ${varName}="sk-..."`;
  const target = isWindows
    ? "your User environment variables (Settings → System → Environment Variables)"
    : `your shell rc (e.g. \`~/.zshrc\` or \`~/.bashrc\`)`;
  void vscode.window.showInformationMessage(
    `sim-flow: paste this into ${target}, then reopen any terminal:\n\n  ${snippet}`,
    { modal: true },
  );
}
