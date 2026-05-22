// Cross-context API-key resolution for the extension. Mirrors the
// Rust-side `__internal/keys.rs` resolver so the same `credentials.toml`
// works for both the CLI and the extension.
//
// Resolution order (first non-empty wins):
//
//   1. Provider environment variable (`ANTHROPIC_API_KEY`,
//      `OPENAI_API_KEY`, ...).
//   2. `<config>/sim-flow/credentials.toml` -- shared with `sim-flow`
//      CLI. Created and edited by `sim-flow keys set <provider>` or
//      by the extension's "Set API Key" command when the user picks
//      "Shared with CLI".
//   3. VS Code SecretStorage entry (`sim-flow.<provider>.apiKey`).
//      Extension-only, OS-keychain backed. The CLI cannot read this
//      so a key only stored here works inside VS Code.
//
// Returns `{ key, source }` so the dashboard / set-key UI can tell
// the user where their currently-effective key lives without
// surfacing the value itself.

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import { parse as parseToml } from "smol-toml";

import type { SecretStorage } from "./types";

export type ProviderId = "anthropic" | "openai" | "ollama" | "lmstudio";

export const ALL_PROVIDERS: readonly ProviderId[] = ["anthropic", "lmstudio", "ollama", "openai"];

/**
 * Wire-stable strings for `KeySource`. Match the Rust-side
 * `SOURCE_CODE_*` constants in `__internal/keys.rs` byte-for-byte
 * so the `keys list --json` output and any tooling that consumes it
 * agrees on the spelling.
 */
export const SOURCE_CODE_ENV = "env";
export const SOURCE_CODE_CONFIG_FILE = "config-file";
export const SOURCE_CODE_SECRET_STORAGE = "secret-storage";

export type KeySource =
  | typeof SOURCE_CODE_ENV
  | typeof SOURCE_CODE_CONFIG_FILE
  | typeof SOURCE_CODE_SECRET_STORAGE;

export interface KeyResolution {
  key: string;
  source: KeySource;
}

export interface KeyStatus {
  provider: ProviderId;
  source: KeySource | null;
}

const CREDENTIALS_FILE_NAME = "credentials.toml";
const API_KEY_FIELD = "api_key";
const POSIX_FILE_MODE = 0o600;

/**
 * Provider env var name. Matches the SDK convention where one
 * exists; for ollama / lmstudio (no upstream convention) we mint
 * `<UPPER>_API_KEY`. Stays in sync with `Provider::env_var` on the
 * Rust side.
 */
export function envVarFor(provider: ProviderId): string {
  switch (provider) {
    case "anthropic":
      return "ANTHROPIC_API_KEY";
    case "openai":
      return "OPENAI_API_KEY";
    case "ollama":
      return "OLLAMA_API_KEY";
    case "lmstudio":
      return "LMSTUDIO_API_KEY";
  }
}

/**
 * VS Code SecretStorage key id. Pre-existed; kept stable so a
 * `secrets.store("sim-flow.anthropic.apiKey", ...)` call from any
 * version of the extension reads back here.
 */
export function secretIdFor(provider: ProviderId): string {
  return `sim-flow.${provider}.apiKey`;
}

/**
 * Cross-platform path to the shared credentials file. Matches what
 * `directories::ProjectDirs::from("", "", "sim-flow").config_dir()`
 * resolves to on the Rust side, including the platform-specific
 * `\config\` subdirectory the `directories` crate appends on
 * Windows. Discrepancies between the two would silently strand a
 * user's key in a path the other side never reads.
 *
 * Returns `null` only when neither `$XDG_CONFIG_HOME` nor `$HOME`
 * (POSIX) / `$APPDATA` (Windows) is set -- which doesn't happen on
 * any actual user environment but the type-narrows are a forcing
 * function for callers to handle the no-config-dir case gracefully.
 */
export function credentialsFilePath(): string | null {
  if (process.platform === "win32") {
    const appData = process.env["APPDATA"];
    if (!appData) {
      return null;
    }
    // The `directories` crate (v5) returns
    // `%APPDATA%\<author>\<name>\config` for `config_dir()` on
    // Windows. With our (qualifier="", organization="", app="sim-flow")
    // call, that's `%APPDATA%\sim-flow\config\`.
    return path.join(appData, "sim-flow", "config", CREDENTIALS_FILE_NAME);
  }
  if (process.platform === "darwin") {
    const home = process.env["HOME"] || os.homedir();
    if (!home) {
      return null;
    }
    return path.join(home, "Library", "Application Support", "sim-flow", CREDENTIALS_FILE_NAME);
  }
  // Linux / others: XDG_CONFIG_HOME first, then ~/.config.
  const xdg = process.env["XDG_CONFIG_HOME"];
  if (xdg && xdg.length > 0) {
    return path.join(xdg, "sim-flow", CREDENTIALS_FILE_NAME);
  }
  const home = process.env["HOME"] || os.homedir();
  if (!home) {
    return null;
  }
  return path.join(home, ".config", "sim-flow", CREDENTIALS_FILE_NAME);
}

/**
 * Parsed credentials file. Each top-level key is a provider name;
 * each value is a table containing at minimum `api_key`. Modeled
 * as `Record<string, unknown>` so unknown providers (a future
 * version's table that this binary doesn't recognize) round-trip
 * through write without being dropped.
 */
type CredentialsFile = Record<string, unknown>;

interface ProviderEntryShape {
  api_key?: string;
  [k: string]: unknown;
}

function readCredentialsFile(filePath: string): CredentialsFile | null {
  let body: string;
  try {
    body = fs.readFileSync(filePath, "utf8");
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") {
      return {};
    }
    // Real read failure (permission, IO). Surface but treat as
    // "absent" -- consistent with the Rust resolver, which warns
    // and returns None rather than blocking the env-var fallback.
    console.error(
      `sim-flow: could not read credentials file at ${filePath}: ${(err as Error).message}`,
    );
    return null;
  }
  try {
    const parsed = parseToml(body) as unknown;
    if (!parsed || typeof parsed !== "object") {
      return {};
    }
    return parsed as CredentialsFile;
  } catch (err) {
    console.error(
      `sim-flow: ignoring unreadable credentials file ${filePath}: ${(err as Error).message}`,
    );
    return null;
  }
}

/**
 * Serialize a credentials object to TOML. We can't use smol-toml
 * for this (parser-only) and we don't want a second TOML dep just
 * for writing. Hand-rolled but TOML-correct: basic strings escape
 * `\\`, `\"`, the named control chars (`\b\t\n\f\r`), and any other
 * control byte via `\uXXXX`. Non-ASCII passes through verbatim,
 * which matches TOML's basic-string spec.
 *
 * Unknown providers / extra fields per provider are preserved -- we
 * iterate every top-level key, not just the known providers.
 */
function serializeCredentials(file: CredentialsFile): string {
  // Stable provider ordering for known ones; unknowns appended in
  // their existing iteration order. Keeps diffs noise-free across
  // round-trips by the same binary version.
  const known = new Set<string>(ALL_PROVIDERS);
  const orderedNames = [
    ...ALL_PROVIDERS.filter((p) => p in file),
    ...Object.keys(file).filter((k) => !known.has(k)),
  ];
  const out: string[] = [];
  for (const name of orderedNames) {
    const entry = file[name];
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
      continue;
    }
    const fields = Object.entries(entry as Record<string, unknown>);
    if (fields.length === 0) {
      continue;
    }
    out.push(`[${tomlBareKeyOrQuoted(name)}]`);
    for (const [field, value] of fields) {
      const rendered = renderTomlValue(value);
      if (rendered === null) {
        // Skip values we can't represent (functions, undefined, …).
        continue;
      }
      out.push(`${tomlBareKeyOrQuoted(field)} = ${rendered}`);
    }
    out.push("");
  }
  return out.join("\n");
}

function tomlBareKeyOrQuoted(key: string): string {
  // TOML bare keys may contain ASCII letters, digits, underscores,
  // and dashes. Anything else needs quoting.
  if (/^[A-Za-z0-9_-]+$/.test(key)) {
    return key;
  }
  return tomlBasicString(key);
}

function renderTomlValue(value: unknown): string | null {
  if (typeof value === "string") {
    return tomlBasicString(value);
  }
  if (typeof value === "number" && Number.isFinite(value)) {
    return Number.isInteger(value) ? String(value) : String(value);
  }
  if (typeof value === "boolean") {
    return value ? "true" : "false";
  }
  // Anything else (arrays, nested tables) is unexpected for our
  // shape -- credentials.toml is flat tables-of-key=value. Unknown
  // shapes are skipped rather than guessed at.
  return null;
}

/**
 * Render a string as a TOML basic string (RFC 0.5). Escapes the
 * spec-mandated characters; passes non-ASCII through untouched
 * (TOML basic strings are UTF-8). Other control chars (< 0x20) get
 * `\uXXXX` escapes.
 */
function tomlBasicString(s: string): string {
  let out = '"';
  for (const c of s) {
    const code = c.charCodeAt(0);
    if (c === "\\") {
      out += "\\\\";
    } else if (c === '"') {
      out += '\\"';
    } else if (code === 0x08) {
      out += "\\b";
    } else if (code === 0x09) {
      out += "\\t";
    } else if (code === 0x0a) {
      out += "\\n";
    } else if (code === 0x0c) {
      out += "\\f";
    } else if (code === 0x0d) {
      out += "\\r";
    } else if (code < 0x20 || code === 0x7f) {
      out += `\\u${code.toString(16).padStart(4, "0").toUpperCase()}`;
    } else {
      out += c;
    }
  }
  out += '"';
  return out;
}

/**
 * Atomically write `body` to `filePath` under owner-only perms.
 *
 *   - POSIX: open a sibling tempfile with `O_WRONLY | O_CREAT |
 *     O_TRUNC | O_NOFOLLOW` and mode `0o600`, write, fsync, then
 *     `rename(2)` over the destination. The credentials value is
 *     never on disk under any wider mode, even briefly. The
 *     `O_NOFOLLOW` on the tempfile path is defense-in-depth: it
 *     rejects writing through a symlink that may have been planted
 *     in the directory.
 *   - Windows: a single `writeFileSync` is sufficient (NTFS ACLs
 *     are inherited from the user's profile under `%APPDATA%`).
 */
function writeAtomicOwnerOnly(filePath: string, body: string): void {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  if (process.platform === "win32") {
    fs.writeFileSync(filePath, body, "utf8");
    return;
  }
  const dir = path.dirname(filePath);
  const base = path.basename(filePath);
  const tmp = path.join(dir, `.${base}.tmp-${process.pid}`);
  // O_WRONLY | O_CREAT | O_TRUNC | O_NOFOLLOW; mode 0600 from
  // creation. fs.constants is platform-dependent; on darwin/linux
  // these flags are stable.
  const flags =
    fs.constants.O_WRONLY |
    fs.constants.O_CREAT |
    fs.constants.O_TRUNC |
    (fs.constants.O_NOFOLLOW ?? 0);
  let fd: number | null = null;
  try {
    fd = fs.openSync(tmp, flags, POSIX_FILE_MODE);
    fs.writeSync(fd, body, 0, "utf8");
    fs.fsyncSync(fd);
  } finally {
    if (fd !== null) {
      try {
        fs.closeSync(fd);
      } catch {
        // ignore; rename will fail if the close failed for real
      }
    }
  }
  try {
    fs.renameSync(tmp, filePath);
  } catch (err) {
    // Best-effort cleanup if the rename failed; don't mask the
    // original error.
    try {
      fs.unlinkSync(tmp);
    } catch {
      // ignore
    }
    throw err;
  }
}

/**
 * Walk the resolution chain for `provider` and return the first
 * non-empty value, with its source. Returns `null` when nothing is
 * configured anywhere -- the caller surfaces a "set your key" error
 * with the env-var name and `sim-flow keys set` hint.
 *
 * `secrets` is optional so tests / CLI-side code paths can call
 * this without a VS Code context. When omitted, the SecretStorage
 * branch is skipped.
 */
export async function resolveApiKey(
  provider: ProviderId,
  secrets?: SecretStorage,
): Promise<KeyResolution | null> {
  // 1. Process environment variable.
  const envValue = process.env[envVarFor(provider)];
  if (envValue !== undefined) {
    const trimmed = envValue.trim();
    if (trimmed.length > 0) {
      return { key: trimmed, source: SOURCE_CODE_ENV };
    }
  }

  // 2. Shared credentials file.
  const filePath = credentialsFilePath();
  if (filePath !== null) {
    const file = readCredentialsFile(filePath);
    if (file !== null) {
      const entry = file[provider] as ProviderEntryShape | undefined;
      const trimmed = typeof entry?.api_key === "string" ? entry.api_key.trim() : "";
      if (trimmed.length > 0) {
        return { key: trimmed, source: SOURCE_CODE_CONFIG_FILE };
      }
    }
  }

  // 3. VS Code SecretStorage.
  if (secrets) {
    const stored = await secrets.get(secretIdFor(provider));
    if (typeof stored === "string" && stored.trim().length > 0) {
      return { key: stored.trim(), source: SOURCE_CODE_SECRET_STORAGE };
    }
  }

  return null;
}

/**
 * Persist `key` into the shared credentials file. Returns the file
 * path. Used by the extension's "Set API Key" command when the
 * user picks "Shared with CLI". Atomic + owner-only on POSIX;
 * preserves unknown providers + extra fields on round-trip.
 */
export function writeApiKeyToConfigFile(provider: ProviderId, key: string): string {
  const filePath = credentialsFilePath();
  if (filePath === null) {
    throw new Error(
      "sim-flow: no usable config directory on this platform; cannot write credentials.toml",
    );
  }
  const existing = readCredentialsFile(filePath) ?? {};
  const entry = (
    existing[provider] &&
    typeof existing[provider] === "object" &&
    !Array.isArray(existing[provider])
      ? (existing[provider] as Record<string, unknown>)
      : {}
  ) as Record<string, unknown>;
  existing[provider] = { ...entry, [API_KEY_FIELD]: key.trim() };
  writeAtomicOwnerOnly(filePath, serializeCredentials(existing));
  return filePath;
}

/**
 * Remove `provider` from the credentials file. Other providers
 * (known or unknown) are preserved. Returns `true` when an entry
 * was removed, `false` when there was nothing to remove (or the
 * file doesn't exist / can't be parsed).
 */
export function clearApiKeyFromConfigFile(provider: ProviderId): boolean {
  const filePath = credentialsFilePath();
  if (filePath === null) {
    return false;
  }
  const existing = readCredentialsFile(filePath);
  if (existing === null || existing[provider] === undefined) {
    return false;
  }
  delete existing[provider];
  writeAtomicOwnerOnly(filePath, serializeCredentials(existing));
  return true;
}

/**
 * Per-provider where-is-the-key summary. Intended for diagnostic
 * surfaces (a future "Show key sources" command) -- never includes
 * the key value.
 */
export async function listKeyStatuses(secrets?: SecretStorage): Promise<KeyStatus[]> {
  const out: KeyStatus[] = [];
  for (const provider of ALL_PROVIDERS) {
    const resolved = await resolveApiKey(provider, secrets);
    out.push({ provider, source: resolved?.source ?? null });
  }
  return out;
}

/**
 * Cheap check for "is provider X configured in the shared file".
 * Returns false when the file is missing or unparseable -- which
 * is what the UI wants when summarizing existing storage.
 */
export function providerHasConfigFileEntry(provider: ProviderId): boolean {
  const filePath = credentialsFilePath();
  if (filePath === null) {
    return false;
  }
  const file = readCredentialsFile(filePath);
  if (!file) {
    return false;
  }
  const entry = file[provider] as ProviderEntryShape | undefined;
  return typeof entry?.api_key === "string" && entry.api_key.trim().length > 0;
}
