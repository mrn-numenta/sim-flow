# Phase 9: Spec Format Discovery + Structured-Spans Pipeline

## Goal

Replace the markdown-intermediate ingest pipeline with a
structured-spans pipeline driven by `pdf_oxide`'s structured
API and a `format.json` semantic descriptor (Architecture
Chapter 7). Extend spec_md (Chapter 2) with the categories a
cycle-accurate model needs but the current schema doesn't
express: CSRs, glossary, layer / role / domain tags, security
boundaries, numerical conventions, PMU events. Re-wire DM0
auto-populate (Phase 6) to consume `format.json` directly so
classified tables land in the right spec_md sections without
re-classification at every consumer.

The acceptance gate is: ingest the four reference specs (RV12,
Numenta SoC, Apical NoC, Spatial Pooler), produce non-empty
classified tables, glossary entries, and chunks tagged with
spec_md roles; DM0 auto-populate populates the expected sections
on RV12 end-to-end without further LLM calls.

## Inputs

- Architecture Chapter 7
  ([07-spec-format-discovery.md](../architecture/07-spec-format-discovery.md))
  for the descriptor schema, decision policy, and CLI surface.
- Architecture Chapter 2 for the existing spec_md schema (we
  extend, do not replace).
- Brainstorm
  ([spec-format-discovery.md](../brainstorming/spec-format-discovery.md))
  §11 (pdf_oxide capabilities) and §12 (information categories).
- Phase 1 output: `SpecMd` types + parser + writer + traversal +
  validator.
- Phase 2 output: spec_ingest pipeline (loading / chrome /
  parse / classify / references / figures / emit).
- Phase 5 output: agent tools incl. retrieval + `ask_user`.
- Phase 6 output: DM0 module incl. auto_populate + qa_loop +
  gate.
- The pdf_oxide swap commit (`d611b7f`) and the libpdfium
  cleanup commit (`ee95bc4`).

## Outputs

- Extended spec_md types under
  `src/__internal/session/spec_md/types.rs`:
  - `Csr` + `CsrField` (replacing ad-hoc Parameter/Encoding use).
  - `GlossaryEntry`.
  - `ClockDomain`, `PowerDomain`, `ResetDomain`.
  - `PrivilegeLevel` (security boundaries).
  - `NumericalConvention`.
  - `PmuEvent`.
  - New optional fields on existing types: `Block.layer`,
    `Block.clock_domain`, `Block.power_domain`,
    `Block.reset_domain`, `BlockSignalRow.role`,
    `MemoryRegion.required_privilege`.
- New module `src/__internal/session/spec_ingest/format/`:
  - `mod.rs` — public surface.
  - `descriptor.rs` — the `FormatJson` struct hierarchy.
  - `skeleton.rs` — the structural-skeleton builder.
  - `first_cut.rs` — the deterministic heuristic classifier.
  - `discover.rs` — the LLM critique pass.
  - `validate.rs` — the deterministic post-pass that fills the
    `validation` block + emits warnings.
  - `default.rs` — the built-in markdown-friendly default
    descriptor for `--no-format-discovery` / CI.
- Rewritten `loading.rs` that emits per-page `PageLayout {
  spans, tables, paths, images }` instead of markdown strings.
- Rewritten `parse.rs` that builds `SectionTree` by font
  clustering on spans (no markdown round-trip; page numbers
  natively associated).
- Rewritten `classify.rs` that consumes `format.json`'s table
  classifications and `extract_tables` output to emit typed
  spec_md rows.
- Updated `chrome.rs` that uses positional Y-banding plus the
  descriptor's chrome regexes.
- CLI: `--rediscover-format`, `--format <path>`,
  `--no-format-discovery` flags on `sim-flow ingest`.
- Lance index schema additions (per Architecture Chapter 3
  §3.x): chunk-level `spec_md_role`, `layer`,
  `acronyms_referenced`, `clock_domain`, `power_domain`,
  `reset_domain` columns.
- Phase 6 DM0 auto-populate wired through `format.json`:
  `populate_*` reads the descriptor's `tables[].spec_md_target`
  instead of inferring kind from heading text.
- Format-discovery LLM prompt at
  `prompts/format-discovery.md`.
- Tests + fixtures + integration tests on all four reference
  specs.

## Acceptance Gate

- [x] `cargo build --package sim-flow` succeeds.
- [x] `cargo test --package sim-flow --lib` passes; the new
      `spec_ingest::format` module has its own unit tests.
- [x] `cargo clippy --package sim-flow --all-targets --all-features -- -D warnings` clean.
- [x] `sim-flow ingest --no-format-discovery <markdown-spec>`
      produces the same chunks/tables as today on a markdown
      input.
- [x] `sim-flow ingest <pdf>` discovers `format.json`, validates
      cleanly, produces non-empty `tables_classified` and
      `glossary_entries` on each of the four reference specs.
- [x] `format.json` round-trips: a cached descriptor + the same
      source PDF produces identical chunks across machines.
- [x] DM0 auto-populate on the RV12 fixture (Phase 6 milestone
      6.12 integration test) populates `blocks[]`, `csrs[]`,
      `memory_regions[]`, and `glossary[]` from `format.json`.
- [x] Lance index build (`sim-flow build-spec-index`) writes the
      new chunk columns; existing semantic-search smoke tests
      pass.

## Milestones

### Milestone 9.1: spec_md type extensions

- [x] Add `Csr` + `CsrField` types to
      `spec_md/types.rs`. Top-level `SpecMd.csrs: Vec<Csr>`.
- [x] Add `GlossaryEntry` + `SpecMd.glossary: Vec<GlossaryEntry>`.
- [x] Add `ClockDomain` + `SpecMd.clock_domains:
      Vec<ClockDomain>`. Same for `PowerDomain` /
      `ResetDomain` / `PrivilegeLevel`.
- [x] Add `NumericalConvention` + `SpecMd.numerical_conventions:
      Vec<NumericalConvention>` (single entry usual; vec
      supports per-block overrides).
- [x] Add `PmuEvent` + `SpecMd.performance_counters:
      Vec<PmuEvent>`.
- [x] Add `layer: Layer` (`Architectural | Micro | Mixed |
      Unknown`) to `Block` and a section-level annotation on
      `SectionTree` nodes.
- [x] Add `role: SignalRole` (`Control | Data | Status |
      Unknown`) to `BlockSignalRow`.
- [x] Add `clock_domain`, `power_domain`, `reset_domain` as
      optional refs on `Block`.
- [x] Add `required_privilege` ref on `MemoryRegion` and `Csr`.
- [x] Update parser + writer for every new section /
      annotation. Default values for new optional fields ensure
      round-trip stability with old spec.md files.
- [x] Update `missing_required_fields()` traversal so the new
      sections surface as MissingFields only when the spec's
      ingest discovered relevant tables (conditional
      requirement).
- [x] Unit tests on each new type: parse + write + round-trip.

Gate: spec_md tests pass; round-trip on a fixture with every
new section round-trips byte-equal at the parse level.

### Milestone 9.2: format.json descriptor types

- [x] `format/descriptor.rs`: define `FormatJson`,
      `SectionRole`, `TableEntry`, `FigureEntry`,
      `GlossaryEntry`, `ChromeEntry`, `ValidationBlock` matching
      Chapter 7 §7.3 schema exactly.
- [x] Serde (de)serialisers; schema_version pinned to 1.
- [x] `FormatJson::load(path)` / `FormatJson::write(path)`.
- [x] `FormatJson::content_key()` returns
      `(source_sha256, model, prompt_version)`.
- [x] Unit tests on a hand-authored fixture descriptor.

Gate: `cargo test format::descriptor` passes.

### Milestone 9.3: structural-skeleton builder

- [x] `format/discover.rs::build_skeleton(doc: &PdfDocument) ->
      Skeleton`: per Chapter 7 §7.6 layout — DOCUMENT header
      with font clusters + page count + chrome candidates;
      HEADINGS section sorted by page; TABLES section listing
      each detected table's location + dimensions + first-row
      sample; FIGURES section; ACRONYM CANDIDATES section.
- [x] Heading detection: collect spans across all pages, k-means
      cluster on `font_size + font_weight`, take top N clusters
      as heading candidates, sort line-y within page.
- [x] Table detection: call `doc.extract_tables(page)` for every
      page; emit `(page, first_line, rows×cols, first-row text)`.
- [x] Figure detection: pages with `extract_images()` ≥ 1 OR
      `extract_paths()` above threshold per existing
      figures-stage config; emit `(page, raster path,
      neighbouring_heading)`.
- [x] Acronym candidate detection: regex
      `\b([A-Z][A-Za-z\-]+)\s*\(([A-Z][A-Z0-9]{1,5})\)\b` over
      span text; track first-mention page + later usage count.
- [x] Render `Skeleton` to a deterministic string the LLM
      consumes.
- [x] Unit test against synthetic 5-page PDF fixture.

Gate: skeleton builder produces stable strings on RV12 across
runs (deterministic).

### Milestone 9.4: first-cut classifier (deterministic)

Per Architecture Chapter 7 §7.4 + §7.6, the descriptor pipeline
is two-pass: deterministic first cut then LLM critique. This
milestone builds the first cut.

- [x] `format/first_cut.rs::classify(skeleton, doc) ->
      FormatJson`: maps pdf_oxide output to a provisional
      `FormatJson` using heuristic libraries.
- [x] **Section role heuristics**: match heading text against
      a known-section library (case-insensitive prefix /
      substring): `"Glossary" → glossary`, `"Memory Map" |
      "Address Map" → memory_map`, `"CSR" | "Control and
      Status" → csrs`, `"Pipeline" → pipeline_and_hierarchy`,
      `"Parameters" | "Configurations" → parameters`,
      `"Errors" | "Exceptions" → errors`,
      `"State Machine" | "FSM" → state_machines`,
      `"Reset" | "Initialization" → reset_init_flush_drain`,
      `"Clock" → clock_domains`, `"Power" → power_domains`,
      etc. Acronym-stage pattern (`<Name> (<ACR>)`) → block.
      Unknown → kind="unknown".
- [x] **Table kind heuristics**: column-header keyword sets.
      `["Signal", "Direction", ("To/From"|"From/To"),
      "Description"]` (≥3 match) → signal_table.
      `["Parameter", ("Type"|"Kind"), ("Default"|"Value"),
      "Description"]` → parameter_table.
      `[("Address"|"Offset"), "Name", ("Description"|"Reset")]`
      → csr_table.
      `[("Bits"|"Bit"|"Field"), "Name", "Description"]` →
      csr_field_table.
      `["Region", ("Base"|"Start"), "Size"]` →
      memory_map_table.
      `["State", "Transition", "Next"]` → fsm_transition_table.
      Plus a catalog for encoding / error / latency /
      connectivity / pmu. Unknown → kind="unknown".
- [x] **Column-map heuristics**: for each classified table,
      project source column header words onto canonical
      spec_md row fields using a per-kind word→canonical
      mapping (e.g., signal_table:
      `Signal→name, Direction→direction,
      To/From→peer, Description→description`).
- [x] **Figure kind heuristics**: vector-path-rich page
      adjacent to a `block` section → block_diagram;
      adjacent to a `state_machines` section → state_diagram;
      adjacent to a `connectivity` section →
      connectivity_topology; etc. Default → generic.
- [x] **Glossary heuristics**: regex
      `\b([A-Z][A-Za-z\-]+(?:\s+[A-Z][A-Za-z\-]+)*)\s+\(([A-Z][A-Z0-9]{1,5})\)`
      over span text; first-mention page + later-usage count.
- [x] **Chrome heuristics**: lines repeated on ≥ 60% of pages
      → chrome regex (use existing chrome-strip-stage output);
      lines whose bbox Y is consistently within 5pt of page top
      / bottom → positional chrome rule.
- [x] Unit tests per heuristic with a small fixture library of
      column-header strings + section-heading strings + figure
      adjacency cases.

Gate: first-cut classifier produces a non-empty `FormatJson`
on a hand-authored skeleton fixture and gets ≥ 60% of
tables / sections / figures correctly classified on a
mock-RV12 skeleton (the remaining 40% can be `unknown` —
that's the LLM's job in 9.5).

### Milestone 9.5: format-discovery LLM critique pass

- [x] Prompt at `prompts/format-discovery.md` taking the
      first-cut descriptor + the skeleton, with instructions
      to emit an **adjustments patch** (per-entry overrides
      with rationale), NOT a from-scratch descriptor. Patch
      shape: `[{ id, field, old_value, new_value, rationale }]`.
- [x] `format/discover.rs::critique(first_cut, skeleton, llm)
      -> Result<FormatJson>`: build the prompt, call LLM,
      parse the patch, apply it to `first_cut`, return the
      adjusted descriptor.
- [x] Adjustments policy:
      - LLM may change any `kind` / `spec_md_role` / `layer` /
        `column_map.canonical` field with rationale.
      - LLM may add `unknown` entries it found that the first
        cut missed.
      - LLM may NOT change pdf_oxide-derived facts (page,
        line, row_count, col_count, bbox).
      - Each adjustment carries a `rationale: String` so the
        diff is auditable.
- [x] Error handling: malformed adjustments JSON → one retry
      with the schema-violation feedback; second failure aborts
      with the LLM's raw output captured in
      `format.json::validation.warnings`.
- [x] Unit test with mock LLM returning a known-good patch +
      a malformed patch.

Gate: `critique()` correctly applies a patch fixture to a
first-cut fixture; retry + abort paths covered.

### Milestone 9.5: deterministic validation post-pass

- [x] `format/validate.rs::validate(descriptor, doc) ->
      ValidationBlock`: re-runs every table classification
      against the document, counts non-empty rows, flags
      zero-match warnings; re-runs every section_roles entry,
      checks heading-on-disk matches the descriptor; checks
      column_map canonicals against spec_md schema; checks
      chrome regex match counts.
- [x] Result is appended to `format.json::validation` before
      writing.
- [x] Warning codes (per Chapter 7 §7.9):
      `wrap_strategy_zero_merges`, `csrs_role_collision`,
      `unresolved_acronyms`, `chrome_over_match`,
      `unknown_canonical`.
- [x] Unit tests for each warning code on synthetic descriptors.

Gate: validation tests pass; one descriptor with each warning
code lands a specific warning.

### Milestone 9.6: CLI integration

- [x] `sim-flow ingest` resolves `format.json`:
      - If `--no-format-discovery` → use `default.rs`'s
        built-in markdown-friendly descriptor.
      - Else if `--format <path>` → load + validate that path.
      - Else if cached `format.json` exists AND its
        `content_key` matches the current input → reuse.
      - Else → call `discover()` (requires LLM endpoint in
        config); persist to `.sim-flow/spec-ingest/format.json`.
- [x] `--rediscover-format` always re-runs `discover()` and
      overwrites the cache.
- [x] Diagnostic output: discovery result counts + warnings
      printed to stderr.
- [x] Integration test: ingest with no LLM configured → falls
      back to `--no-format-discovery` with a stderr warning.

Gate: CLI flags work end-to-end on a markdown fixture and an
RV12 fixture.

### Milestone 9.7: structured-spans loading.rs

- [x] Replace `LoadedPdf { document: Arc<PdfDocument> }` field
      uses with per-page `PageLayout { page_number, spans,
      tables, paths, images, raw_text }` records.
- [x] `load_pdf` populates the per-page records eagerly from
      pdf_oxide's structured APIs. Text-only consumers (legacy
      ingest) still get a flat `text` field for backwards
      compatibility.
- [x] Drop the `<!-- spec-ingest:page=N -->` marker hack +
      `recover_page_ranges_and_strip_markers` post-pass —
      replaced by direct page-number association in
      `PageLayout`.
- [x] Update tests accordingly.

Gate: loading tests pass; `LoadedSource` consumers (chrome,
parse, classify, figures) compile against the new shape.

### Milestone 9.8: span-based parse.rs

- [x] Replace `parse_pdf` (already dropped post-Phase 8) and
      the markdown-based PDF parse path with a span-based one:
      cluster span font sizes across pages, assign heading
      level per text-line based on cluster id, emit
      `SectionTree` with `Section.page_range` set natively.
- [x] Each section's body is the structured spans within that
      section, NOT a markdown string. Spans pass through to
      classify / emit, which decide how to render.
- [x] `parse_markdown` for markdown / text inputs stays as-is.
- [x] Update `parse_hierarchy` to dispatch by SourceKind:
      `SourceKind::Pdf → parse_spans`,
      `SourceKind::Markdown | Text → parse_markdown`.
- [x] Unit tests with synthetic spans fixtures.

Gate: parse tests pass; RV12 produces a `SectionTree` whose
roots match the section headings detected in Milestone 9.3.

### Milestone 9.9: format-driven classify.rs

- [x] `classify.rs` reads `format.json::tables` and, for each
      table's `(page, first_line)`, calls
      `doc.extract_tables(page)` and matches by location, then
      uses the descriptor's `column_map` to project pdf_oxide's
      `TableCell.text` into the spec_md row schema.
- [x] Apply `wrap_strategy` (merge_continuation_rows /
      join_on_blank_first_col / single_row) before column
      mapping.
- [x] For each `spec_md_target.kind`, emit the appropriate
      typed table TOML under `primary/tables/<kind>/` (current
      Phase 2 emit layout — extend with `csrs/`, `csr_fields/`,
      `register_files/`, etc.).
- [x] Unknown-kind tables are still emitted as raw text under
      `primary/tables/unknown/` so DM0 can ask the user about
      them.
- [x] Unit tests for each table kind on synthetic descriptors +
      synthetic pdf_oxide `Table` outputs.

Gate: classify tests pass; ingest of RV12 with hand-authored
`format.json` emits non-zero tables of each declared kind.

### Milestone 9.10: chrome.rs hybrid positional + regex

- [x] `chrome.rs` reads `format.json::chrome` regexes AND uses
      span bboxes to identify lines whose Y is consistently in
      the top-of-page or bottom-of-page band across multiple
      pages.
- [x] Stripped chrome is removed from `PageLayout.spans` before
      parse runs.
- [x] Unit tests with synthetic chrome patterns.

Gate: chrome tests pass; running-header stripping on RV12
removes the per-page banner without false positives.

### Milestone 9.11: emit.rs chunk tagging

- [x] Each emitted chunk's front matter (`chunks/*.md`)
      includes per Chapter 7 §7.8:
      `spec_md_role`, `layer`, `acronyms_referenced`,
      `clock_domain`, `power_domain`, `reset_domain`,
      `contained_csr_tables`.
- [x] Manifest summary (`manifest.toml`) gains counts:
      `csrs_count`, `glossary_entries_count`, etc.
- [x] Update existing snapshot tests for the new front-matter
      keys.

Gate: emit tests + snapshot tests pass; RV12 chunks land with
populated `spec_md_role` and `acronyms_referenced` fields.

### Milestone 9.12: DM0 auto-populate over format.json

- [x] `dm0::auto_populate::populate_csrs(corpus_root, spec)`:
      reads `primary/tables/csrs/*.toml` and per-csr field
      tables, builds `Csr` + `CsrField` rows.
- [x] `populate_glossary(corpus_root, spec)` reads
      `format.json::glossary`.
- [x] `populate_clock_domains` / `populate_power_domains` /
      `populate_reset_domains` / `populate_security_boundaries` /
      `populate_numerical_conventions` /
      `populate_performance_counters` — each from the
      corresponding format.json section + tables.
- [x] Existing `populate_blocks` / `populate_parameters` /
      `populate_encodings` / `populate_errors` / `populate_fsms`
      switch from heading-text guessing to reading
      `format.json::tables[].spec_md_target` directly. Block
      ownership is `spec_md_target.block_name` (no inference).
- [x] Update `populate_blocks` to apply `BlockSignalRow.role`
      per descriptor.
- [x] Update `run()` orchestrator to invoke the new populates.
- [x] Unit tests for each new populate on synthetic fixtures.

Gate: per-populate tests pass; end-to-end DM0 RV12 fixture
test (Phase 6 milestone 6.12) populates the new sections.

### Milestone 9.13: lance index column additions

- [x] Per Architecture Chapter 3 §3.x, add to `spec_chunks`
      table: `spec_md_role: Utf8`, `layer: Utf8`,
      `acronyms_referenced: List<Utf8>`, `clock_domain:
      Utf8?`, `power_domain: Utf8?`, `reset_domain: Utf8?`.
- [x] Update build / refresh code in
      `lance_index::build::spec` to populate from chunk front
      matter.
- [x] Add semantic-search retrieval filters by `spec_md_role`
      / `layer` to `spec_semantic_search` tool args.
- [x] Unit tests on the build + a smoke test on retrieval-by-role.

Gate: spec_chunks lance table builds with new columns;
spec_semantic_search filters by role correctly.

### Milestone 9.14: built-in default descriptor

- [x] `format/default.rs::default_descriptor() -> FormatJson`:
      a hand-coded descriptor that produces today's behavior
      on Markdown inputs (`#`-style headings → role inferred
      from heading text; `|` pipe tables → kind inferred from
      column words).
- [x] Used by `--no-format-discovery` and as a starting point
      for hand-authoring.
- [x] Unit test on a markdown fixture.

Gate: markdown ingest with the default descriptor produces
the same chunks/tables as the markdown path produces today.

### Milestone 9.15: integration on four reference specs

- [x] Run `sim-flow ingest` on each of RV12, Numenta SoC,
      Apical NoC, Spatial Pooler (PDF inputs in
      `~/Downloads/` and `~/nta/sim-models/.../rv12/docs/`).
- [x] For each, capture the discovered `format.json`,
      validation block, and resulting manifest.
- [x] Record outcomes (counts of tables / glossary / chunks /
      stubs / warnings) under
      `tests/fixtures/spec-ingest-snapshots/<spec>/format-summary.md`.
- [x] Manual review: are CSR tables classified correctly on
      RV12? Memory map on NoC? Glossary on each? Note any
      `unknown` items for follow-up.

Gate: each spec ingests successfully (no fatal errors); each
yields non-empty classified tables + glossary; manual review
documented.

### Milestone 9.16: live DM0 against RV12 with new pipeline

- [ ] Run DM0 work session against RV12's freshly-ingested
      corpus + format.json with a real LLM (Claude Opus 4.7 or
      Qwen 3.6).
- [ ] Verify spec.md ends up with populated `blocks[]`,
      `csrs[]`, `memory_regions[]`, `glossary[]`, and the new
      domain / convention / PMU sections where the spec
      contains them.
- [ ] Gate-check passes.
- [ ] Record outcome alongside Phase 6 milestone 6.14 snapshot.

Gate: manual verification; recorded.

## Out of Scope (deferred to later phases)

- **Cross-spec descriptor merging.** Peer specs each get their
  own `format.json`; correlating them is future work.
- **Watch-loop re-discovery on source change.** Today the user
  runs `--rediscover-format`; an automated detector is later.
- **Per-spec descriptor authoring tooling** (`sim-flow ingest
  format edit <field>`). v1 ships with hand-edit-the-JSON.
- **Vision-model figure captioning.** Figures are rasterised
  and classified by `kind`; captioning prose stays as
  manual / DM0-driven for v1.
- **DM1+ retrieval-by-role wiring.** Phase 7 takes the lance
  filters this phase adds and wires them into DM1 / DM2 /
  DM3 prompts. Not done here.
- **Debug / test (JTAG, scan, ATPG) modeling.** Out of scope
  for the cycle-accurate model by user direction.
