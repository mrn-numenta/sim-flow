# Phase 2: Spec Ingest Pipeline

## Goal

Replace `src/__internal/session/spec_ingest.rs` with the
seven-stage pipeline specified in Architecture Chapter 1.
Produce the on-disk corpus under `.sim-flow/spec-ingest/`,
expose the `sim-flow ingest` CLI subcommand, and pass
integration tests against the four sample specs.

## Inputs

- Architecture Chapter 1 (sections 1.1 through 1.9 in detail).
- Existing `src/__internal/session/spec_ingest.rs` (for
  reference; will be removed or substantially rewritten).
- Sample spec PDFs:
  - `tests/fixtures/specs/apical-noc.pdf`
  - `tests/fixtures/specs/numenta-soc.pdf`
  - `tests/fixtures/specs/spatial-pooler.pdf`
  - `tests/fixtures/specs/rv12.pdf`
  (Existing fixtures or new ones copied from
  `~/nta/sim-models/users/mneilly/*/docs/`; copy-of-pdf path
  acceptable.)

## Outputs

- New module `src/__internal/session/spec_ingest/` with
  per-stage submodules.
- CLI subcommand `sim-flow ingest`.
- Sample-spec ingest output under
  `tests/fixtures/spec-ingest-snapshots/<name>/` (golden
  files for regression).
- Integration tests in `tests/spec_ingest_integration.rs`.

## Acceptance Gate

- [ ] `cargo build --package sim-flow` succeeds.
- [ ] `cargo test --package sim-flow spec_ingest::` passes.
- [ ] `cargo test --package sim-flow --test spec_ingest_integration`
      passes against all four sample specs.
- [ ] `sim-flow ingest --source tests/fixtures/specs/rv12.pdf
      --out <tmp>` produces a `manifest.toml` whose
      `primary_signal_table_count >= 6` and
      `primary_figure_count >= 5`.
- [ ] Visual spot-check: the rendered `figures/page-013.png`
      from RV12 ingest contains visible signal labels
      (`if_nxt_pc`, `pc+2`, etc.). Verified by human or by an
      OCR pass; the milestone records the verification.
- [ ] `cargo clippy --package sim-flow -- -D warnings` passes.

## Milestones

### Milestone 2.1: Module scaffolding

- [ ] Create `src/__internal/session/spec_ingest/mod.rs` and
      remove the old `spec_ingest.rs` (or convert it into a
      shim that re-exports from the new module).
- [ ] Create submodule files: `pipeline.rs`, `stages/mod.rs`,
      `stages/loading.rs`, `stages/chrome.rs`, `stages/parse.rs`,
      `stages/classify.rs`, `stages/references.rs`,
      `stages/figures.rs`, `stages/emit.rs`.
- [ ] Define `IngestRequest`, `IngestOutcome`,
      `IngestWarning`, `IngestConfig` types in `pipeline.rs`.
- [ ] Define an internal `Pipeline` orchestrator that runs the
      seven stages in order, with each stage taking the
      previous stage's output type.
- [ ] Wire empty stage stubs returning default outputs.

Gate: `cargo build` succeeds; an empty pipeline can be
constructed and run on an empty input without panicking.

### Milestone 2.2: Stage 1 -- Source loading

- [ ] In `stages/loading.rs`, implement `load(source: &Path)
      -> Result<LoadedSource>` dispatching by extension.
- [ ] PDF branch: open with `pdfium_render::PdfDocument`,
      validate parseability, return the document handle.
- [ ] Markdown branch: read UTF-8 with BOM stripping; return
      as a single-page document.
- [ ] Text branch: same as markdown but mark `source_kind =
      "text"` so later stages know.
- [ ] Hard-error on unknown extensions.
- [ ] Unit tests for each branch with minimal fixtures.

Gate: `cargo test spec_ingest::stages::loading::` passes.

### Milestone 2.3: Stage 2 -- Page-chrome stripping

- [ ] In `stages/chrome.rs`, implement `strip_chrome(pages:
      Vec<PageText>) -> (Vec<PageText>, ChromeRecord)`.
- [ ] Detect lines appearing on >= 50% of pages with
      identical content after whitespace normalization.
- [ ] Strip patterns matching `Page \d+ of \d+` and
      `\d+ / \d+` even when not appearing on all pages.
- [ ] Record removed lines in `ChromeRecord` for the
      manifest.
- [ ] Unit test on a synthetic three-page input with known
      chrome lines; assert stripping happens and the record
      captures them.
- [ ] Markdown / text branch: no-op pass-through.

Gate: `cargo test spec_ingest::stages::chrome::` passes.

### Milestone 2.4: Stage 3 -- Hierarchical parsing (PDF)

- [ ] In `stages/parse.rs`, implement
      `parse_hierarchy(loaded: &LoadedSource) -> SectionTree`.
- [ ] For PDF: walk pages with pdfium, extract text with
      style information, infer heading levels from font
      size + style (bold / italic) clusters.
- [ ] If the PDF has an outline (table of contents), use it
      as ground truth for the heading hierarchy and prefer it
      over inferred headings.
- [ ] Construct `SectionTree` with each `Section` carrying
      heading text, level, full breadcrumb (ancestor chain),
      body text, and `page_range = (start, end)`.
- [ ] Handle the degenerate case (no headings detected):
      produce a single root section spanning the whole
      document.
- [ ] Markdown branch: use pulldown_cmark to walk H1-H6
      headings; produce the same tree.
- [ ] Unit tests on a synthetic 3-page PDF (build inline via
      pdfium write API or use a checked-in tiny fixture) and
      on a synthetic markdown.

Gate: `cargo test spec_ingest::stages::parse::` passes.

### Milestone 2.5: Stage 4 -- Stub and TBD detection

- [ ] In `stages/classify.rs`, implement `detect_stubs(tree:
      &SectionTree) -> Vec<StubRecord>`.
- [ ] Stub criteria: section body is empty, contains only
      "TBD", or contains only placeholder phrases (regex over
      a small allow-list).
- [ ] Implement `detect_tbds(tree: &SectionTree) ->
      Vec<TbdRecord>` scanning every section body for
      whole-word `TBD` with surrounding context.
- [ ] Unit tests against fixtures with known stubs / TBDs.

Gate: `cargo test spec_ingest::stages::classify::` (stub /
TBD tests) passes.

### Milestone 2.6: Stage 4 -- Markdown table reassembly across pages

- [ ] In `stages/classify.rs`, implement
      `reassemble_tables(section: &mut Section)`.
- [ ] Detect tables that start in one section and continue in
      the next (a section ending with a partial table row +
      next section starting with the rest).
- [ ] Stitch them and re-attach to the originating section
      with `page_range` extended to cover both.
- [ ] Unit test on a fixture with a known cross-section
      table (synthetic).

Gate: table-reassembly unit test passes.

### Milestone 2.7: Stage 4 -- Signal-table extraction

- [ ] In `stages/classify.rs`, implement
      `extract_signal_tables(section: &Section) ->
      Vec<SignalTable>`.
- [ ] Match the table's header row against the canonical
      column set (Chapter 1 §1.7 `header_aliases`) with
      case-insensitive comparison and order-tolerance.
- [ ] When matched, extract typed rows and emit a
      `SignalTable` with `breadcrumb = section.breadcrumb`,
      `stage = section.heading`.
- [ ] Leave the matched table marked in the section body as
      `<!-- signal-table extracted to tables/signals/NNN.toml -->`.
- [ ] Unit test against fixtures with two-column-order
      variations.

Gate: signal-table unit test passes.

### Milestone 2.8: Stage 4 -- Parameter, error, encoding, FSM table extraction

- [ ] Implement `extract_parameter_tables` keyed on `Name |
      Default | Comment` or `Name | Type | Default | ...`
      header patterns.
- [ ] Implement `extract_error_tables` keyed on `Error Type |
      Detecting Component | ...`.
- [ ] Implement `extract_encoding_tables` keyed on `Value |
      Name | Abbreviation` or `Value | Name`.
- [ ] Implement `extract_fsm_tables` keyed on `From | Input |
      To | Output` or similar transition shapes.
- [ ] Per-kind unit tests.

Gate: per-kind unit tests pass.

### Milestone 2.9: Stage 5 -- Cross-spec reference parsing

- [ ] In `stages/references.rs`, implement
      `parse_references(tree: &SectionTree) ->
      Vec<CrossSpecReference>`.
- [ ] Pattern: `see <Title>[\s,:]+section\s+["']?<text>["']?`
      (case-insensitive).
- [ ] Pattern: markdown links to peer spec files
      (`*.md` / `*.pdf` with relative-path heuristic).
- [ ] Pattern: section content under a heading matching
      `^(References|Inherits[\s-]+from)$` (case-insensitive).
- [ ] Each detected reference gets `peer_id = ""` (resolved
      later by the orchestrator from configured peer
      registrations).
- [ ] Unit tests on a fixture with three reference styles.

Gate: references unit test passes.

### Milestone 2.10: Stage 6 -- Figure detection and page-region rendering

- [ ] In `stages/figures.rs`, implement
      `detect_figure_pages(doc: &PdfDocument) -> Vec<u32>`
      returning page numbers that contain figure content.
- [ ] Heuristic: a page contains figure content if either
      (a) it has at least one image XObject OR (b) the count
      of vector drawing operations (non-text path ops)
      exceeds a threshold (configurable; default 20).
- [ ] Implement `render_figure_page(doc: &PdfDocument, page:
      u32, dpi: u32) -> Result<DynamicImage>` using pdfium's
      `render_with_config`.
- [ ] Save as PNG to `figures/page-<NNN>.png`.
- [ ] Emit `figures/page-<NNN>.caption.md` stub per Chapter 1
      §1.3.4.
- [ ] Unit test: render a known fixture PDF page, verify the
      PNG is non-empty and has expected dimensions.

Gate: figure-rendering unit tests pass.

### Milestone 2.11: RV12 figure-render verification

- [ ] Run the figure-rendering stage on
      `tests/fixtures/specs/rv12.pdf` and produce
      `figures/page-013.png`.
- [ ] Compare against a golden snapshot (e.g. file size and
      perceptual hash). Alternatively, run an OCR pass
      (tesseract or a small VLM) and assert known labels
      (`if_nxt_pc`, `parcel_pc`) appear in the OCR output.
- [ ] If OCR isn't available, fall back to: open the file,
      verify it's a valid PNG of at least 800x800 pixels and
      file size > 50 KB.
- [ ] Add to integration tests.

Gate: RV12 figure-render verification passes.

### Milestone 2.12: Stage 7 -- Output emission

- [ ] In `stages/emit.rs`, implement
      `emit_corpus(tree: &SectionTree, stubs: &[StubRecord],
      tbds: &[TbdRecord], refs: &[CrossSpecReference],
      figures: &[FigureOutput], out_dir: &Path)`.
- [ ] Write each section as `chunks/NNN-<slug>.md` with YAML
      front matter per Chapter 1 §1.3.2.
- [ ] Compute stable `chunk_id = sha256(breadcrumb ||
      source_page_range || body)` for each chunk.
- [ ] Write structured tables to their typed locations under
      `tables/signals/`, `tables/parameters/`,
      `tables/errors/`, `tables/encodings/`, `tables/fsms/`.
- [ ] Write `stubs.toml`, `tbds.toml`, `references.toml`,
      `manifest.toml`.
- [ ] Implement atomic replace: write to
      `<out>.tmp/`, then `std::fs::rename` over `<out>/`.
- [ ] Unit test: emit a synthetic corpus to a tmp dir and
      verify expected files exist with expected front-matter
      structure.

Gate: emit unit test passes.

### Milestone 2.13: Configuration loading

- [ ] Implement `IngestConfig::load(project_root: &Path) ->
      IngestConfig` reading
      `<project>/.sim-flow/spec-ingest.config.toml`.
- [ ] Apply defaults (Chapter 1 §1.7) for unset fields.
- [ ] Unit test: load a fixture config; assert overrides
      apply and defaults fill in.

Gate: config-load unit test passes.

### Milestone 2.14: CLI subcommand

- [ ] In `src/main.rs` or the appropriate CLI module, register
      the `ingest` subcommand with clap.
- [ ] Implement `sim-flow ingest --source <path> [--peer
      <id>=<path>]... [--config <path>] [--out <project-root>]`.
- [ ] Implement `sim-flow ingest --rebuild [--out
      <project-root>]` reading the existing manifest to
      recover source paths.
- [ ] Implement `sim-flow ingest --status [--out
      <project-root>]` printing a manifest summary.
- [ ] Integration test: shell out to the binary against a
      fixture and verify exit code + manifest contents.

Gate: CLI integration test passes.

### Milestone 2.15: Programmatic API

- [ ] Expose `sim_flow::session::spec_ingest::pipeline::run`
      as the public Rust API per Chapter 1 §1.9.
- [ ] Update `Cargo.toml` if needed to expose the symbol
      through the library target.
- [ ] Unit test calling `run` programmatically.

Gate: programmatic-API unit test passes.

### Milestone 2.16: Integration test against four sample specs

- [ ] Create `tests/spec_ingest_integration.rs`.
- [ ] For each of the four sample specs:
  - Run the pipeline against `tests/fixtures/specs/<name>.pdf`.
  - Assert manifest.toml exists with expected
    `primary_chunk_count` ranges.
  - Assert at least one `tables/signals/*.toml` for RV12.
  - Assert at least one `figures/*.png` for each.
  - Assert `stubs.toml` records the known stub sections for
    Numenta SoC (HTM, CPU System, Memory System, NoC, Boot,
    Clock, Debug, Register Definition, SW Flow).
  - Assert `tbds.toml` records at least one TBD for each
    spec that has them.
- [ ] Add the test fixtures (PDF files) to
      `tests/fixtures/specs/` if not present; record their
      sha256 in a `tests/fixtures/specs/CHECKSUMS.toml` to
      detect drift.

Gate: integration test passes against all four specs.

### Milestone 2.17: Golden-output snapshots

- [ ] Create `tests/fixtures/spec-ingest-snapshots/<name>/`
      with the expected output structure of each sample
      spec.
- [ ] Add a snapshot test: ingest each fixture, compare the
      output directory's file list against the snapshot.
- [ ] Allow regenerating snapshots via
      `UPDATE_INGEST_SNAPSHOTS=1 cargo test`.
- [ ] Document the regenerate flow in a README under
      `tests/fixtures/spec-ingest-snapshots/`.

Gate: snapshot tests pass; regenerate flow documented.

### Milestone 2.18: Diagnostic surfacing through manifest

- [ ] Aggregate warnings produced by stages 1-7 into
      `manifest.toml.warnings` per Chapter 1 §1.5.
- [ ] Unit test: a degenerate-fixture (no headings detected)
      produces a manifest with the expected warning entry.

Gate: warning-surfacing unit test passes.

## Out of Scope (deferred to later phases)

- **Building the LanceDB index from the corpus.** Phase 4.
- **DM0 integration that auto-populates spec.md from the
  corpus.** Phase 6.
- **Captioning the figures.** Out of scope entirely for v1.
- **Cropping individual figures from a page.** Whole-page
  rendering only for v1.
- **OCR-rotation normalization.** Captions consumed by vision
  models tolerate rotation.
- **Streaming / partial-rebuild paths.** Re-ingestion is
  full-rebuild; partial refresh is a future optimization.
- **Multi-process locking.** Single-writer per project; no
  concurrent ingest support in v1.
