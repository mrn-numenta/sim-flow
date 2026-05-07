// Document enumeration for the dashboard's "Documents" tab.
//
// Walks the on-disk project state and produces a flat list of files
// the user typically wants to open in an editor: per-step work
// artifacts, per-step critique files, and the ingested source spec
// (TOC + original copy). The tab uses this list to render rows; the
// host opens the corresponding file when the user clicks "Open".

import * as fs from "node:fs";
import * as path from "node:path";

import type { DocumentEntry } from "../webview/messages";

/** Hard-coded expected work artifacts per step. Mirrors the
 * `work_artifacts` field in `tools/sim-flow/src/steps/dm.rs` and
 * `ds.rs`. Keeping this in TS avoids running `sim-flow describe`
 * once per step on every dashboard refresh; the lists are stable
 * step-definition data. If the Rust side adds a step / artifact,
 * mirror it here. */
const STEP_ARTIFACTS: Record<string, string[]> = {
  // Direct Modeling Flow. All generated markdown documents now live
  // under `docs/`; Rust source / tests stay at `src/` / `tests/`. See
  // `tools/sim-flow/src/steps/dm.rs` for the canonical paths. The
  // paths below MUST match the Rust step descriptors -- when they
  // drift, the dashboard's per-step artifact list shows phantom
  // "not yet on disk" rows for files that never get written.
  DM0: ["docs/spec.md"],
  DM1: ["docs/targets.md", "docs/testbench.md"],
  DM2a: ["docs/analysis/decomposition.md", "docs/analysis/data-movement.md"],
  DM2b: ["docs/analysis/pipeline-mapping.md"],
  DM2c: ["docs/impl-plan/"],
  // DM2cd writes per-milestone task lists into the SAME directory
  // DM2c set up; the dashboard groups by step id so the same file
  // can appear under both rows. Listing the directory here keeps
  // the per-step view honest.
  DM2cd: ["docs/impl-plan/"],
  DM2d: ["src/", "tests/", "Cargo.toml"],
  DM3a: ["docs/test-plan/"],
  DM3ad: ["docs/test-plan/"],
  DM3b: ["tests/"],
  DM3c: ["tests/"],
  DM4a: ["docs/perf-plan/"],
  DM4ad: ["docs/perf-plan/"],
  DM4b: ["docs/analysis/"],
  // Design Study Flow.
  DS0: ["docs/spec.md"],
  DS1: ["docs/targets.md", "docs/testbench.md"],
  DS2: ["docs/analysis/decomposition.md", "docs/analysis/data-movement.md"],
  DS3: ["docs/analysis/pipeline-mapping.md"],
  DS4: ["docs/analysis/screening.md"],
  DS5a: ["docs/analysis/prototype.md"],
  DS5b: ["docs/analysis/smoke.md"],
  DS5c: ["docs/analysis/full.md"],
  DS6: ["docs/analysis/results.md"],
};

const FLOW_STEP_ORDER: Record<string, string[]> = {
  "direct-modeling": [
    "DM0",
    "DM1",
    "DM2a",
    "DM2b",
    "DM2c",
    "DM2cd",
    "DM2d",
    "DM3a",
    "DM3ad",
    "DM3b",
    "DM3c",
    "DM4a",
    "DM4ad",
    "DM4b",
  ],
  "design-study": ["DS0", "DS1", "DS2", "DS3", "DS4", "DS5a", "DS5b", "DS5c", "DS6"],
};

export interface EnumerateInput {
  projectDir: string;
  flow: string;
}

/**
 * Per-file preview rule. Each rule names either:
 *  - `tableSection`: extract the FIRST markdown table that follows
 *    a `## <section>` heading. The dashboard renders it as a real
 *    HTML table (no raw `| col |` markdown leaking through).
 *  - `markdown`: ship the full file body as a single markdown
 *    preview, rendered inline by the webview.
 *
 * Anything not in the rules ships without `previews`; the
 * dashboard falls back to a TOC-only entry with an Open button.
 */
type PreviewRule = { kind: "tableSection"; section: string } | { kind: "markdown" };

const PREVIEW_RULES: Record<string, PreviewRule> = {
  // DM1 sets up targets + testbench strategy. The user wants the
  // Target Summary table rendered (not the surrounding prose) and
  // the testbench file rendered fully -- it's small + structured
  // and the user wants the whole document at a glance.
  "docs/targets.md": { kind: "tableSection", section: "Target Summary" },
  "docs/testbench.md": { kind: "markdown" },
  // DM2a writes decomposition + data-movement. The user wants the
  // Operation Summary and Edge Summary tables specifically, not
  // the full files (which contain free-form narrative + multiple
  // tables -- only the named summary belongs in the dashboard).
  "docs/analysis/decomposition.md": {
    kind: "tableSection",
    section: "Operation Summary",
  },
  "docs/analysis/data-movement.md": {
    kind: "tableSection",
    section: "Edge Summary",
  },
  // DM2b writes pipeline-mapping. Same pattern: Stage Summary
  // table only.
  "docs/analysis/pipeline-mapping.md": {
    kind: "tableSection",
    section: "Stage Summary",
  },
};

const PREVIEW_FULL_CAP_BYTES = 8192;

/** Extensions for which the per-step view shows a "files / lines"
 *  code summary (DM2d / DM3b / DM3c). Markdown / TOML are excluded
 *  -- those are docs, not code. */
const CODE_EXTENSIONS = new Set<string>([".rs"]);

import type { ArtifactPreview } from "../webview/messages";

/**
 * Build the inline preview list for `rel`. Returns `undefined`
 * when no rule matches; an empty array when a rule matches but
 * the file's content didn't yield extractable content (caller
 * should treat as "preview attempted but empty").
 */
function buildPreviews(rel: string, abs: string, sizeBytes: number): ArtifactPreview[] | undefined {
  const rule = PREVIEW_RULES[rel];
  if (!rule || sizeBytes === 0) {
    return undefined;
  }
  let body: string;
  try {
    body = fs.readFileSync(abs, "utf8");
  } catch {
    return undefined;
  }
  if (rule.kind === "markdown") {
    const truncated =
      body.length > PREVIEW_FULL_CAP_BYTES
        ? body.slice(0, PREVIEW_FULL_CAP_BYTES) + "\n\n_... (truncated for preview)_"
        : body;
    return [{ kind: "markdown", body: truncated }];
  }
  const table = extractTableUnderHeading(body, rule.section);
  if (!table) {
    return undefined;
  }
  return [{ kind: "table", caption: rule.section, headers: table.headers, rows: table.rows }];
}

/**
 * Find a `## <heading>` line (any level >= 1, case-insensitive)
 * and return the FIRST markdown table that follows. Returns
 * `null` when the heading is missing or no table follows before
 * the next heading. Tolerates extra heading adornment (e.g.
 * `### Target Summary (3 rows)`).
 */
function extractTableUnderHeading(
  body: string,
  heading: string,
): { headers: string[]; rows: string[][] } | null {
  const lines = body.split("\n");
  const headingPattern = new RegExp(
    `^#{1,6}\\s+${heading.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\b`,
    "i",
  );
  let i = 0;
  while (i < lines.length && !headingPattern.test(lines[i])) {
    i++;
  }
  if (i >= lines.length) {
    return null;
  }
  i++;
  // Walk to the first table-shaped line under this heading,
  // stopping at the next heading.
  while (i < lines.length) {
    if (/^#{1,6}\s+/.test(lines[i])) {
      return null;
    }
    if (isTableRow(lines[i]) && i + 1 < lines.length && isTableSeparator(lines[i + 1])) {
      return parseTable(lines, i);
    }
    i++;
  }
  return null;
}

function isTableRow(line: string): boolean {
  const t = line.trim();
  return t.startsWith("|") && t.endsWith("|");
}

function isTableSeparator(line: string): boolean {
  const t = line.trim();
  if (!t.startsWith("|") || !t.endsWith("|")) {
    return false;
  }
  // Cells should contain only `-`, `:`, and whitespace.
  return /^\|[\s|:-]+\|$/.test(t);
}

function splitRow(line: string): string[] {
  // Trim outer pipes before splitting so a leading/trailing `|`
  // doesn't add empty cells.
  const t = line.trim().replace(/^\||\|$/g, "");
  return t.split("|").map((c) => c.trim());
}

function parseTable(lines: string[], startIdx: number): { headers: string[]; rows: string[][] } {
  const headers = splitRow(lines[startIdx]);
  const rows: string[][] = [];
  let i = startIdx + 2; // skip header + separator
  while (i < lines.length && isTableRow(lines[i])) {
    const cells = splitRow(lines[i]);
    // Pad missing cells / trim extras to match the header count
    // so the renderer doesn't have to defend against ragged rows.
    while (cells.length < headers.length) {
      cells.push("");
    }
    rows.push(cells.slice(0, headers.length));
    i++;
  }
  return { headers, rows };
}

function countLines(abs: string): number | undefined {
  try {
    const body = fs.readFileSync(abs, "utf8");
    if (body.length === 0) {
      return 0;
    }
    let count = 1;
    for (let i = 0; i < body.length; i++) {
      if (body.charCodeAt(i) === 0x0a) {
        count++;
      }
    }
    // If the file ends with a newline the last "line" is empty;
    // discount it so a 10-line file with a trailing newline reads
    // as 10, not 11.
    if (body.endsWith("\n")) {
      count--;
    }
    return count;
  } catch {
    return undefined;
  }
}

/**
 * Walk the on-disk project layout and produce a list of `DocumentEntry`
 * rows for the dashboard. Pure I/O + no business logic; safe to call
 * on every refresh.
 */
export function enumerateProjectDocuments(input: EnumerateInput): DocumentEntry[] {
  const out: DocumentEntry[] = [];
  const stepOrder = FLOW_STEP_ORDER[input.flow] ?? [];

  // Per-step work artifacts + critique file.
  for (const stepId of stepOrder) {
    const artifacts = STEP_ARTIFACTS[stepId] ?? [];
    for (const rel of artifacts) {
      // Directory artifacts: list immediate file children that match
      // the step's expected output shape (markdown / source). Falls
      // back to a single placeholder row when the directory is empty
      // or absent so the user still sees what's expected.
      if (rel.endsWith("/")) {
        const dirAbs = path.join(input.projectDir, rel);
        let added = 0;
        if (fs.existsSync(dirAbs) && fs.statSync(dirAbs).isDirectory()) {
          for (const child of walkDirShallow(dirAbs, 200)) {
            const childRel = path.posix.join(rel, path.relative(dirAbs, child));
            const stats = safeStat(child);
            const ext = path.extname(child).toLowerCase();
            const lineCount =
              stats !== null && CODE_EXTENSIONS.has(ext) ? countLines(child) : undefined;
            out.push({
              absPath: child,
              relPath: childRel,
              category: "work-artifact",
              step: stepId,
              bytes: stats?.size ?? null,
              modifiedAt: stats?.mtime.toISOString() ?? null,
              exists: stats !== null,
              ...(lineCount !== undefined ? { lineCount } : {}),
            });
            added++;
          }
        }
        if (added === 0) {
          out.push({
            absPath: dirAbs,
            relPath: rel,
            category: "work-artifact",
            step: stepId,
            bytes: null,
            modifiedAt: null,
            exists: false,
          });
        }
        continue;
      }
      const abs = path.join(input.projectDir, rel);
      const stats = safeStat(abs);
      const previews = stats !== null ? buildPreviews(rel, abs, stats.size) : undefined;
      const ext = path.extname(rel).toLowerCase();
      const lineCount = stats !== null && CODE_EXTENSIONS.has(ext) ? countLines(abs) : undefined;
      out.push({
        absPath: abs,
        relPath: rel,
        category: "work-artifact",
        step: stepId,
        bytes: stats?.size ?? null,
        modifiedAt: stats?.mtime.toISOString() ?? null,
        exists: stats !== null,
        ...(previews !== undefined ? { previews } : {}),
        ...(lineCount !== undefined ? { lineCount } : {}),
      });
    }

    const critiqueRel = `docs/critiques/${stepId}-critique.md`;
    const critiqueAbs = path.join(input.projectDir, critiqueRel);
    const critiqueStats = safeStat(critiqueAbs);
    if (critiqueStats) {
      out.push({
        absPath: critiqueAbs,
        relPath: critiqueRel,
        category: "critique",
        step: stepId,
        bytes: critiqueStats.size,
        modifiedAt: critiqueStats.mtime.toISOString(),
        exists: true,
      });
    }
  }

  // Source spec (TOC + ingested copy + first few pages). Pages tend
  // to be many; we list only the TOC and source copy as
  // openable rows. The agent reaches per-page files via tools.
  const dotDir = path.join(input.projectDir, ".sim-flow");
  if (fs.existsSync(dotDir)) {
    const tocAbs = path.join(dotDir, "source-spec-toc.md");
    const tocStats = safeStat(tocAbs);
    if (tocStats) {
      out.push({
        absPath: tocAbs,
        relPath: ".sim-flow/source-spec-toc.md",
        category: "source-spec",
        bytes: tocStats.size,
        modifiedAt: tocStats.mtime.toISOString(),
        exists: true,
      });
    }
    for (const ext of ["pdf", "md", "markdown", "txt"]) {
      const sourceAbs = path.join(dotDir, `source-spec.${ext}`);
      const stats = safeStat(sourceAbs);
      if (stats) {
        out.push({
          absPath: sourceAbs,
          relPath: `.sim-flow/source-spec.${ext}`,
          category: "source-spec",
          bytes: stats.size,
          modifiedAt: stats.mtime.toISOString(),
          exists: true,
        });
      }
    }
  }

  return out;
}

function safeStat(p: string): fs.Stats | null {
  try {
    const s = fs.statSync(p);
    return s.isFile() ? s : null;
  } catch {
    return null;
  }
}

function walkDirShallow(dir: string, cap: number): string[] {
  const out: string[] = [];
  const stack: string[] = [dir];
  while (stack.length > 0 && out.length < cap) {
    const current = stack.pop()!;
    let entries: fs.Dirent[];
    try {
      entries = fs.readdirSync(current, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) {
        stack.push(full);
      } else if (entry.isFile()) {
        out.push(full);
        if (out.length >= cap) {
          break;
        }
      }
    }
  }
  out.sort();
  return out;
}
