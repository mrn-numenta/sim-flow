// Read/write helpers for `.sim-flow/config.toml` -- the project-scoped
// config the sim-flow orchestrator reads. Today the dashboard only
// needs to read/write the `spec_path` field, but the helpers
// preserve unknown keys so other tools (or the user) can manage the
// rest of the file without us clobbering their settings.
//
// Why TOML round-trip in TS at all? The orchestrator's pre-DM0
// ingestion hook reads `spec_path` from this file, so the dashboard
// has to be able to set it. Shelling out to a `sim-flow config set`
// CLI would also work but doesn't exist yet -- this is the smallest
// viable plumbing.

import * as path from "node:path";
import * as fs from "node:fs/promises";

import { parse as parseToml, stringify as stringifyToml } from "smol-toml";
import type { TomlTable } from "smol-toml";

const CONFIG_FILE = "config.toml";

function configPath(projectDir: string): string {
  return path.join(projectDir, ".sim-flow", CONFIG_FILE);
}

/**
 * Load `.sim-flow/config.toml` as a TOML table, returning an empty
 * table when the file doesn't exist (the orchestrator treats absent
 * config as "all defaults"). Throws on malformed TOML rather than
 * silently swallowing -- a bad config file is a user-visible problem
 * the dashboard should surface, not paper over.
 */
async function loadConfigTable(projectDir: string): Promise<TomlTable> {
  const file = configPath(projectDir);
  let raw: string;
  try {
    raw = await fs.readFile(file, "utf8");
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") {
      return {};
    }
    throw err;
  }
  const parsed = parseToml(raw);
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new Error(`${file}: top-level TOML must be a table`);
  }
  return parsed as TomlTable;
}

async function writeConfigTable(projectDir: string, table: TomlTable): Promise<void> {
  const file = configPath(projectDir);
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, stringifyToml(table), "utf8");
}

/**
 * Read the configured source-spec path. Returns `""` when no
 * spec_path is set (rather than `undefined`) so callers can compare
 * to the dashboard's text-input value directly.
 */
export async function readSpecPath(projectDir: string): Promise<string> {
  const table = await loadConfigTable(projectDir);
  const v = table["spec_path"];
  return typeof v === "string" ? v : "";
}

/**
 * Write the configured source-spec path. Empty string clears the
 * field (writing `spec_path = ""` is fine; the orchestrator treats
 * that as "no spec configured" via its `Option<String>` schema with
 * `skip_serializing_if = "Option::is_none"`). All other keys are
 * preserved untouched.
 */
export async function writeSpecPath(projectDir: string, specPath: string): Promise<void> {
  const table = await loadConfigTable(projectDir);
  if (specPath.length === 0) {
    delete table["spec_path"];
  } else {
    table["spec_path"] = specPath;
  }
  await writeConfigTable(projectDir, table);
}

/**
 * DM3c coverage acceptance criteria mirror of the Rust
 * `CoverageSettings` struct. The dashboard reads these to seed the
 * Settings panel, and writes back when the user edits either field.
 */
export interface CoverageSettings {
  /** 0..=100. Stored verbatim; `writeCoverageSettings` clamps. */
  thresholdPct: number;
  /** "module": every module hits the bar. "total": only the project total does. */
  level: "module" | "total";
}

export const COVERAGE_DEFAULTS: CoverageSettings = {
  thresholdPct: 90,
  level: "total",
};

/**
 * Read `[coverage]` from `.sim-flow/config.toml`. Returns the
 * defaults when the file or section is missing -- the orchestrator
 * does the same on its side, so the dashboard reads the same value
 * the agent will eventually see.
 */
export async function readCoverageSettings(projectDir: string): Promise<CoverageSettings> {
  const table = await loadConfigTable(projectDir);
  const section = table["coverage"];
  if (typeof section !== "object" || section === null || Array.isArray(section)) {
    return { ...COVERAGE_DEFAULTS };
  }
  const sectionTable = section as TomlTable;
  const rawPct = sectionTable["threshold_pct"];
  const rawLevel = sectionTable["level"];
  const thresholdPct =
    typeof rawPct === "number" && Number.isFinite(rawPct)
      ? rawPct
      : COVERAGE_DEFAULTS.thresholdPct;
  const level =
    rawLevel === "module" || rawLevel === "total" ? rawLevel : COVERAGE_DEFAULTS.level;
  return { thresholdPct, level };
}

/**
 * Write `[coverage]` back to `.sim-flow/config.toml`, clamping the
 * threshold percentage to `[0, 100]` so a slipped keystroke in the
 * Settings panel can't write `9000` to disk. All other keys are
 * preserved untouched.
 */
export async function writeCoverageSettings(
  projectDir: string,
  settings: CoverageSettings,
): Promise<CoverageSettings> {
  const clampedPct = Math.max(0, Math.min(100, settings.thresholdPct));
  const table = await loadConfigTable(projectDir);
  table["coverage"] = {
    threshold_pct: clampedPct,
    level: settings.level,
  };
  await writeConfigTable(projectDir, table);
  return { thresholdPct: clampedPct, level: settings.level };
}
