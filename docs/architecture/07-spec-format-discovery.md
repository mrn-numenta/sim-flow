# Chapter 7: Spec Format Discovery

This chapter specifies the pre-pass that turns a source PDF
into a **semantic map** — `format.json` — driving downstream
chunking, table classification, and DM0 auto-populate. It
replaces the regex-anchored heuristic table extractors from
Chapter 1 §1.5 with a layout-aware pipeline built on
`pdf_oxide`, a deterministic Rust first-cut classifier, and a
small LLM critique pass that refines the first cut.

## 7.1 Purpose

The pipeline before this chapter converts a PDF into chunks +
tables + figures by running pattern-matching against the raw
text dump. Phase 2's RV12 trial (see
`docs/brainstorming/spec-format-discovery.md`) exposed two
problems with that approach:

1. Heading detection by regex on flat text mis-classifies
   whitespace-aligned table rows (`3   11   Machine   M`) as
   level-1 numbered headings and silently drops real
   plain-text headings (`Privilege Levels`,
   `Instruction Fetch (IF)`). Every heading regex tweak that
   fixed one false positive surfaced another, and the heuristic
   stack grew unbounded.
2. Table extraction by header-line anchor regex
   (`^Signal\s+Direction\s+To/From\s+Description$`) fails the
   moment the source PDF uses different column words or
   pdf_oxide reshapes the text into per-column streams. Adding
   new anchor regexes per spec is the same dead-end loop.

This chapter formalises the alternative: pdf_oxide handles
heading detection (font-size clustering) and table detection
(spatial projection) deterministically; a Rust first-cut
classifier labels each detected element with a provisional
spec_md role using heuristics over column-header keywords +
heading patterns; an LLM critique pass refines the first cut
where heuristics returned `unknown` or got it wrong; the
result is `format.json`, a content-addressed descriptor that
the rest of the pipeline consumes.

## 7.2 Position in the pipeline

```text
loading        →  per-page { spans, tables, paths, images }
                  via pdf_oxide structured API
                  (no markdown intermediate)

format         →  format.json
discover           - section roles
                   - table kinds + column maps
                   - figure kinds
                   - glossary entries
                   - chrome regexes
                   - validation block

parse          →  SectionTree from font-clustered spans,
                  page-numbered natively, role-tagged from
                  format.json

classify       →  emits typed tables (TOML rows) for each
                  classified table using pdf_oxide's cells +
                  format.json's column map

chrome-strip   →  positional (bbox top/bottom-of-page) + LLM
                  regexes from format.json

emit           →  per-section chunks (Markdown for retrieval),
                  typed tables (TOML), figure rasters (PNG),
                  manifest.toml
```

The new `format` stage runs once per (source-SHA, model,
prompt-version). Internally it runs in four phases (§7.4):
pdf_oxide detection, Rust first-cut classifier, LLM critique,
and a deterministic validation post-pass. On cache hit the
file is reused unchanged. On cache miss the LLM critique pass
emits a single request whose primary input is the first-cut
descriptor draft, with a compact **structural skeleton** of
the document supplied alongside for context (heading list,
table-header strings, image references, glossary candidates) —
never raw page samples. See §7.6 for the skeleton's shape.

## 7.3 `format.json` schema

`format.json` lives at
`<project>/.sim-flow/spec-ingest/format.json`. Encoded JSON; the
schema is versioned and includes the LLM model + prompt version
used to derive it.

### 7.3.1 Top-level

```json
{
  "schema_version": 1,
  "model": "claude-sonnet-4-6",
  "prompt_version": "2026-05-19",
  "source_sha256": "<hex>",
  "discovered_at": "<RFC3339>",
  "section_roles": [...],
  "tables": [...],
  "figures": [...],
  "glossary": [...],
  "chrome": [...],
  "validation": {...}
}
```

### 7.3.2 `section_roles[]`

One entry per detected section heading. Each entry carries the
heading text, its page + line origin, an inferred level, and a
spec_md role. Roles are an enumerated set tied to spec_md
sections (Chapter 2):

```json
{
  "heading": "Instruction Fetch (IF)",
  "page": 11,
  "line": 5,
  "font_size": 14.7,
  "font_weight": "bold",
  "level": 2,
  "spec_md_role": {
    "kind": "block",
    "block_name": "Instruction Fetch (IF)"
  },
  "layer": "micro",
  "rationale": "matches pipeline-stage acronym pattern; appears under 'Execution Pipeline' parent"
}
```

`spec_md_role.kind` values:

- `metadata` (cover, version, ownership)
- `assumptions` (quantitative + qualitative)
- `external_interfaces`
- `block` — with `block_name`
- `parameters`
- `csrs` — first-class section for control/status registers
- `csr_fields` — bit-field subsection for a parent CSR
- `register_files`
- `memory_map`
- `state_machines`
- `encodings`
- `connectivity`
- `errors`
- `functional_behavior`
- `timing_and_throughput`
- `pipeline_and_hierarchy`
- `reset_init_flush_drain`
- `worked_examples`
- `glossary`
- `clock_domains`
- `power_domains`
- `reset_domains`
- `security_boundaries`
- `numerical_conventions`
- `performance_counters`
- `prose` — generic narrative not pinned to a typed section
- `unknown` — neither the first-cut classifier nor the LLM
  critique committed to a role; DM0 surfaces this via
  `ask_user` (§7.4 item 4)

`layer` ∈ `{ "architectural", "micro", "mixed", "unknown" }`.
Architectural sections describe software-visible behavior
(registers, instructions, privilege model); micro sections
describe implementation (bypass paths, cache geometry). The
distinction drives chunk tags so a query like "what's
software-visible about the IF stage?" filters away
implementation prose.

### 7.3.3 `tables[]`

One entry per detected table. The detection itself is
deterministic from `pdf_oxide::PdfDocument::extract_tables(page)`;
the first-cut classifier assigns `kind`, `spec_md_target`,
`column_map`, and `wrap_strategy` from column-header
heuristics where they fire; the LLM critique pass refines or
overrides entries where the first cut returned `unknown` or
got the classification wrong.

```json
{
  "id": "tbl_023",
  "page": 12,
  "first_line": 17,
  "row_count": 9,
  "col_count": 4,
  "kind": "signal_table",
  "spec_md_target": {
    "kind": "block_signals",
    "block_name": "Instruction Fetch (IF)"
  },
  "column_map": [
    { "source": "Signal", "canonical": "name" },
    { "source": "Direction", "canonical": "direction" },
    { "source": "To/From", "canonical": "peer" },
    { "source": "Description", "canonical": "description" }
  ],
  "wrap_strategy": "merge_continuation_rows",
  "rationale": "column headers match signal-table convention; sits under IF block section"
}
```

Recognised `kind` values:

- `signal_table` (per-block I/O)
- `external_signal_table`
- `parameter_table`
- `csr_table` (register address + summary)
- `csr_field_table` (bit fields for one register)
- `register_file_table`
- `memory_map_table`
- `encoding_table`
- `error_table`
- `fsm_state_table`
- `fsm_transition_table`
- `latency_table`
- `connectivity_table`
- `pmu_event_table`
- `unknown` — neither first-cut nor LLM critique committed;
  falls through to ask_user at DM0 time

`wrap_strategy` ∈
`{ "single_row", "merge_continuation_rows", "join_on_blank_first_col" }`
tells classify.rs how to coalesce pdf_oxide's per-cell rows when
multi-line cells wrap into separate `TableRow`s.

`column_map.canonical` values are tied to the row schema of
spec_md's matching section. The deterministic post-pass
validates that every `canonical` name appears in the target's
row schema and rejects descriptors that try to map a column to
a field that doesn't exist.

### 7.3.4 `figures[]`

```json
{
  "id": "fig_005",
  "page": 13,
  "kind": "block_diagram",
  "rasterized_to": "figures/page-013.png",
  "spec_md_target": {
    "kind": "block_diagram",
    "block_name": "Instruction Fetch (IF)"
  },
  "referenced_acronyms": ["IF", "PD", "ID"],
  "rationale": "vector-path page with stage labels; first figure under IF section"
}
```

`kind` values:

- `block_diagram`
- `state_diagram`
- `timing_diagram`
- `memory_map_diagram`
- `connectivity_topology`
- `pipeline_diagram`
- `generic`

Figures are rasterized regardless of classification (pdf_oxide
renders the page at the configured DPI per Chapter 1 §1.5).
Classification tags the figure for retrieval and for DM0
auto-populate's figure references.

### 7.3.5 `glossary[]`

```json
{
  "acronym": "IF",
  "expansion": "Instruction Fetch",
  "first_page": 11,
  "scope": "spec",
  "used_in_blocks": ["Instruction Fetch (IF)"],
  "source": "parenthesised_first_mention"
}
```

`source` ∈ `{ "parenthesised_first_mention", "glossary_section",
"user_added" }`. Acronyms feed both the spec_md `Glossary`
section and the chunk tags (a chunk that mentions `IF` carries
`acronyms_referenced: ["IF"]` for retrieval).

### 7.3.6 `chrome[]`

```json
{
  "regex": "^RV12 RISC-V.*\\d+/\\d+/\\d+.*$",
  "kind": "running_header",
  "y_band_pt": [766.0, 774.0],
  "match_count": 95
}
```

Chrome detection is hybrid: positional (running headers always
live in a Y-band near the page top or bottom, derived from
pdf_oxide span bboxes) and textual (repeated lines collected
by the existing chrome-strip stage and refined by the first-cut
classifier). The LLM critique pass adds or corrects regexes for
repeated lines that the first cut missed; the deterministic
chrome-strip stage applies both positional and regex filters.

`kind` values: `running_header`, `running_footer`, `page_number`,
`footer_link`, `watermark`.

### 7.3.7 `validation`

Filled by the deterministic post-pass that runs every
classification (column-map projection, chrome regex match,
section-role uniqueness) against the full document.

```json
{
  "section_roles_assigned": 184,
  "tables_classified": {
    "signal_table": 6,
    "parameter_table": 2,
    "csr_table": 31,
    "memory_map_table": 1
  },
  "tables_unknown": 0,
  "glossary_entries": 23,
  "chrome_lines_stripped": 312,
  "warnings": [
    { "code": "wrap_strategy_zero_merges", "table_id": "tbl_011" }
  ]
}
```

Validation warnings surface in the CLI output and in DM0's
diagnostic stream. Zero-merge wrap strategies, missing
canonical columns, and unmatched chrome regexes are all
recoverable by hand-edit or `--rediscover-format`.

## 7.4 Decision policy

The descriptor draws from four sources of truth, in order:

1. **Deterministic detection** (pdf_oxide structured output):
   table detection, font-clustered heading list, image / path
   detection, span bboxes for chrome banding. No LLM, no
   user. Always runs.
2. **Deterministic first-cut classifier** (heuristics over
   pdf_oxide output): assigns provisional `spec_md_role`,
   `kind`, `column_map`, `layer`, etc. by matching against
   known patterns (column-header keyword sets, section-heading
   regex families, acronym shapes). Always runs. Cheap,
   testable, predictable; gets the easy cases.
3. **LLM critique pass at format-discovery time** (one call
   per spec): shown the deterministic first cut + the
   structural skeleton, asked to flag/refine. The LLM does
   NOT re-label from scratch; it emits an **adjustments
   patch** against the first cut. Cases the first cut nailed
   pass through with the LLM's confirmation; cases it got
   wrong (or marked `unknown`) get LLM-revised tags. Token
   cost scales with the count of ambiguous cases, not the
   document size.
4. **User via `ask_user` at DM0 time**: anything that survives
   the first cut + LLM critique still marked `unknown`. The
   descriptor records each `unknown` slot's location so DM0
   surfaces a focused question, not a free-form prompt.

The "decide up-front vs defer" rule:

- **First cut decides** when the input has a single
  interpretation under known patterns: column header
  `Signal Direction To/From Description` is a signal_table;
  section heading `Instruction Fetch (IF)` matches the
  stage-acronym pattern; column header `Parameter Type
  Default Description` is a parameter_table; `\b\w+ \([A-Z]+\)\b`
  is an acronym candidate.
- **LLM critique decides** when interpretation requires
  reading column words or section names that vary across
  specs and don't match a known pattern: a `csr_table` with
  novel column phrasing; a section heading that could be
  either a block or a memory map; a table whose first cut
  flagged `unknown` because column headers didn't match
  anything in the heuristic library.
- **User decides** when both passes fail to commit.

This two-pass shape has three practical wins:

1. The first-cut classifier is plain Rust code, unit-tested
   against fixture column-header strings. It gives the same
   answer every time for the easy cases.
2. The LLM input becomes a labelled descriptor draft, not a
   raw skeleton. The model's job is critique, not
   classification — a smaller, more constrained task that
   smaller models can handle reliably.
3. When the first cut is wrong, the LLM's correction is
   visible in the diff between first cut and final
   descriptor (preserved in the `validation` block). The
   first-cut classifier evolves over time by absorbing
   patterns the LLM keeps correcting.

## 7.5 CLI surface

```text
sim-flow ingest <source>
sim-flow ingest --rediscover-format <source>
sim-flow ingest --format <path-to-format.json> <source>
sim-flow ingest --no-format-discovery <source>
```

- Default: discover if `format.json` is missing or its
  `source_sha256` doesn't match the current input; otherwise
  reuse.
- `--rediscover-format`: force a fresh LLM call. Caches the new
  descriptor.
- `--format <path>`: use a hand-authored descriptor; skip
  discovery entirely. Escape hatch for testing + offline cases.
- `--no-format-discovery`: skip the LLM critique pass and
  ship the first-cut descriptor as-is. The pipeline still
  runs pdf_oxide detection and the Rust first-cut classifier
  — they're deterministic and don't depend on an LLM
  endpoint. Useful for CI, offline development, and specs
  whose first cut already matches the heuristic library
  cleanly.

The format-discovery LLM model is configurable via
`.sim-flow/config.toml`'s existing LLM-config block; default is
`claude-sonnet-4-6`. Local vLLM endpoints work; the LLM client
is the same one Chapter 5 (rig) wraps.

## 7.6 The two-pass classifier (LLM input)

The LLM never sees raw page text. It sees the **first-cut
descriptor draft** plus the structural skeleton it was derived
from. The deterministic first-cut classifier runs before the
LLM call:

```text
pdf_oxide outputs
   ↓
first-cut classifier (Rust heuristics):
   - section_roles: pattern-match heading text against known
     spec_md section names (e.g. "Memory Map" → memory_map,
     "CSR Listing" → csrs, "Glossary" → glossary, "<Name> (<ACR>)"
     → block:<Name>, "Configurations / Core Parameters" →
     parameters, etc.)
   - tables: match column-header strings against a heuristic
     library (Signal/Direction → signal_table; Parameter/Type/
     Default → parameter_table; Address/Name/Reset → csr_table;
     Bit*/Field/Description → csr_field_table; etc.)
     Tables that don't match any heuristic get kind="unknown".
   - figures: classify by neighbouring heading + path/image
     count (block_diagram if vector-rich + adjacent to a Block
     heading; state_diagram if adjacent to a StateMachine
     heading; etc.)
   - chrome: emit positional Y-band rules from running-line
     repetition.
   - glossary: parenthesised first-mention regex; later-usage
     count.
   ↓
first-cut descriptor (+ skeleton for LLM context)
   ↓
LLM critique pass:
   - confirm/correct each first-cut tag
   - resolve "unknown" entries
   - flag inconsistencies (two sections both claiming the
     `csrs` role, etc.)
   ↓
final descriptor → format.json
```

The structural skeleton (skeleton.txt fed alongside the
first-cut draft) is built deterministically from pdf_oxide
output:

```text
# DOCUMENT
total_pages: 95
font_clusters: [{size: 34.7, freq: 12}, {size: 26.7, freq: 18},
                {size: 14.7, freq: 8412}, {size: 12.0, freq: 1843}]
source_kind: pdf
detected_chrome_repeated_lines: ["RV12 RISC-V 32/64-bit ...",
                                 "https://roalogic.github.io/..."]

# HEADINGS (font-clustered)
[L1] p11 "Execution Pipeline"  (size=26.7, bold)
[L2] p11 "Instruction Fetch (IF)"  (size=14.7, bold)
[L2] p15 "Instruction Pre-Decode (PD)"  (size=14.7, bold)
[L1] p25 "Configurations"  (size=26.7, bold)
[L2] p25 "Core Parameters"  (size=14.7, bold)
...

# TABLES (extract_tables)
[T01] p4 5x4 "Level | Encoding | Name | Abbreviation"
        first row: "0 | 00 | User/Application | U"
[T02] p12 7x4 "Signal | Direction | To/From | Description"
        first row: "if_nxt_pc | to | Bus Interface | Next address ..."
[T03] p25 13x4 "Parameter | Type | Default | Description"
        first row: "JEDEC_BANK | Integer | 0x0A | JEDEC Bank"
...

# FIGURES
[F01] p2 page-002.png   neighbouring_heading: "Introduction"
[F02] p13 page-013.png  neighbouring_heading: "Instruction Fetch (IF)"
...

# ACRONYM CANDIDATES (parenthesised first-mentions)
"Instruction Fetch (IF)" @ p11 — uses 47 times after
"Pre-Decode (PD)" @ p15 — uses 22 times after
"Control and Status Register (CSR)" @ p43 — uses 156 times after
...
```

For RV12 (95 pages, 205 sections after detection) the skeleton
is ~5k tokens. For DDR5-scale 600+ page specs perhaps 20-30k.
Both fit a single LLM call with room for system prompt + the
first-cut descriptor + the adjustments-patch schema.

The LLM's task is **critique**, not classification: for each
first-cut entry, either accept it (no patch entry) or override
the tag with rationale. The model may also resolve entries the
first cut marked `unknown` and flag inconsistencies (two
sections both claiming the same `csrs` role, etc.). The output
is an adjustments patch the post-pass applies to the first cut
to produce the final descriptor. The prompt constrains the
patch to the §7.3 schema; the deterministic post-pass validates
the merged descriptor before writing `format.json`.

## 7.7 spec_md extensions

The information categories driving cycle-accurate model
authoring (see brainstorm §8) require these additions to spec_md
(see Chapter 2):

- **`csrs`** — `Csr { address, name, access, reset_value,
  privilege_required, description, fields: Vec<CsrField {
  bits, name, access, description }> }`. Replaces the
  ad-hoc Parameter / Encoding overload for register
  documentation.
- **`glossary`** — `GlossaryEntry { term, expansion, scope,
  used_in_blocks: Vec<String> }`. First-class for retrieval.
- **`layer` tag** on `Block` and `Section` — `Architectural |
  Micro | Mixed | Unknown`. Drives chunk tagging for
  retrieval filtering.
- **`role` tag** on `BlockSignalRow` — `Control | Data |
  Status | Unknown`. Set by classify.rs from naming-pattern
  heuristics where unambiguous; LLM-assigned at
  format-discovery for novel cases; user-confirmed at DM0
  time when both fail.
- **`clock_domains`** — `ClockDomain { name, frequency,
  source, description }`. Per-Block `clock_domain: String`
  reference.
- **`power_domains`** — `PowerDomain { name, voltage,
  always_on, description }`. Per-Block `power_domain: String`.
- **`reset_domains`** — `ResetDomain { name, polarity, sync,
  source, description }`. Per-Block `reset_domain: String`.
- **`security_boundaries`** — `PrivilegeLevel { id, name,
  description, capabilities }`. Per-CSR / per-MemoryRegion
  `required_privilege: String` reference.
- **`numerical_conventions`** — `NumericalConvention {
  q_format_default, saturation_policy, signed_default,
  rounding_mode, description }`. Per-signal / per-parameter
  optional `numeric_type: { width, signed, q_format }` override.
  Relevant for HTM/SP-style numerical specs.
- **`performance_counters`** — `PmuEvent { id, name,
  description, csr_address }`. Per-PMU-event entry; ties into
  `csrs` via `csr_address` when the counter is read through a
  CSR.

Out of scope for v1: debug/test infrastructure (JTAG, scan,
ATPG, trace). spec_md will not model these.

The extensions are additive to Chapter 2's schema; the parser
in Phase 1 accepts the new sections as optional. DM0's gate
treats `csrs` and `glossary` as REQUIRED only when the
auto-populate found CSR-like tables or parenthesised
acronyms — i.e., the requirement is conditional on what the
ingest discovered in the source.

## 7.8 Vector DB chunk tagging

Each chunk emitted by the `emit` stage carries the format.json
role tags so the lance index (Chapter 3) can answer queries by
semantic role:

```toml
chunk_id = "..."
breadcrumb = ["Execution Pipeline", "Instruction Fetch (IF)"]
section_heading = "Instruction Fetch (IF)"
source_page_range = [11, 14]
kind = "block"
spec_md_role = "block:Instruction Fetch (IF)"
layer = "micro"
acronyms_referenced = ["IF", "PC", "PD"]
contained_signal_tables = ["tables/signals/000-instruction-fetch-if.toml"]
contained_csr_tables = []
contained_figures = ["figures/page-013.png"]
contained_table_refs = []
clock_domain = "core_clk"
power_domain = "core_pd"
```

Two retrieval modes work in parallel:

1. **Semantic-role lookup**: agent queries "the IF block's
   signals" → indexed by `spec_md_role = "block:Instruction
   Fetch (IF)"`, returns the precisely-tagged chunk.
2. **Text similarity (existing)**: agent queries "how does
   branch prediction interact with the fetch stage" →
   cross-references retrieved by text similarity from chunks
   that mention "branch prediction" + "fetch", regardless of
   their primary section role.

Both indices reuse the same chunks; tagging happens once at
ingest time. See Chapter 3 §3.x for the field additions to the
spec_chunks lance table.

## 7.9 Validation

The post-pass runs every classified pattern against the full
document and surfaces:

- **Zero-row tables**: a table classified as kind X whose
  rows don't parse against the column_map → warning; falls
  back to `kind: "unknown"` and surfaces via ask_user at DM0.
- **Heading role collisions**: two sections both classified
  as `csrs` when there should be one → warning; the
  descriptor is flagged for hand-edit or
  `--rediscover-format` (which re-prompts the LLM with the
  conflict surfaced in the first-cut input).
- **Unresolved acronyms**: an acronym appears in a chunk but
  is not in glossary → warning; surfaces via ask_user.
- **Chrome regex over-match**: a regex matches more than 80%
  of lines on a page → warning; descriptor is suspicious.
- **Wrap strategy never fires**: a table's wrap_strategy is
  set to `merge_continuation_rows` but no continuation rows
  exist → warning; harmless but flags LLM error.

Validation results land in `format.json::validation` and in the
CLI's stderr output. They are advisory, not fatal: the pipeline
continues with the descriptor as written. Fatal errors (parse
failure, schema mismatch) abort the ingest.

## 7.10 Out of scope (this chapter)

- The LLM prompt text itself (lives at
  `tools/sim-flow/prompts/format-discovery.md` per Chapter 6
  prompt-loader conventions; revisable without schema changes).
- Specific LLM model selection or runtime knobs (Chapter 5
  governs that).
- The vector-DB schema field additions (Chapter 3).
- Per-spec descriptor authoring tooling (`sim-flow ingest
  format edit <field>` CLI — possible follow-up; v1 ships with
  hand-edit-the-JSON).
- Cross-spec descriptor merging (peer specs each get their own
  `format.json`; correlating across them is a future feature).
- Re-ingestion strategy on format change (today: user runs
  `--rediscover-format`; tomorrow maybe a watch loop). Out of
  scope here.
