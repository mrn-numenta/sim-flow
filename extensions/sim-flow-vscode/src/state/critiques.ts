// List and parse `docs/critiques/<step>-critique.{json,md}`. The
// canonical form is JSON (see `tools/sim-flow/src/critique.rs`); the
// orchestrator renders a markdown sibling for human reading. We
// prefer the JSON when present and fall back to parsing the markdown
// (legacy projects, or transient races where the JSON landed but the
// `.md` render hasn't yet) so the dashboard's blocker counts and
// `hasBlocking` flag track the same source-of-truth as the gate.
//
// Markdown parser rule mirrors the Rust `Critique::parse`: a line
// whose first non-whitespace token (after stripping common markdown
// list markers `- ` / `* ` / numbered `12. `) is `UNRESOLVED:`,
// `BLOCKER:`, or `RESOLVED:` becomes a finding; all other lines are
// prose.
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

/**
 * Resolve the absolute path of a specific step's critique markdown
 * view. The orchestrator's render-on-write path produces this file
 * from the canonical JSON; existing call sites (open-in-editor,
 * "open critique" buttons) want the human-readable version.
 */
export function critiquePath(projectDir: string, stepId: string): string {
  return path.join(critiquesDir(projectDir), `${stepId}-critique.md`);
}

/** Resolve the absolute path of a specific step's critique JSON. */
export function critiqueJsonPath(projectDir: string, stepId: string): string {
  return path.join(critiquesDir(projectDir), `${stepId}-critique.json`);
}

interface CritiqueJsonShape {
  step?: string;
  summary?: string;
  findings?: Array<{ kind?: string; title?: string; body?: string }>;
  notes?: string;
}

function parseJsonCritique(text: string): { findings: Finding[]; hasBlocking: boolean } | null {
  let parsed: CritiqueJsonShape;
  try {
    parsed = JSON.parse(text) as CritiqueJsonShape;
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object" || !Array.isArray(parsed.findings)) {
    return null;
  }
  const findings: Finding[] = [];
  let hasBlocking = false;
  parsed.findings.forEach((f, idx) => {
    const kindRaw = typeof f?.kind === "string" ? f.kind.toLowerCase() : "";
    let kind: FindingKind | null = null;
    if (kindRaw === "blocker") kind = "blocker";
    else if (kindRaw === "unresolved") kind = "unresolved";
    else if (kindRaw === "resolved") kind = "resolved";
    if (kind === null) {
      return;
    }
    const title = typeof f?.title === "string" ? f.title.trim() : "";
    findings.push({ kind, text: title, line: idx + 1 });
    // `blocker` and `unresolved` both fail the gate; `resolved`
    // stays informational. Mirrors the Rust `Finding::is_blocking`
    // rule in
    // `tools/sim-flow/src/__internal/critique.rs`.
    if (kind === "blocker" || kind === "unresolved") {
      hasBlocking = true;
    }
  });
  return { findings, hasBlocking };
}

/**
 * List every critique file present under a project.
 *
 * Returns an empty array if the directory does not exist (normal for
 * projects that have not completed any steps yet). When both `.json`
 * and `.md` exist for the same step, the JSON's structured findings
 * win and the markdown body is preserved for any consumer that wants
 * the human-readable text.
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
  const stepIds = new Set<string>();
  for (const name of entries) {
    if (name.endsWith("-critique.json")) {
      stepIds.add(name.replace(/-critique\.json$/, ""));
    } else if (name.endsWith("-critique.md")) {
      stepIds.add(name.replace(/-critique\.md$/, ""));
    }
  }
  const sorted = [...stepIds].sort();
  const results: CritiqueFile[] = [];
  for (const step of sorted) {
    const file = await readCritique(projectDir, step);
    if (file) {
      results.push(file);
    }
  }
  return results;
}

/**
 * Read a specific step's critique. Prefers the JSON form for
 * findings (canonical structured source) and falls back to parsing
 * the markdown body when only `.md` exists. Returns `null` when
 * neither form is on disk.
 */
export async function readCritique(
  projectDir: string,
  stepId: string,
): Promise<CritiqueFile | null> {
  const jsonFull = critiqueJsonPath(projectDir, stepId);
  const mdFull = critiquePath(projectDir, stepId);
  const jsonText = await readIfExists(jsonFull);
  const mdText = await readIfExists(mdFull);
  if (jsonText !== null) {
    const parsed = parseJsonCritique(jsonText);
    if (parsed !== null) {
      return {
        // Surface the markdown path when present so existing
        // open-in-editor wiring still routes to the human-readable
        // view; otherwise point at the JSON.
        path: mdText !== null ? mdFull : jsonFull,
        step: stepId,
        body: mdText ?? jsonText,
        findings: parsed.findings,
        hasBlocking: parsed.hasBlocking,
      };
    }
    // Malformed JSON: fall through to markdown if we have it.
  }
  if (mdText !== null) {
    return {
      path: mdFull,
      step: stepId,
      body: mdText,
      ...findingsFor(mdText),
    };
  }
  return null;
}

async function readIfExists(p: string): Promise<string | null> {
  try {
    return await fs.readFile(p, "utf8");
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") {
      return null;
    }
    throw err;
  }
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
    // `blocker` and `unresolved` both block the gate. See the
    // matching note on `parseJsonCritique`.
    if (finding.kind === "blocker" || finding.kind === "unresolved") {
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
