// List and parse `docs/critiques/*.md`. The parser follows the
// same rule as the Rust `Critique::parse` (see
// `tools/sim-flow/src/critique.rs`): a line whose first non-whitespace
// token (after stripping common markdown list markers `- ` or `* `) is
// `UNRESOLVED:`, `BLOCKER:`, or `RESOLVED:` becomes a finding; all
// other lines are prose. Blocking status = any Blocker.
//
// Critiques deliberately live OUTSIDE `.sim-flow/` (which is the
// orchestrator's private state tree) so the agent can write to them
// via its own filesystem tools without being granted write access
// to `state.toml` etc. Mirrors `tools/sim-flow/src/runner.rs`'s
// `CRITIQUES_DIR` -- keep both in sync.

import { promises as fs } from "node:fs";
import * as path from "node:path";

import type { CritiqueFile, Finding, FindingKind } from "./types";

export type { CritiqueFile, Finding, FindingKind };

export const CRITIQUES_DIR = path.join("docs", "critiques");

/** Resolve the absolute path to a project's critiques directory. */
export function critiquesDir(projectDir: string): string {
  return path.join(projectDir, CRITIQUES_DIR);
}

/** Resolve the absolute path of a specific step's critique file. */
export function critiquePath(projectDir: string, stepId: string): string {
  return path.join(critiquesDir(projectDir), `${stepId}-critique.md`);
}

/**
 * List every critique file present under a project.
 *
 * Returns an empty array if the directory does not exist (normal for
 * projects that have not completed any steps yet).
 */
export async function listCritiqueFiles(projectDir: string): Promise<CritiqueFile[]> {
  const dir = critiquesDir(projectDir);
  let entries: string[];
  try {
    entries = await fs.readdir(dir);
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") {
      return [];
    }
    throw err;
  }
  const files = entries.filter((name) => name.endsWith("-critique.md")).sort();
  const results: CritiqueFile[] = [];
  for (const name of files) {
    const full = path.join(dir, name);
    const step = name.replace(/-critique\.md$/, "");
    const body = await fs.readFile(full, "utf8");
    results.push({
      path: full,
      step,
      body,
      ...findingsFor(body),
    });
  }
  return results;
}

/** Read and parse a specific step's critique file. Returns `null` if missing. */
export async function readCritique(
  projectDir: string,
  stepId: string,
): Promise<CritiqueFile | null> {
  const full = critiquePath(projectDir, stepId);
  let body: string;
  try {
    body = await fs.readFile(full, "utf8");
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") {
      return null;
    }
    throw err;
  }
  return {
    path: full,
    step: stepId,
    body,
    ...findingsFor(body),
  };
}

/** Parse the finding markers in a critique body. Exposed for testing. */
export function parseFindings(body: string): {
  findings: Finding[];
  hasBlocking: boolean;
} {
  return findingsFor(body);
}

function findingsFor(body: string): { findings: Finding[]; hasBlocking: boolean } {
  const findings: Finding[] = [];
  let hasBlocking = false;
  const lines = body.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    const finding = parseLine(lines[i], i + 1);
    if (!finding) {
      continue;
    }
    findings.push(finding);
    if (finding.kind !== "resolved") {
      hasBlocking = true;
    }
  }
  return { findings, hasBlocking };
}

function parseLine(raw: string, line: number): Finding | null {
  // Strip leading whitespace, common list markers (`- `, `* `,
  // numbered `12. `), and any wrapping bold markdown (`**...**`)
  // before checking for the BLOCKER/UNRESOLVED/RESOLVED prefix.
  // Agents tend to format findings as `1. **UNRESOLVED: ...**`, so
  // the parser has to tolerate that to surface them in the dashboard.
  let stripped = raw.replace(/^\s+/, "");
  // Markdown list markers: `- `, `* `, `1. `, `12. ` etc.
  stripped = stripped.replace(/^([-*]\s+|\d+\.\s+)/, "");
  // Strip leading `**` (and trailing `**` from the same line) so a
  // `**UNRESOLVED: ...**` heading collapses to `UNRESOLVED: ...`.
  if (stripped.startsWith("**")) {
    stripped = stripped.slice(2);
    // Drop the closing `**` if it's on this line.
    const closing = stripped.lastIndexOf("**");
    if (closing !== -1) {
      stripped = stripped.slice(0, closing) + stripped.slice(closing + 2);
    }
  }
  for (const [prefix, kind] of [
    ["UNRESOLVED:", "unresolved" as const],
    ["BLOCKER:", "blocker" as const],
    ["RESOLVED:", "resolved" as const],
  ] as const) {
    if (stripped.startsWith(prefix)) {
      return {
        kind,
        text: stripped.slice(prefix.length).trim(),
        line,
      };
    }
  }
  return null;
}
