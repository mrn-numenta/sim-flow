import { mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";

import { load } from "cheerio";

const inputRoot = process.argv[2];
const outputRoot = process.argv[3];

if (!inputRoot || !outputRoot) {
  console.error("render-rustdoc-api: usage: node render-rustdoc-api.mjs <rustdoc-dir> <out-dir>");
  process.exit(1);
}

const crates = ["foundation_framework", "foundation_macros"];
const curatedToc = [
  [
    "pages/foundation_framework/index.md",
    "crate root overview and curated root re-exports",
  ],
  [
    "pages/foundation_framework/prelude/index.md",
    "recommended model-author convenience import surface",
  ],
  [
    "pages/foundation_framework/model/dataflow/index.md",
    "core modeling traits, ports, topology declarations, and lane contexts",
  ],
  [
    "pages/foundation_framework/model/hierarchy/index.md",
    "structural modeling, hierarchy builders, elaboration, and connectivity",
  ],
  [
    "pages/foundation_framework/module_library/index.md",
    "built-in reusable modules and library metadata",
  ],
  [
    "pages/foundation_framework/runtime/index.md",
    "runtime construction helpers, multi-clock runtime specs, and execution entry points",
  ],
  [
    "pages/foundation_framework/integration/index.md",
    "integration-facing APIs including testbench support",
  ],
  [
    "pages/foundation_framework/uvm/index.md",
    "simulation environment builders, drivers, sequencers, and scoreboards",
  ],
  [
    "pages/foundation_framework/topology/index.md",
    "topology helpers used by mesh / NoC style models",
  ],
  [
    "pages/foundation_framework/observability/index.md",
    "signal tracing, checkpoints, observability readers/writers, and related helpers",
  ],
  ["pages/foundation_macros/index.md", "derive macro crate overview"],
  ["pages/foundation_macros/derive.HasLogic.md", "derive macro for HasLogic"],
  ["pages/foundation_macros/derive.HasInstances.md", "derive macro for HasInstances"],
  [
    "pages/foundation_macros/derive.SignalTracePayload.md",
    "derive macro for SignalTracePayload",
  ],
  [
    "pages/foundation_macros/derive.SignalTraceState.md",
    "derive macro for SignalTraceState",
  ],
  ["pages/foundation_macros/derive.CheckpointModel.md", "derive macro for CheckpointModel"],
  ["pages/foundation_macros/derive.ConfigModel.md", "derive macro for ConfigModel"],
];

rmSync(outputRoot, { recursive: true, force: true });
mkdirSync(join(outputRoot, "pages"), { recursive: true });

let pageCount = 0;
for (const crateName of crates) {
  const crateRoot = join(inputRoot, crateName);
  for (const htmlPath of walkHtml(crateRoot)) {
    const rel = relative(inputRoot, htmlPath);
    const outRel = join("pages", rel).replace(/\.html$/u, ".md");
    const outPath = join(outputRoot, outRel);
    mkdirSync(dirname(outPath), { recursive: true });
    writeFileSync(outPath, convertRustdocHtml(htmlPath, rel));
    pageCount += 1;
  }
}

writeFileSync(join(outputRoot, "toc.md"), buildToc(pageCount));

function walkHtml(root) {
  const out = [];
  const stack = [root];
  while (stack.length > 0) {
    const current = stack.pop();
    let entries = [];
    try {
      entries = readdirSync(current);
    } catch {
      continue;
    }
    for (const entry of entries) {
      const abs = join(current, entry);
      const st = statSync(abs);
      if (st.isDirectory()) {
        stack.push(abs);
        continue;
      }
      if (!st.isFile() || !entry.endsWith(".html")) {
        continue;
      }
      if (entry === "all.html") {
        continue;
      }
      const rel = relative(root, abs);
      if (rel.startsWith("src/")) {
        continue;
      }
      out.push(abs);
    }
  }
  out.sort();
  return out;
}

function convertRustdocHtml(path, rel) {
  const $ = load(readFileSync(path, "utf8"));
  const title = normalize($("main h1").first().text()) || normalize($("title").text());
  const breadcrumbs = normalize($(".rustdoc-breadcrumbs").first().text());
  const itemDecl = normalizeWhitespacePreserveCode($("pre.item-decl").first().text());
  const topDoc = collectDocblocks($, ".top-doc .docblock");
  const sections = [];

  $("main .content > h2.section-header").each((_, el) => {
    const heading = normalize($(el).text());
    const anchor = $(el).attr("id") ?? "";
    const next = $(el).next();
    const body = sectionBody($, next);
    if (body) {
      sections.push({ heading, anchor, body });
    }
  });

  const lines = [];
  lines.push(`# ${title || rel}`);
  lines.push("");
  lines.push(`- Rustdoc path: \`${rel}\``);
  if (breadcrumbs) {
    lines.push(`- Breadcrumbs: \`${breadcrumbs}\``);
  }
  lines.push("");

  if (itemDecl) {
    lines.push("## Declaration");
    lines.push("");
    lines.push("```rust");
    lines.push(itemDecl);
    lines.push("```");
    lines.push("");
  }

  if (topDoc) {
    lines.push("## Summary");
    lines.push("");
    lines.push(topDoc);
    lines.push("");
  }

  for (const section of sections) {
    lines.push(`## ${section.heading}`);
    lines.push("");
    lines.push(section.body);
    lines.push("");
  }

  return `${lines.join("\n").trim()}\n`;
}

function collectDocblocks($, selector) {
  const chunks = [];
  $(selector).each((_, el) => {
    const text = normalizeWhitespacePreserveCode($(el).text());
    if (text) {
      chunks.push(text);
    }
  });
  return chunks.join("\n\n");
}

function sectionBody($, node) {
  if (!node || node.length === 0) {
    return "";
  }
  if (node.is("dl.item-table")) {
    const items = [];
    node.children("dt").each((_, dt) => {
      const name = normalize($(dt).text());
      const dd = $(dt).next("dd");
      const summary = dd.length > 0 ? normalizeWhitespacePreserveCode(dd.text()) : "";
      items.push(summary ? `- ${name}: ${summary}` : `- ${name}`);
    });
    return items.join("\n");
  }
  if (node.hasClass("methods") || node.attr("id") === "implementors-list") {
    const items = [];
    node.children("section").each((_, section) => {
      const code = normalizeWhitespacePreserveCode($(section).find(".code-header").first().text());
      const doc = normalizeWhitespacePreserveCode($(section).find(".docblock").first().text());
      if (code) {
        items.push(`- \`${code}\`${doc ? `\n  ${indent(doc)}` : ""}`);
      }
    });
    return items.join("\n");
  }
  const text = normalizeWhitespacePreserveCode(node.text());
  return text;
}

function indent(text) {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .join("\n  ");
}

function normalize(value) {
  return value.replace(/\s+/gu, " ").trim();
}

function normalizeWhitespacePreserveCode(value) {
  return value
    .replace(/\r\n/gu, "\n")
    .replace(/\n{3,}/gu, "\n\n")
    .split("\n")
    .map((line) => line.trimEnd())
    .join("\n")
    .trim();
}

function buildToc(pageCount) {
  const lines = [];
  lines.push("# Framework API navigation");
  lines.push("");
  lines.push(
    "Two routes are available. Prefer the LIVE `api_*` tools first; fall back to the static page snapshot only when a signature isn't covered there or rust-analyzer is unavailable.",
  );
  lines.push("");
  lines.push("## Tool palette (prefer this)");
  lines.push("");
  lines.push(
    "These talk to a `rust-analyzer` rooted at the foundation workspace, so results always match the current code -- no stale snapshots.",
  );
  lines.push("");
  lines.push(
    "- `api_search(query)` -- find symbols by name. First call when you don't know the exact spelling. Returns `kind  location  name`.",
  );
  lines.push(
    "- `api_hover(query)` -- signature + rustdoc for a named symbol. Direct replacement for reading individual `fw:api/pages/...md` files.",
  );
  lines.push(
    "- `api_impls(query)` -- every `impl` of a trait, or every concrete instantiation of a generic. Answers questions the static docs cannot: \"who implements `HasLogic`?\"",
  );
  lines.push(
    "- `api_references(query)` -- every usage site of a symbol. Use to see how a type is actually consumed by library models and examples.",
  );
  lines.push(
    "- `api_expand_macro(path, line)` -- show what a derive (or any macro) expands to at a given source line. The static docs cannot answer this.",
  );
  lines.push("");
  lines.push(
    "First call per session spawns rust-analyzer (~2 min for the cold index of the full foundation workspace); subsequent calls are fast.",
  );
  lines.push("");
  lines.push("## Recommended starting points");
  lines.push("");
  lines.push(
    "For each area below, run `api_hover(<symbol>)` to see the live signature + docs. Use `api_search` if a name doesn't resolve.",
  );
  lines.push("");
  lines.push(
    "- `prelude` -- recommended model-author convenience imports.",
  );
  lines.push(
    "- `model::dataflow` -- core modeling traits (`HasLogic`, `HasInstances`), ports, lane contexts.",
  );
  lines.push(
    "- `model::hierarchy` -- structural modeling, hierarchy builders, elaboration, connectivity.",
  );
  lines.push("- `module_library` -- built-in reusable modules.");
  lines.push(
    "- `runtime` -- runtime construction, multi-clock specs, execution entry points (`Scheduler`).",
  );
  lines.push(
    "- `integration` -- integration-facing APIs including testbench support.",
  );
  lines.push(
    "- `uvm` -- simulation environment builders, drivers, sequencers, scoreboards.",
  );
  lines.push("- `topology` -- mesh / NoC topology helpers.");
  lines.push(
    "- `observability` -- signal tracing (`SignalTracePayload`, `SignalTraceState`), checkpoints (`CheckpointModel`, `ConfigModel`), readers/writers.",
  );
  lines.push(
    "- Derive macros: `HasLogic`, `HasInstances`, `SignalTracePayload`, `SignalTraceState`, `CheckpointModel`, `ConfigModel`. Use `api_hover(<Name>)` for the derive's own docs, then `api_expand_macro(<derive-site-path>, <line>)` on a struct that uses it to see what it generates.",
  );
  lines.push("");
  lines.push("## Static page snapshot (fallback)");
  lines.push("");
  lines.push(
    "The full rustdoc corpus is mirrored under `fw:api/pages/...` for cases the tools don't cover or when rust-analyzer is unavailable. Use `read_file(path=\"fw:api/pages/...\")` or `list_dir(\"fw:api/pages/foundation_framework/<module>\")` to browse. The starting points above map one-to-one to `fw:api/pages/foundation_framework/<area>/index.md`.",
  );
  lines.push("");
  lines.push(`- Total normalized API pages: ${pageCount}`);
  lines.push("- TOC path: `fw:api/toc.md`");
  lines.push("- Page root: `fw:api/pages/`");
  lines.push("");
  lines.push("Curated page index (same areas as above):");
  lines.push("");
  for (const [path, desc] of curatedToc) {
    lines.push(`- \`fw:api/${path}\` -- ${desc}`);
  }
  lines.push("");
  return `${lines.join("\n")}\n`;
}
