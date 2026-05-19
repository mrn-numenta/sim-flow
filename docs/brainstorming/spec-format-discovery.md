# Spec Format Discovery — Brainstorm

**Status:** brainstorm. Captures the design thinking behind
sim-flow's per-spec format descriptor and the two-pass
classifier. The architecture contract lives in Chapter 7
([07-spec-format-discovery.md](../architecture/07-spec-format-discovery.md));
the implementation plan in
[09-phase-format-discovery.md](../plan/09-phase-format-discovery.md).
**Date:** 2026-05-18; revised 2026-05-19.
**Related:** [spec-ingest-figure-extraction.md](spec-ingest-figure-extraction.md),
[spec-md-restructure.md](spec-md-restructure.md),
[lancedb-rig-incorporation-plan.md](lancedb-rig-incorporation-plan.md),
the four-spec evaluation thread from the pdf_oxide swap.

## 1. Why this matters

The pdf_oxide swap (commit `d611b7f`) replaced pdfium's flat
text dump with a Rust-native PDF backend that exposes:

- `extract_spans(page)` / `extract_text_lines(page)` /
  `extract_words(page)` — per-span text + bbox + font_size +
  font_weight + is_italic + color.
- `extract_tables(page)` — spatial-projection table detector
  returning `Vec<Table>` with rows, columns, cells, bboxes,
  header detection, and span metadata.
- `extract_paths(page)` / `extract_images(page)` — vector ops
  and embedded raster images.
- `render_page(doc, page, opts)` — tiny-skia page rasteriser.

On RV12 the chunk quality jumped from 135 chunks / 112 stubs
(83% noise) to 205 chunks / 20 stubs (10% noise) at the same
DPI just from font-cluster-driven heading detection.
`extract_tables` recovers the privilege table (5×4, clean),
per-stage signal tables, and Apical NoC's 29×3 parameter
table with wrapped comments — without any header-line regex.

So detection is solved deterministically. What's left is
**classification**: is this detected table a signal table, a
CSR table, a memory map, an FSM transition table? Is this
section heading the Instruction Fetch block, the glossary, the
memory-map section, or generic prose? Each spec phrases column
headers and section names differently (`Signal Direction
To/From Description` vs `Port Size Direction Description`;
`Memory Map` vs `Address Map` vs `System Address Space`), and
we don't want to hand-roll Rust heuristics to recognise every
variation.

The shape: deterministic detection (pdf_oxide) + deterministic
first-cut classification (Rust heuristics covering the easy
cases) + an LLM critique pass that fixes ambiguous /
heuristic-missed cases, emitting a per-spec **format
descriptor**. The descriptor is cached on disk and reused
unchanged across runs.

## 2. The core idea

The pipeline gains a `format` stage between `loading` and
`parse`. Internally everything operates on **structured
per-page data** (spans, tables, paths, images) — no markdown
intermediate, no `<!-- page=N -->` marker hack. Markdown is
the OUTPUT shape for chunks emitted to disk; it is not the
internal representation.

```text
loading        → per-page { spans, tables, paths, images }
                 via pdf_oxide structured API

format         → format.json
                   1. pdf_oxide detection (always)
                   2. Rust first-cut classifier (always)
                   3. LLM critique pass (one call per spec,
                      cached)
                   4. deterministic validation post-pass

parse          → SectionTree from font-clustered spans;
                 sections tagged with format.json roles;
                 page_range native from per-page API

classify       → typed tables (TOML) for each format.json
                 table entry, projecting pdf_oxide cells
                 through the descriptor's column_map

chrome-strip   → bbox Y-banding + format.json chrome regexes

emit           → per-section chunks (Markdown rendering for
                 retrieval) + typed tables (TOML) + figure
                 rasters (PNG) + manifest.toml
```

`format.json` lives at
`<project>/.sim-flow/spec-ingest/format.json` alongside
`manifest.toml`. Generated on first ingest, reused on
subsequent runs unless the source PDF SHA or the LLM model /
prompt version changes, OR the user runs `--rediscover-format`.

## 3. The two-pass classifier

The descriptor pipeline has four sources of truth, in order:

1. **pdf_oxide detection.** Deterministic. Tables come from
   `extract_tables(page)`; headings from font-clustering
   spans; figures from `extract_images` + `extract_paths`;
   span bboxes for chrome-band detection. No LLM, no user.
   Always runs.

2. **First-cut classifier (Rust heuristics).** Deterministic.
   Assigns provisional `spec_md_role`, `kind`, `column_map`,
   `layer`, etc. by matching against known patterns:
   - **Column-header keyword sets** — `{"Signal", "Direction",
     ("To/From"|"From/To"), "Description"}` (≥ 3-of-4 match) →
     `signal_table`; `{"Parameter", "Type", "Default",
     "Description"}` → `parameter_table`; `{"Bit"|"Bits"|"Field",
     "Name", "Description"}` → `csr_field_table`; etc.
   - **Section-heading patterns** — `"Glossary"` → `glossary`;
     `"Memory Map" | "Address Map"` → `memory_map`;
     `<Name> (<ACR>)` → `block:<Name>`;
     `"Configurations" | "Parameters"` → `parameters`; etc.
   - **Parenthesised first-mention regex** for glossary
     candidates.
   - **Repeated-line + Y-band** for chrome candidates.
   - Heuristics that don't match anything mark the entry
     `kind: "unknown"`.
   Always runs. Unit-tested in isolation. Cheap, predictable.

3. **LLM critique pass.** One call per spec. The model sees
   the first-cut descriptor draft + a structural skeleton of
   the document (headings list, table-header strings, image
   references, glossary candidates — built from pdf_oxide
   output). The model's job is to **critique and refine**,
   not classify from scratch:
   - Confirm correct first-cut tags (the cheap case).
   - Override incorrect tags (with rationale).
   - Resolve `kind: "unknown"` entries when context allows.
   - Flag inconsistencies (two sections claiming the same
     `csrs` role; column maps that don't align with the
     table's actual content; etc.).
   The model emits an **adjustments patch** — per-entry
   `{ id, field, old_value, new_value, rationale }` — not a
   full descriptor. Token cost scales with the number of
   ambiguous cases, not the document size.

4. **`ask_user` at DM0 time.** Anything that survives both
   passes still marked `unknown` becomes a focused question
   DM0 surfaces via the `ask_user` tool. The descriptor
   records the location so the question is concrete
   ("table at page 73 — is this an interrupt table or a
   memory-fault table?"), not free-form.

This shape has three practical wins:

1. The first-cut classifier is plain Rust, unit-tested
   against fixture column-header strings + section names.
   It gives the same answer every time for the easy cases.
2. The LLM's task is bounded: confirm or correct. A smaller
   model can do critique reliably even when it might miss
   patterns at full-doc classification.
3. The first-cut classifier evolves over time. When the LLM
   keeps correcting the same heuristic miss across specs,
   we promote the correction to a first-cut rule. The LLM's
   work shrinks as the heuristic library grows.

## 4. The structural skeleton (LLM context)

The LLM never sees raw page text. Its inputs are:

- The **first-cut descriptor draft** (its primary subject).
- A compact **structural skeleton** of the document, built
  deterministically from pdf_oxide output:

  ```text
  # DOCUMENT
  total_pages: 95
  font_clusters: [{size: 34.7, freq: 12}, {size: 26.7, freq: 18},
                  {size: 14.7, freq: 8412}, {size: 12.0, freq: 1843}]
  source_kind: pdf
  detected_chrome_repeated_lines: ["RV12 RISC-V 32/64-bit ...",
                                   "https://roalogic.github.io/..."]

  # HEADINGS (font-clustered)
  [L1] p11  cluster=1  bold  "Execution Pipeline"
  [L2] p11  cluster=2  bold  "Instruction Fetch (IF)"
  [L2] p15  cluster=2  bold  "Instruction Pre-Decode (PD)"
  [L1] p25  cluster=1  bold  "Configurations"
  ...

  # TABLES (from extract_tables)
  [T01] p4   5x4   header_row="Level | Encoding | Name | Abbreviation"
                   first_data_row="0 | 00 | User/Application | U"
  [T02] p12  7x4   header_row="Signal | Direction | To/From | Description"
                   first_data_row="if_nxt_pc | to | Bus Interface | Next address ..."
  ...

  # FIGURES (from extract_images + extract_paths)
  [F01] p2   raster=figures/page-002.png  neighbouring_heading="Introduction"
  [F02] p13  raster=figures/page-013.png  neighbouring_heading="Instruction Fetch (IF)"
  ...

  # ACRONYM CANDIDATES (parenthesised first-mentions)
  "Instruction Fetch (IF)"          first@p11  used 47×
  "Control and Status Register (CSR)" first@p43  used 156×
  ...
  ```

The skeleton is **not** prose. Body paragraphs are dropped —
they don't add structural signal and crowd the context window.
Every line carries enough info for the model to confirm or
override the first cut's tag for that element.

For RV12 (95 pages, ~205 sections post-detection) the skeleton
is ~5k tokens. For a 600-page DDR5 spec, ~20-30k. Both fit a
single LLM call with room for the descriptor schema and a
system prompt.

## 5. Descriptor schema (high level)

The formal schema is in Chapter 7 §7.3. Sketch:

```json
{
  "schema_version": 1,
  "model": "claude-sonnet-4-6",
  "prompt_version": "2026-05-19",
  "source_sha256": "<hex>",
  "discovered_at": "<RFC3339>",

  "section_roles": [
    {
      "heading": "Instruction Fetch (IF)",
      "page": 11, "line": 5,
      "font_size": 14.7, "font_weight": "bold",
      "level": 2,
      "spec_md_role": { "kind": "block",
                        "block_name": "Instruction Fetch (IF)" },
      "layer": "micro",
      "rationale": "pipeline-stage acronym pattern;
                    under 'Execution Pipeline' parent"
    },
    ...
  ],

  "tables": [
    {
      "id": "tbl_023",
      "page": 12, "first_line": 17,
      "row_count": 9, "col_count": 4,
      "kind": "signal_table",
      "spec_md_target": { "kind": "block_signals",
                          "block_name": "Instruction Fetch (IF)" },
      "column_map": [
        { "source": "Signal",      "canonical": "name" },
        { "source": "Direction",   "canonical": "direction" },
        { "source": "To/From",     "canonical": "peer" },
        { "source": "Description", "canonical": "description" }
      ],
      "wrap_strategy": "merge_continuation_rows",
      "rationale": "column headers match signal-table convention;
                    sits under IF block section"
    },
    ...
  ],

  "figures": [...],
  "glossary": [...],
  "chrome": [...],
  "validation": {
    "section_roles_assigned": 184,
    "tables_classified": {"signal_table": 6, "csr_table": 31, ...},
    "tables_unknown": 0,
    "glossary_entries": 23,
    "chrome_lines_stripped": 312,
    "warnings": []
  }
}
```

Key shape decisions:

- Tables carry **locations** (page, first_line), not detection
  regexes. Detection is pdf_oxide's job; the descriptor only
  carries classification and the column-map.
- Sections carry **spec_md_role** mapping straight to spec_md's
  section types. The pipeline doesn't infer the role at every
  consumer; it's fixed once at discovery time.
- The `validation` block is filled in by the deterministic
  post-pass. Zero-row classified tables, unresolved acronyms,
  and over-matching chrome regexes surface as warnings — not
  fatal errors; the descriptor stands and the user can
  hand-edit or `--rediscover-format`.

## 6. Caching and reproducibility

`format.json` is content-addressed by
`(source_sha256, model_id, prompt_version)`. The pipeline reads
the cached file if it exists and the key matches; otherwise it
runs discovery and writes a fresh one. The
`--rediscover-format` flag forces a re-discovery. The
`--format <path>` flag uses an externally-authored descriptor
(escape hatch for testing + offline + manual override).

`format.json` is **committed** alongside `manifest.toml` for
reproducibility. Re-running `sim-flow ingest` on a different
machine pointing at the same source PDF produces the same
`format.json` (cache hit) → the same chunks, tables, and
figures.

Hand-edits are supported. The `source_sha256` and `model`
fields stay valid; the deterministic post-pass re-runs every
classification against the current document and updates the
hit counts, so reviewers can see whether the edit broke
anything.

## 7. Offline / no-LLM fallback

The pipeline must still run when no LLM is available:

- `--no-format-discovery` skips the LLM call and ships the
  first-cut descriptor as-is. For specs that match the
  heuristic library cleanly (markdown-friendly inputs,
  well-conventioned PDFs), this is good enough to produce
  the existing chunks/tables behaviour.
- `--format <path>` uses a hand-authored descriptor.
- A cached `format.json` is reused without any LLM call.

The unit + integration tests always pass `--format <fixture>`
or `--no-format-discovery` so CI is deterministic and does not
require an LLM endpoint.

## 8. Information categories the cycle-accurate model needs

(Settled with user on 2026-05-19.) A cycle-accurate model
authored from a spec needs the following information categories.
This list is what `format.json`'s section/table/figure tagging
must cover, and what spec_md must be able to express:

- External interfaces (`ExternalInterface` ✓)
- Internal interfaces per block (`Block.signals` ✓)
- Control & Status Registers — bit fields included (**new section**)
- Address map (`MemoryRegion` ✓)
- Register files (**new section** or extended `Parameter`)
- Memories / SRAM (`MemoryRegion` + `Parameter` ✓)
- Functional decomposition (`Block[]` ✓)
- Pipelining (`PipelineAndHierarchy` + `Block[]` ✓)
- State machines (`StateMachine` ✓)
- Control vs datapath per signal (**new tag** on `BlockSignalRow`)
- Architectural vs micro-arch per section/block (**new tag**)
- Errors / exceptions (`ErrorEntry` ✓)
- Connectivity (`Connectivity` ✓)
- Timing / throughput (`LatencyRow` ✓)
- Reset / init / flush / drain (`ResetInitFlushDrain` ✓)
- Acronyms / glossary (**new section**)
- Clock domains (**new section** + per-Block / per-signal refs)
- Power domains (**new section** + per-Block refs)
- Reset domains (**new section** + per-Block refs)
- Security boundaries / privilege levels (**new section**)
- Numerical conventions — Q-format, saturation, signed/unsigned (**new section**)
- Performance counters / PMU events (**new section** or part of CSRs)

Out of scope for cycle-accurate v1: debug / test infrastructure
(JTAG, scan, ATPG, trace ports). We intentionally do not model
these in spec_md.

For each category, `format.json` records where the information
lives in the source spec. The vector DB (Chapter 3 lance index)
indexes chunks tagged with the same spec_md roles so the agent
can retrieve cross-references by category, not just by text
similarity.

## 9. Tradeoffs (honest)

| Pro | Con |
| --- | --- |
| One adaptive entry point replaces N regex heuristics that grow forever | Adds an LLM dependency to a previously-deterministic pipeline (mitigated: cached; `--no-format-discovery` fallback for CI / offline) |
| Each spec gets a descriptor that's hand-editable | LLM output may be wrong on edge cases; deterministic post-pass validates and warns |
| Format descriptor is the contract; extractors stay simple and stable | Schema drift between LLM versions; `schema_version` + `prompt_version` in the file |
| pdf_oxide does the structural heavy lifting; LLM only does semantic classification | The first-cut classifier needs ongoing maintenance as new specs surface novel column-header phrasings |
| Recovers structured table extraction without re-introducing per-spec Rust heuristics | Chrome detection straddles positional (Y-band) + textual (LLM regex); two-mechanism overlap to resolve at integration time |

## 10. Open questions

- **Which LLM is the default?** Sonnet-4-6 is the obvious
  choice for quality/cost. Local vLLM (Qwen3) is plausible
  because the critique-pass framing makes the task small.
  Worth a shoot-out once §3-§5 are implemented.
- **Do we let the LLM emit Rust-style regex or PCRE for
  chrome rules?** Rust `regex` crate doesn't support
  lookbehind / backreferences. Force the model to a subset
  the crate can compile, validate at load time, fail fast.
- **What about peer specs?** Manifest supports multiple
  ingested specs (primary + peers). One `format.json` per
  spec — peers from the same vendor may overlap heavily but
  the validation block makes drift obvious.
- **Telemetry.** Per-spec ingest, what does the orchestrator
  surface in the chat panel? At minimum: "discovered format
  from `{model}`, `{N}` signal tables / `{M}` params / `{K}`
  figures extracted; validation hit counts: ..."; warnings
  list.
- **Format-validity gate.** Should DM0's gate-check assert
  at least some classified tables exist when `source_kind =
  "pdf"` and the spec describes hardware? Chapter 6's gate
  is the right place for that check; wire as a follow-up.

## 11. Closure

If we agree on the shape, the formal contract is Chapter 7
([07-spec-format-discovery.md](../architecture/07-spec-format-discovery.md))
and the implementation plan is Phase 9
([09-phase-format-discovery.md](../plan/09-phase-format-discovery.md))
with 16 milestones spanning spec_md extensions, descriptor
types, first-cut classifier, LLM critique pass, validation,
CLI integration, structured-spans loading / parse / classify,
chrome, emit chunk tagging, DM0 auto-populate over
`format.json`, lance index column additions, and integration
tests on all four reference specs.
