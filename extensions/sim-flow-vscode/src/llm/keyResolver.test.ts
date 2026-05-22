import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  ALL_PROVIDERS,
  clearApiKeyFromConfigFile,
  credentialsFilePath,
  envVarFor,
  listKeyStatuses,
  providerHasConfigFileEntry,
  resolveApiKey,
  secretIdFor,
  SOURCE_CODE_CONFIG_FILE,
  SOURCE_CODE_ENV,
  SOURCE_CODE_SECRET_STORAGE,
  writeApiKeyToConfigFile,
} from "./keyResolver";

import type { SecretStorage } from "./types";

class FakeSecrets implements SecretStorage {
  private readonly entries = new Map<string, string>();
  set(key: string, value: string): void {
    this.entries.set(key, value);
  }
  async get(key: string): Promise<string | undefined> {
    return this.entries.get(key);
  }
  async store(key: string, value: string): Promise<void> {
    this.entries.set(key, value);
  }
  async delete(key: string): Promise<void> {
    this.entries.delete(key);
  }
}

let tmpRoot: string;
const savedEnv: Record<string, string | undefined> = {};

function captureEnv(...keys: string[]): void {
  for (const k of keys) {
    savedEnv[k] = process.env[k];
  }
}

function restoreEnv(): void {
  for (const [k, v] of Object.entries(savedEnv)) {
    if (v === undefined) {
      delete process.env[k];
    } else {
      process.env[k] = v;
    }
  }
  for (const k of Object.keys(savedEnv)) {
    delete savedEnv[k];
  }
}

beforeEach(() => {
  tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), "sim-flow-keyres-"));
  captureEnv(
    "XDG_CONFIG_HOME",
    "HOME",
    "APPDATA",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "OLLAMA_API_KEY",
    "LMSTUDIO_API_KEY",
  );
  process.env["XDG_CONFIG_HOME"] = tmpRoot;
  process.env["HOME"] = tmpRoot;
  process.env["APPDATA"] = tmpRoot;
  for (const p of ALL_PROVIDERS) {
    delete process.env[envVarFor(p)];
  }
});

afterEach(() => {
  restoreEnv();
  fs.rmSync(tmpRoot, { recursive: true, force: true });
});

describe("credentialsFilePath: per-platform layout", () => {
  it("returns a path under the platform's config dir", () => {
    const p = credentialsFilePath();
    expect(p).not.toBeNull();
    expect(p!.endsWith("credentials.toml")).toBe(true);
  });

  it("on darwin, lives under Library/Application Support/sim-flow", () => {
    if (process.platform !== "darwin") {
      return;
    }
    const p = credentialsFilePath()!;
    expect(p).toContain(path.join("Library", "Application Support", "sim-flow"));
  });

  it("on linux, honors XDG_CONFIG_HOME", () => {
    if (process.platform === "win32" || process.platform === "darwin") {
      return;
    }
    const p = credentialsFilePath()!;
    expect(p.startsWith(path.join(tmpRoot, "sim-flow"))).toBe(true);
  });

  it("on win32, includes the directories-crate `config` subdir", () => {
    // Simulate a win32 path layout regardless of host platform by
    // checking the function's win32 branch via process.platform.
    // We can't override process.platform here, but the algorithm is
    // simple: on win32 the path is `%APPDATA%\sim-flow\config\credentials.toml`.
    // Asserted in the Rust matching test below; here we just verify
    // the function's branching honors APPDATA when on win32.
    if (process.platform !== "win32") {
      return;
    }
    const p = credentialsFilePath()!;
    expect(p).toContain(path.join("sim-flow", "config", "credentials.toml"));
  });
});

describe("resolveApiKey: env > config-file > secret-storage", () => {
  it("env var wins over config file and SecretStorage", async () => {
    writeApiKeyToConfigFile("anthropic", "from-config");
    const secrets = new FakeSecrets();
    secrets.set(secretIdFor("anthropic"), "from-secrets");
    process.env["ANTHROPIC_API_KEY"] = "from-env";

    const r = await resolveApiKey("anthropic", secrets);
    expect(r).toEqual({ key: "from-env", source: SOURCE_CODE_ENV });
  });

  it("config file wins over SecretStorage when env is unset", async () => {
    writeApiKeyToConfigFile("anthropic", "from-config");
    const secrets = new FakeSecrets();
    secrets.set(secretIdFor("anthropic"), "from-secrets");

    const r = await resolveApiKey("anthropic", secrets);
    expect(r).toEqual({ key: "from-config", source: SOURCE_CODE_CONFIG_FILE });
  });

  it("falls back to SecretStorage when env and file are empty", async () => {
    const secrets = new FakeSecrets();
    secrets.set(secretIdFor("anthropic"), "from-secrets");
    const r = await resolveApiKey("anthropic", secrets);
    expect(r).toEqual({ key: "from-secrets", source: SOURCE_CODE_SECRET_STORAGE });
  });

  it("returns null when nothing is set anywhere", async () => {
    const secrets = new FakeSecrets();
    const r = await resolveApiKey("anthropic", secrets);
    expect(r).toBeNull();
  });

  it("treats whitespace-only env as unset and falls through", async () => {
    process.env["ANTHROPIC_API_KEY"] = "   ";
    writeApiKeyToConfigFile("anthropic", "from-config");
    const r = await resolveApiKey("anthropic");
    expect(r).toEqual({ key: "from-config", source: SOURCE_CODE_CONFIG_FILE });
  });

  it("works without a SecretStorage (CLI-side / tests)", async () => {
    process.env["OPENAI_API_KEY"] = "from-env";
    const r = await resolveApiKey("openai");
    expect(r).toEqual({ key: "from-env", source: SOURCE_CODE_ENV });
  });

  it("a malformed credentials file does not shadow the env var", async () => {
    const filePath = credentialsFilePath()!;
    fs.mkdirSync(path.dirname(filePath), { recursive: true });
    fs.writeFileSync(filePath, "this is { not valid toml\n", "utf8");
    process.env["ANTHROPIC_API_KEY"] = "from-env";
    const r = await resolveApiKey("anthropic");
    expect(r).toEqual({ key: "from-env", source: SOURCE_CODE_ENV });
  });

  it("a malformed credentials file resolves as null when no env is set", async () => {
    const filePath = credentialsFilePath()!;
    fs.mkdirSync(path.dirname(filePath), { recursive: true });
    fs.writeFileSync(filePath, "this is { not valid toml\n", "utf8");
    const r = await resolveApiKey("anthropic");
    expect(r).toBeNull();
  });
});

describe("write/clear round-trip", () => {
  it("writeApiKeyToConfigFile creates the file, returns the path, and is readable", async () => {
    const filePath = writeApiKeyToConfigFile("anthropic", "secret-1");
    expect(fs.existsSync(filePath)).toBe(true);
    const r = await resolveApiKey("anthropic");
    expect(r?.key).toBe("secret-1");
  });

  it("preserves other providers when one is updated", async () => {
    writeApiKeyToConfigFile("anthropic", "ant-1");
    writeApiKeyToConfigFile("openai", "oai-1");
    writeApiKeyToConfigFile("anthropic", "ant-2");
    expect((await resolveApiKey("anthropic"))?.key).toBe("ant-2");
    expect((await resolveApiKey("openai"))?.key).toBe("oai-1");
  });

  it("preserves unknown provider tables across rewrites (forward compat)", () => {
    // Simulate a future binary's `[vertex]` table that this binary
    // doesn't recognize. After we set anthropic and clear it, the
    // unknown table must still be intact.
    const filePath = credentialsFilePath()!;
    fs.mkdirSync(path.dirname(filePath), { recursive: true });
    fs.writeFileSync(filePath, '[vertex]\napi_key = "vx-future"\nproject = "my-proj"\n', "utf8");

    writeApiKeyToConfigFile("anthropic", "ant-1");
    let body = fs.readFileSync(filePath, "utf8");
    expect(body).toContain("[vertex]");
    expect(body).toContain("vx-future");
    expect(body).toContain("my-proj");

    clearApiKeyFromConfigFile("anthropic");
    body = fs.readFileSync(filePath, "utf8");
    expect(body).not.toContain("[anthropic]");
    expect(body).toContain("[vertex]");
    expect(body).toContain("vx-future");
  });

  it("clearApiKeyFromConfigFile removes one provider, idempotent thereafter", async () => {
    writeApiKeyToConfigFile("anthropic", "to-be-removed");
    expect(clearApiKeyFromConfigFile("anthropic")).toBe(true);
    expect(await resolveApiKey("anthropic")).toBeNull();
    expect(clearApiKeyFromConfigFile("anthropic")).toBe(false);
  });

  it("sets 0600 permissions on POSIX", () => {
    if (process.platform === "win32") {
      return;
    }
    const filePath = writeApiKeyToConfigFile("anthropic", "secret");
    const mode = fs.statSync(filePath).mode & 0o777;
    expect(mode).toBe(0o600);
  });

  it("does not leave a tempfile sibling after a successful write (POSIX)", () => {
    if (process.platform === "win32") {
      return;
    }
    const filePath = writeApiKeyToConfigFile("anthropic", "secret");
    const dir = path.dirname(filePath);
    const stragglers = fs.readdirSync(dir).filter((n) => n.includes(".tmp-"));
    expect(stragglers).toEqual([]);
  });

  it("escapes strings as TOML basic strings (quote, backslash, control chars)", () => {
    // The hand-rolled writer needs to emit valid TOML for keys with
    // characters that JSON-string-escape would disagree on. Use a
    // contrived value with backslash, quote, and a control byte.
    writeApiKeyToConfigFile("openai", 'sk-"with\\quote');
    const filePath = credentialsFilePath()!;
    const body = fs.readFileSync(filePath, "utf8");
    // Quote and backslash escaped as TOML expects.
    expect(body).toContain('\\"');
    expect(body).toContain("\\\\");
    // Control byte escaped as .
    expect(body).toContain("\\u0001");
  });
});

describe("listKeyStatuses + providerHasConfigFileEntry", () => {
  it("reports per-provider source without revealing the key value", async () => {
    process.env["ANTHROPIC_API_KEY"] = "from-env";
    writeApiKeyToConfigFile("openai", "from-config");
    const secrets = new FakeSecrets();
    secrets.set(secretIdFor("ollama"), "from-secrets");

    const statuses = await listKeyStatuses(secrets);
    const get = (p: string) => statuses.find((s) => s.provider === p)!;
    expect(get("anthropic").source).toBe(SOURCE_CODE_ENV);
    expect(get("openai").source).toBe(SOURCE_CODE_CONFIG_FILE);
    expect(get("ollama").source).toBe(SOURCE_CODE_SECRET_STORAGE);
    expect(get("lmstudio").source).toBeNull();
  });

  it("providerHasConfigFileEntry returns false on absent / parse-fail / missing entry", () => {
    expect(providerHasConfigFileEntry("anthropic")).toBe(false);
    writeApiKeyToConfigFile("anthropic", "secret");
    expect(providerHasConfigFileEntry("anthropic")).toBe(true);
    expect(providerHasConfigFileEntry("openai")).toBe(false);

    // Corrupt the file: providerHasConfigFileEntry must NOT throw.
    fs.writeFileSync(credentialsFilePath()!, "not toml", "utf8");
    expect(providerHasConfigFileEntry("anthropic")).toBe(false);
  });

  it("does not false-positive on a comment containing [anthropic]", () => {
    // Regression: the prior implementation substring-matched the
    // raw body; a comment alone must not be enough to claim the
    // provider is configured.
    const filePath = credentialsFilePath()!;
    fs.mkdirSync(path.dirname(filePath), { recursive: true });
    fs.writeFileSync(filePath, "# [anthropic] is what we used to use\n", "utf8");
    expect(providerHasConfigFileEntry("anthropic")).toBe(false);
  });
});
