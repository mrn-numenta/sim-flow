// Read and parse `.sim-flow/state.toml` directly, without going through
// the CLI. Intended for the dashboard's live update path where spawning
// a subprocess on every state change would be wasteful. The CLI remains
// authoritative for mutations; this module is read-only.

import { promises as fs } from "node:fs";
import * as path from "node:path";
import { parse as parseToml } from "smol-toml";

import type { Flow, Gate } from "../cli/types";
import type { FlowState } from "./types";

export type { FlowState };

/** Relative path from a project root to the state file. */
export const STATE_FILE = path.join(".sim-flow", "state.toml");

export class FlowStateParseError extends Error {
  readonly file: string;

  constructor(message: string, file: string, cause?: unknown) {
    super(message, { cause });
    this.name = "FlowStateParseError";
    this.file = file;
  }
}

/** Resolve the absolute path to a project's state.toml. */
export function stateFilePath(projectDir: string): string {
  return path.join(projectDir, STATE_FILE);
}

/**
 * Read and parse `state.toml`. Rejects the promise with a
 * {@link FlowStateParseError} on malformed content; the orchestrator
 * guarantees well-formed TOML but user hand-edits are possible.
 */
export async function readFlowState(projectDir: string): Promise<FlowState> {
  const file = stateFilePath(projectDir);
  let raw: string;
  try {
    raw = await fs.readFile(file, "utf8");
  } catch (cause) {
    throw new FlowStateParseError(`Cannot read ${file}: ${(cause as Error).message}`, file, cause);
  }
  return parseFlowStateText(raw, file);
}

/** Parse state.toml content supplied as a string. Test entry point. */
export function parseFlowStateText(text: string, file = STATE_FILE): FlowState {
  let parsed: unknown;
  try {
    parsed = parseToml(text);
  } catch (cause) {
    throw new FlowStateParseError(
      `Malformed TOML in ${file}: ${(cause as Error).message}`,
      file,
      cause,
    );
  }
  if (!isRecord(parsed)) {
    throw new FlowStateParseError(`Top level of ${file} must be a table`, file);
  }

  const flow = parseFlow(parsed.flow, file);
  const current_step = parseString(parsed.current_step, "current_step", file);
  const started = parseOptionalString(parsed.started, "started", file);
  const gates = parseGateMap(parsed.gates, `${file}:[gates]`);
  const archivedRaw = parsed.archived_gates;
  const archived_gates = parseArchivedGates(archivedRaw, file);

  return {
    flow,
    current_step,
    started,
    gates,
    archived_gates,
  };
}

// -------------------------------------------------------------
// Helpers
// -------------------------------------------------------------

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseFlow(value: unknown, file: string): Flow {
  if (value === "direct-modeling" || value === "design-study") {
    return value;
  }
  throw new FlowStateParseError(
    `Field \`flow\` in ${file} must be "direct-modeling" or "design-study", got ${JSON.stringify(value)}`,
    file,
  );
}

function parseString(value: unknown, field: string, file: string): string {
  if (typeof value !== "string") {
    throw new FlowStateParseError(`Field \`${field}\` in ${file} must be a string`, file);
  }
  return value;
}

function parseOptionalString(value: unknown, _field: string, _file: string): string | null {
  if (value === undefined || value === null) {
    return null;
  }
  if (typeof value === "string") {
    return value;
  }
  // smol-toml parses RFC-3339 datetimes into Date objects; stringify back
  // so downstream code sees the same ISO-8601 shape as the JSON contract.
  if (value instanceof Date) {
    return value.toISOString();
  }
  return String(value);
}

function parseGateMap(value: unknown, ctx: string): Record<string, Gate> {
  if (value === undefined) {
    return {};
  }
  if (!isRecord(value)) {
    throw new FlowStateParseError(`${ctx} must be a table`, ctx);
  }
  const out: Record<string, Gate> = {};
  for (const [key, v] of Object.entries(value)) {
    out[key] = parseGate(v, `${ctx}.${key}`);
  }
  return out;
}

function parseGate(value: unknown, ctx: string): Gate {
  if (!isRecord(value)) {
    throw new FlowStateParseError(`${ctx} must be a table`, ctx);
  }
  const passed = typeof value.passed === "boolean" ? value.passed : false;
  const timestamp = parseOptionalString(value.timestamp, "timestamp", ctx);
  const candidates = parseGateMap(value.candidates, `${ctx}.candidates`);
  return { passed, timestamp, candidates };
}

function parseArchivedGates(value: unknown, file: string): Record<string, Record<string, Gate>> {
  if (value === undefined) {
    return {};
  }
  if (!isRecord(value)) {
    throw new FlowStateParseError(`Field \`archived_gates\` in ${file} must be a table`, file);
  }
  const out: Record<string, Record<string, Gate>> = {};
  for (const [flowKey, inner] of Object.entries(value)) {
    out[flowKey] = parseGateMap(inner, `${file}:[archived_gates.${flowKey}]`);
  }
  return out;
}
