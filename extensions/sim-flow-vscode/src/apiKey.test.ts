import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Captured VS Code UI calls and stubbed return values. The tests
// reset these between cases.
let pickedQuickPick: { label?: string; target?: string } | undefined;
let inputBoxValue: string | undefined;
let pickQuickPickHistory: Array<{ items: Array<{ label: string }>; placeHolder?: string }>;
let inputBoxHistory: Array<{ placeHolder?: string; password?: boolean }>;
let infoMessages: string[];
let errorMessages: string[];
let warnMessages: string[];

let secretStore = new Map<string, string>();

// Captured calls into the keyResolver shim so we can assert on what
// the production code asked it to do.
let writeApiKeyToConfigFileCalls: Array<{ provider: string; value: string }>;
let writeApiKeyToConfigFileImpl: ((provider: string, value: string) => string) | null;
let clearApiKeyFromConfigFileCalls: string[];
let clearApiKeyFromConfigFileImpl: ((provider: string) => boolean) | null;
let providerHasConfigFileEntryValue: boolean;
let credentialsFilePathValue: string | null;

vi.mock("vscode", () => ({
  window: {
    showQuickPick: async (items: Array<{ label: string }>, opts?: { placeHolder?: string }) => {
      pickQuickPickHistory.push({ items, placeHolder: opts?.placeHolder });
      if (pickedQuickPick === undefined) {
        return undefined;
      }
      // Find the item matching the stubbed label.
      const match = items.find((i) => i.label === pickedQuickPick!.label);
      return match;
    },
    showInputBox: async (opts: { placeHolder?: string; password?: boolean } = {}) => {
      inputBoxHistory.push({ placeHolder: opts.placeHolder, password: opts.password });
      return inputBoxValue;
    },
    showInformationMessage: (msg: string) => {
      infoMessages.push(msg);
      return Promise.resolve(undefined);
    },
    showErrorMessage: (msg: string) => {
      errorMessages.push(msg);
      return Promise.resolve(undefined);
    },
    showWarningMessage: (msg: string) => {
      warnMessages.push(msg);
      return Promise.resolve(undefined);
    },
  },
}));

vi.mock("./llm/keyResolver", () => ({
  envVarFor: (provider: string) => `${provider.toUpperCase()}_API_KEY`,
  secretIdFor: (provider: string) => `sim-flow.${provider}.apiKey`,
  credentialsFilePath: () => credentialsFilePathValue,
  providerHasConfigFileEntry: () => providerHasConfigFileEntryValue,
  writeApiKeyToConfigFile: (provider: string, value: string) => {
    writeApiKeyToConfigFileCalls.push({ provider, value });
    if (writeApiKeyToConfigFileImpl) {
      return writeApiKeyToConfigFileImpl(provider, value);
    }
    return "/fake/credentials.toml";
  },
  clearApiKeyFromConfigFile: (provider: string) => {
    clearApiKeyFromConfigFileCalls.push(provider);
    if (clearApiKeyFromConfigFileImpl) {
      return clearApiKeyFromConfigFileImpl(provider);
    }
    return true;
  },
}));

const { setApiKey, clearApiKey } = await import("./apiKey");

function fakeContext(): {
  secrets: {
    get: (k: string) => Promise<string | undefined>;
    store: (k: string, v: string) => Promise<void>;
    delete: (k: string) => Promise<void>;
  };
} {
  return {
    secrets: {
      get: async (k: string) => secretStore.get(k),
      store: async (k: string, v: string) => {
        secretStore.set(k, v);
      },
      delete: async (k: string) => {
        secretStore.delete(k);
      },
    },
  };
}

beforeEach(() => {
  pickedQuickPick = undefined;
  inputBoxValue = undefined;
  pickQuickPickHistory = [];
  inputBoxHistory = [];
  infoMessages = [];
  errorMessages = [];
  warnMessages = [];
  secretStore = new Map();
  writeApiKeyToConfigFileCalls = [];
  writeApiKeyToConfigFileImpl = null;
  clearApiKeyFromConfigFileCalls = [];
  clearApiKeyFromConfigFileImpl = null;
  providerHasConfigFileEntryValue = false;
  credentialsFilePathValue = "/fake/credentials.toml";
});

afterEach(() => {
  delete process.env.ANTHROPIC_API_KEY;
  delete process.env.OPENAI_API_KEY;
});

describe("setApiKey", () => {
  it("aborts silently when no provider is picked", async () => {
    pickedQuickPick = undefined; // user dismissed the quick pick
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await setApiKey(fakeContext() as any);
    // First (only) prompt should be the provider quick pick.
    expect(pickQuickPickHistory).toHaveLength(1);
    // No subsequent input box, no writes anywhere.
    expect(inputBoxHistory).toHaveLength(0);
    expect(writeApiKeyToConfigFileCalls).toEqual([]);
    expect(secretStore.size).toBe(0);
  });

  it("aborts when the user picks a provider but cancels the storage target", async () => {
    // First call picks Anthropic; second call (target picker) returns undefined.
    const calls: number[] = [];
    pickedQuickPick = { label: "Anthropic" };
    // Patch QuickPick to return Anthropic on first call, undefined on second.
    const vscode = await import("vscode");
    const orig = vscode.window.showQuickPick;
    (vscode.window as unknown as { showQuickPick: typeof orig }).showQuickPick = (async (
      items: Array<{ label: string }>,
    ) => {
      calls.push(items.length);
      if (calls.length === 1) {
        return items.find((i) => i.label === "Anthropic");
      }
      return undefined;
    }) as typeof orig;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await setApiKey(fakeContext() as any);
    (vscode.window as unknown as { showQuickPick: typeof orig }).showQuickPick = orig;
    expect(calls).toEqual([4, 3]); // 4 providers, 3 storage options
    expect(inputBoxHistory).toHaveLength(0);
    expect(writeApiKeyToConfigFileCalls).toEqual([]);
    expect(secretStore.size).toBe(0);
  });

  it("env-only path shows the export snippet and writes nothing", async () => {
    const vscode = await import("vscode");
    const orig = vscode.window.showQuickPick;
    (vscode.window as unknown as { showQuickPick: typeof orig }).showQuickPick = (async (
      items: Array<{ label: string }>,
    ) => {
      if (items.length === 4) {
        return items.find((i) => i.label === "OpenAI");
      }
      // Storage picker -- pick env-only.
      return items.find((i) => i.label === "I'll set the env var myself");
    }) as typeof orig;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await setApiKey(fakeContext() as any);
    (vscode.window as unknown as { showQuickPick: typeof orig }).showQuickPick = orig;
    expect(inputBoxHistory).toHaveLength(0);
    expect(writeApiKeyToConfigFileCalls).toEqual([]);
    expect(secretStore.size).toBe(0);
    // Snippet shown.
    expect(infoMessages).toHaveLength(1);
    expect(infoMessages[0]).toContain("OPENAI_API_KEY");
  });

  it("shared-file path writes via keyResolver and shows the saved path", async () => {
    const vscode = await import("vscode");
    const orig = vscode.window.showQuickPick;
    (vscode.window as unknown as { showQuickPick: typeof orig }).showQuickPick = (async (
      items: Array<{ label: string }>,
    ) => {
      if (items.length === 4) {
        return items.find((i) => i.label === "Anthropic");
      }
      return items.find((i) => i.label === "Shared with CLI (recommended)");
    }) as typeof orig;
    inputBoxValue = "sk-shared-1234";
    writeApiKeyToConfigFileImpl = () => "/cfg/credentials.toml";
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await setApiKey(fakeContext() as any);
    (vscode.window as unknown as { showQuickPick: typeof orig }).showQuickPick = orig;
    expect(writeApiKeyToConfigFileCalls).toEqual([
      { provider: "anthropic", value: "sk-shared-1234" },
    ]);
    expect(secretStore.size).toBe(0);
    expect(infoMessages[0]).toContain("/cfg/credentials.toml");
  });

  it("shared-file write failure surfaces an error and does not fall through to keychain", async () => {
    const vscode = await import("vscode");
    const orig = vscode.window.showQuickPick;
    (vscode.window as unknown as { showQuickPick: typeof orig }).showQuickPick = (async (
      items: Array<{ label: string }>,
    ) => {
      if (items.length === 4) {
        return items.find((i) => i.label === "Anthropic");
      }
      return items.find((i) => i.label === "Shared with CLI (recommended)");
    }) as typeof orig;
    inputBoxValue = "sk-shared-broken";
    writeApiKeyToConfigFileImpl = () => {
      throw new Error("disk full");
    };
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await setApiKey(fakeContext() as any);
    (vscode.window as unknown as { showQuickPick: typeof orig }).showQuickPick = orig;
    expect(errorMessages).toHaveLength(1);
    expect(errorMessages[0]).toContain("disk full");
    expect(secretStore.size).toBe(0); // did NOT fall through to keychain
    expect(infoMessages).toEqual([]); // no success message
  });

  it("vscode-keychain path stores into context.secrets (trimmed) and shows the keychain note", async () => {
    const vscode = await import("vscode");
    const orig = vscode.window.showQuickPick;
    (vscode.window as unknown as { showQuickPick: typeof orig }).showQuickPick = (async (
      items: Array<{ label: string }>,
    ) => {
      if (items.length === 4) {
        return items.find((i) => i.label === "OpenAI");
      }
      return items.find((i) => i.label === "VS Code keychain (this machine only)");
    }) as typeof orig;
    inputBoxValue = "   sk-openai-zzz   "; // padded -- should be trimmed before store
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await setApiKey(fakeContext() as any);
    (vscode.window as unknown as { showQuickPick: typeof orig }).showQuickPick = orig;
    expect(writeApiKeyToConfigFileCalls).toEqual([]);
    expect(secretStore.get("sim-flow.openai.apiKey")).toBe("sk-openai-zzz");
    expect(infoMessages[0]).toContain("SecretStorage");
  });

  it("aborts when the user cancels the api key input box", async () => {
    const vscode = await import("vscode");
    const orig = vscode.window.showQuickPick;
    (vscode.window as unknown as { showQuickPick: typeof orig }).showQuickPick = (async (
      items: Array<{ label: string }>,
    ) => {
      if (items.length === 4) {
        return items.find((i) => i.label === "Anthropic");
      }
      return items.find((i) => i.label === "Shared with CLI (recommended)");
    }) as typeof orig;
    inputBoxValue = undefined; // user dismissed the input box
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await setApiKey(fakeContext() as any);
    (vscode.window as unknown as { showQuickPick: typeof orig }).showQuickPick = orig;
    expect(writeApiKeyToConfigFileCalls).toEqual([]);
    expect(secretStore.size).toBe(0);
  });

  it("input box placeholder summarizes existing configured sources", async () => {
    process.env.ANTHROPIC_API_KEY = "from-env";
    providerHasConfigFileEntryValue = true;
    secretStore.set("sim-flow.anthropic.apiKey", "from-keychain");
    const vscode = await import("vscode");
    const orig = vscode.window.showQuickPick;
    (vscode.window as unknown as { showQuickPick: typeof orig }).showQuickPick = (async (
      items: Array<{ label: string }>,
    ) => {
      if (items.length === 4) {
        return items.find((i) => i.label === "Anthropic");
      }
      return items.find((i) => i.label === "Shared with CLI (recommended)");
    }) as typeof orig;
    inputBoxValue = "sk-new";
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await setApiKey(fakeContext() as any);
    (vscode.window as unknown as { showQuickPick: typeof orig }).showQuickPick = orig;
    expect(inputBoxHistory).toHaveLength(1);
    const placeholder = inputBoxHistory[0].placeHolder ?? "";
    expect(placeholder).toContain("ANTHROPIC_API_KEY");
    expect(placeholder).toContain("credentials.toml");
    expect(placeholder).toContain("keychain");
  });
});

describe("clearApiKey", () => {
  it("aborts silently when no provider is picked", async () => {
    pickedQuickPick = undefined;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await clearApiKey(fakeContext() as any);
    expect(clearApiKeyFromConfigFileCalls).toEqual([]);
    expect(secretStore.size).toBe(0);
    expect(infoMessages).toEqual([]);
  });

  it("clears the config file AND the keychain entry and reports both", async () => {
    pickedQuickPick = { label: "Anthropic" };
    secretStore.set("sim-flow.anthropic.apiKey", "to-be-cleared");
    clearApiKeyFromConfigFileImpl = () => true;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await clearApiKey(fakeContext() as any);
    expect(clearApiKeyFromConfigFileCalls).toEqual(["anthropic"]);
    expect(secretStore.has("sim-flow.anthropic.apiKey")).toBe(false);
    expect(infoMessages).toHaveLength(1);
    expect(infoMessages[0]).toContain("removed from /fake/credentials.toml");
    expect(infoMessages[0]).toContain("removed from VS Code SecretStorage");
  });

  it("when nothing was in the file the message only mentions keychain removal", async () => {
    pickedQuickPick = { label: "OpenAI" };
    clearApiKeyFromConfigFileImpl = () => false; // nothing to remove from file
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await clearApiKey(fakeContext() as any);
    expect(infoMessages[0]).not.toContain("removed from /fake/credentials.toml");
    expect(infoMessages[0]).toContain("removed from VS Code SecretStorage");
  });

  it("mentions the env var when it is still set in this shell", async () => {
    process.env.OPENAI_API_KEY = "still-set";
    pickedQuickPick = { label: "OpenAI" };
    clearApiKeyFromConfigFileImpl = () => true;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await clearApiKey(fakeContext() as any);
    expect(infoMessages[0]).toContain("OPENAI_API_KEY is still set");
  });

  it("warns but still attempts keychain removal when the file clear throws", async () => {
    pickedQuickPick = { label: "Anthropic" };
    secretStore.set("sim-flow.anthropic.apiKey", "should-still-go-away");
    clearApiKeyFromConfigFileImpl = () => {
      throw new Error("permission denied");
    };
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await clearApiKey(fakeContext() as any);
    expect(warnMessages).toHaveLength(1);
    expect(warnMessages[0]).toContain("permission denied");
    expect(secretStore.has("sim-flow.anthropic.apiKey")).toBe(false);
    // We still surfaced a success/summary message after the warning.
    expect(infoMessages).toHaveLength(1);
  });
});
