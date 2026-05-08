import * as fs from "node:fs";
import * as path from "node:path";
import { tmpdir } from "node:os";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { enumerateProjectDocuments } from "./documents";

let projectDir: string;

beforeEach(() => {
  projectDir = fs.mkdtempSync(path.join(tmpdir(), "sim-flow-docs-"));
});

afterEach(() => {
  fs.rmSync(projectDir, { recursive: true, force: true });
});

function writeFile(rel: string, body: string): string {
  const full = path.join(projectDir, rel);
  fs.mkdirSync(path.dirname(full), { recursive: true });
  fs.writeFileSync(full, body, "utf8");
  return full;
}

describe("enumerateProjectDocuments (flow handling)", () => {
  it("returns an empty list for an unknown flow", () => {
    expect(enumerateProjectDocuments({ projectDir, flow: "no-such-flow" })).toEqual([]);
  });

  it("emits placeholder rows for every step's expected artifacts when nothing exists", () => {
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    // Every step's artifacts should be represented as `exists: false`
    // placeholder rows. DM2c's `docs/impl-plan/` is a directory
    // artifact -- empty dir should still produce one placeholder.
    expect(got.length).toBeGreaterThan(0);
    for (const row of got) {
      expect(row.exists).toBe(false);
      expect(row.bytes).toBeNull();
      expect(row.modifiedAt).toBeNull();
    }
  });

  it("supports the design-study flow's step set", () => {
    writeFile("docs/spec.md", "# DS spec\n");
    const got = enumerateProjectDocuments({ projectDir, flow: "design-study" });
    const ds0 = got.find((r) => r.step === "DS0" && r.relPath === "docs/spec.md");
    expect(ds0).toBeDefined();
    expect(ds0!.exists).toBe(true);
  });
});

describe("enumerateProjectDocuments (file artifacts)", () => {
  it("populates exists / bytes / modifiedAt for present files", () => {
    writeFile("docs/spec.md", "Spec body\n");
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const dm0 = got.find((r) => r.step === "DM0" && r.relPath === "docs/spec.md");
    expect(dm0).toBeDefined();
    expect(dm0!.exists).toBe(true);
    expect(dm0!.bytes).toBeGreaterThan(0);
    expect(typeof dm0!.modifiedAt).toBe("string");
  });

  it("includes a critique row when docs/critiques/<step>-critique.md exists", () => {
    writeFile("docs/critiques/DM0-critique.md", "# critique\n- BLOCKER: nope\n");
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const c = got.find(
      (r) => r.category === "critique" && r.relPath === "docs/critiques/DM0-critique.md",
    );
    expect(c).toBeDefined();
    expect(c!.step).toBe("DM0");
    expect(c!.exists).toBe(true);
  });

  it("does not emit critique rows for missing critique files", () => {
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const anyCritique = got.find((r) => r.category === "critique");
    expect(anyCritique).toBeUndefined();
  });
});

describe("enumerateProjectDocuments (directory artifacts)", () => {
  it("walks directory artifacts shallowly and emits a row per file", () => {
    writeFile("docs/impl-plan/plan.md", "outline\n");
    writeFile("docs/impl-plan/milestone-01-foo.md", "- [ ] task\n");
    writeFile("docs/impl-plan/milestone-02-bar.md", "- [x] done\n");
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const planFiles = got.filter(
      (r) => r.step === "DM2c" && r.relPath.startsWith("docs/impl-plan/"),
    );
    expect(planFiles.length).toBeGreaterThanOrEqual(3);
    for (const row of planFiles) {
      expect(row.exists).toBe(true);
      expect(row.category).toBe("work-artifact");
    }
  });

  it("falls back to a placeholder row when the directory is missing", () => {
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const dm2c = got.filter((r) => r.step === "DM2c");
    expect(dm2c).toHaveLength(1);
    expect(dm2c[0].relPath).toBe("docs/impl-plan/");
    expect(dm2c[0].exists).toBe(false);
  });

  it("counts lines for source files (DM2d's src/ tree) but not markdown", () => {
    writeFile("src/lib.rs", "pub fn one() {}\npub fn two() {}\n");
    writeFile("src/main.rs", "fn main() {}\n");
    writeFile("docs/spec.md", "spec content\nline 2\n"); // markdown -> no lineCount
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const lib = got.find((r) => r.relPath === "src/lib.rs");
    const main = got.find((r) => r.relPath === "src/main.rs");
    const spec = got.find((r) => r.step === "DM0" && r.relPath === "docs/spec.md");
    expect(lib?.lineCount).toBe(2);
    expect(main?.lineCount).toBe(1);
    expect(spec?.lineCount).toBeUndefined();
  });
});

describe("enumerateProjectDocuments (previews)", () => {
  it("attaches a markdown preview for docs/testbench.md", () => {
    writeFile("docs/testbench.md", "## Strategy\n\nwhole body\n");
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const tb = got.find((r) => r.relPath === "docs/testbench.md");
    expect(tb?.previews).toBeDefined();
    expect(tb!.previews!).toHaveLength(1);
    expect(tb!.previews![0].kind).toBe("markdown");
    if (tb!.previews![0].kind === "markdown") {
      expect(tb!.previews![0].body).toContain("whole body");
    }
  });

  it("truncates large markdown previews", () => {
    // 10KB > 8KB cap.
    writeFile("docs/testbench.md", "x".repeat(10_000));
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const tb = got.find((r) => r.relPath === "docs/testbench.md");
    expect(tb!.previews![0].kind).toBe("markdown");
    if (tb!.previews![0].kind === "markdown") {
      expect(tb!.previews![0].body).toMatch(/\(truncated for preview\)/);
      expect(tb!.previews![0].body.length).toBeLessThan(10_000);
    }
  });

  it("extracts a table preview under the named heading", () => {
    writeFile(
      "docs/targets.md",
      [
        "# DM1 Targets",
        "",
        "## Target Summary",
        "",
        "| Metric | Value |",
        "|--------|-------|",
        "| Throughput | 1 GHz |",
        "| Area | 2.0 mm^2 |",
        "",
        "## Other Section",
        "ignored",
      ].join("\n"),
    );
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const t = got.find((r) => r.relPath === "docs/targets.md");
    expect(t?.previews).toBeDefined();
    expect(t!.previews!).toHaveLength(1);
    const preview = t!.previews![0];
    expect(preview.kind).toBe("table");
    if (preview.kind === "table") {
      expect(preview.caption).toBe("Target Summary");
      expect(preview.headers).toEqual(["Metric", "Value"]);
      expect(preview.rows).toEqual([
        ["Throughput", "1 GHz"],
        ["Area", "2.0 mm^2"],
      ]);
    }
  });

  it("falls back to no preview when the table heading is missing", () => {
    writeFile("docs/targets.md", "# Targets\n\nno table here.\n");
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const t = got.find((r) => r.relPath === "docs/targets.md");
    expect(t?.previews).toBeUndefined();
  });

  it("falls back to no preview when the heading is present but no table follows before the next heading", () => {
    writeFile(
      "docs/targets.md",
      ["## Target Summary", "prose only", "", "## Next", "next"].join("\n"),
    );
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const t = got.find((r) => r.relPath === "docs/targets.md");
    expect(t?.previews).toBeUndefined();
  });

  it("pads ragged table rows to match the header column count", () => {
    writeFile(
      "docs/analysis/decomposition.md",
      [
        "## Operation Summary",
        "",
        "| Op | Cost | Notes |",
        "|----|------|-------|",
        "| add | 1 |", // missing third cell
        "| mul | 3 | hot path | extra |", // extra cell -- trimmed
      ].join("\n"),
    );
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const d = got.find((r) => r.relPath === "docs/analysis/decomposition.md");
    const preview = d?.previews?.[0];
    expect(preview?.kind).toBe("table");
    if (preview?.kind === "table") {
      expect(preview.headers).toEqual(["Op", "Cost", "Notes"]);
      expect(preview.rows[0]).toEqual(["add", "1", ""]);
      expect(preview.rows[1]).toEqual(["mul", "3", "hot path"]);
    }
  });

  it("does not attach a preview for files without a matching rule", () => {
    writeFile("docs/spec.md", "# spec\n");
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const spec = got.find((r) => r.step === "DM0" && r.relPath === "docs/spec.md");
    expect(spec?.previews).toBeUndefined();
  });

  it("does not attach a preview for an empty file", () => {
    writeFile("docs/testbench.md", "");
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const tb = got.find((r) => r.relPath === "docs/testbench.md");
    expect(tb?.previews).toBeUndefined();
  });
});

describe("enumerateProjectDocuments (source-spec rows)", () => {
  it("emits a TOC row when .sim-flow/source-spec-toc.md exists", () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    fs.writeFileSync(
      path.join(projectDir, ".sim-flow", "source-spec-toc.md"),
      "TOC body",
      "utf8",
    );
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const toc = got.find((r) => r.relPath === ".sim-flow/source-spec-toc.md");
    expect(toc).toBeDefined();
    expect(toc!.category).toBe("source-spec");
    expect(toc!.exists).toBe(true);
  });

  it("emits a source-spec row for each known extension that's present", () => {
    fs.mkdirSync(path.join(projectDir, ".sim-flow"));
    for (const ext of ["md", "txt", "pdf"]) {
      fs.writeFileSync(
        path.join(projectDir, ".sim-flow", `source-spec.${ext}`),
        "body",
        "utf8",
      );
    }
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    const specs = got.filter((r) => r.relPath.startsWith(".sim-flow/source-spec."));
    expect(specs.length).toBeGreaterThanOrEqual(3);
    for (const s of specs) {
      expect(s.category).toBe("source-spec");
      expect(s.exists).toBe(true);
    }
  });

  it("emits no source-spec rows when .sim-flow doesn't exist", () => {
    const got = enumerateProjectDocuments({ projectDir, flow: "direct-modeling" });
    expect(got.find((r) => r.category === "source-spec")).toBeUndefined();
  });
});
