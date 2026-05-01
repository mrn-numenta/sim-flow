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
  // `tools/sim-flow/src/steps/dm.rs` for the canonical paths.
  DM0: ["docs/spec.md"],
  DM1: ["docs/targets.md", "docs/testbench.md"],
  DM2a: ["docs/analysis/decomposition.md", "docs/analysis/data-movement.md"],
  DM2b: ["docs/analysis/pipeline-mapping.md"],
  DM2c: ["docs/plan/plan.md"],
  DM2d: ["src/model/", "tests/"],
  DM3a: ["docs/plan/test-plan.md"],
  DM3b: ["tests/"],
  DM3c: ["tests/", "docs/plan/test-plan.md"],
  DM4a: ["docs/plan/perf-plan.md"],
  DM4b: ["docs/analysis/", "experiments.db"],
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
    "DM2d",
    "DM3a",
    "DM3b",
    "DM3c",
    "DM4a",
    "DM4b",
  ],
  "design-study": ["DS0", "DS1", "DS2", "DS3", "DS4", "DS5a", "DS5b", "DS5c", "DS6"],
};

export interface EnumerateInput {
  projectDir: string;
  flow: string;
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
            out.push({
              absPath: child,
              relPath: childRel,
              category: "work-artifact",
              step: stepId,
              bytes: stats?.size ?? null,
              exists: stats !== null,
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
            exists: false,
          });
        }
        continue;
      }
      const abs = path.join(input.projectDir, rel);
      const stats = safeStat(abs);
      out.push({
        absPath: abs,
        relPath: rel,
        category: "work-artifact",
        step: stepId,
        bytes: stats?.size ?? null,
        exists: stats !== null,
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
