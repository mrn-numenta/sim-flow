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
 * Files for which the per-step dashboard view inlines a content
 * preview (capped at PREVIEW_CAP_BYTES). The user wants
 * decomposition + pipeline-mapping summary tables visible at a
 * glance from the step rail without an "Open" click round-trip.
 * Other markdown artifacts stay link-only -- inlining everything
 * would balloon the dashboard payload.
 */
const PREVIEW_PATHS = new Set<string>([
  "docs/analysis/decomposition.md",
  "docs/analysis/pipeline-mapping.md",
  "docs/analysis/data-movement.md",
  "docs/targets.md",
  "docs/testbench.md",
]);
const PREVIEW_CAP_BYTES = 4096;

/** Extensions for which the per-step view shows a "files / lines"
 *  code summary (DM2d / DM3b / DM3c). Markdown / TOML are excluded
 *  -- those are docs, not code. */
const CODE_EXTENSIONS = new Set<string>([".rs"]);

function readPreview(abs: string, sizeBytes: number): string | undefined {
  if (sizeBytes === 0) {
    return undefined;
  }
  try {
    if (sizeBytes <= PREVIEW_CAP_BYTES) {
      return fs.readFileSync(abs, "utf8");
    }
    // Larger files: read the head only. Capped reads keep dashboard
    // refresh cheap even when a generated artifact grows past the
    // cap (rare in practice -- most plans stay small).
    const fd = fs.openSync(abs, "r");
    try {
      const buf = Buffer.alloc(PREVIEW_CAP_BYTES);
      const read = fs.readSync(fd, buf, 0, PREVIEW_CAP_BYTES, 0);
      return buf.subarray(0, read).toString("utf8") + "\n... (truncated)";
    } finally {
      fs.closeSync(fd);
    }
  } catch {
    return undefined;
  }
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
      const previewBody =
        stats !== null && PREVIEW_PATHS.has(rel) ? readPreview(abs, stats.size) : undefined;
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
        ...(previewBody !== undefined ? { previewBody } : {}),
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
