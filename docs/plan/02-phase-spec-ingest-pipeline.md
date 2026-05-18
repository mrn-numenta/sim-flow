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

- [x] Create `src/__internal/session/spec_ingest/mod.rs` and
      remove the old `spec_ingest.rs` (or convert it into a
      shim that re-exports from the new module).
- [x] Create submodule files: `pipeline.rs`, `stages/mod.rs`,
      `stages/loading.rs`, `stages/chrome.rs`, `stages/parse.rs`,
      `stages/classify.rs`, `stages/references.rs`,
      `stages/figures.rs`, `stages/emit.rs`.
- [x] Define `IngestRequest`, `IngestOutcome`,
      `IngestWarning`, `IngestConfig` types in `pipeline.rs`.
- [x] Define an internal `Pipeline` orchestrator that runs the
      seven stages in order, with each stage taking the
      previous stage's output type.
- [x] Wire empty stage stubs returning default outputs.

Gate: `cargo build` succeeds; an empty pipeline can be
constructed and run on an empty input without panicking.

### Milestone 2.2: Stage 1 -- Source loading

- [x] In `stages/loading.rs`, implement `load(source: &Path)
      -> Result<LoadedSource>` dispatching by extension.
- [x] PDF branch: open with `pdfium_render::PdfDocument`,
      validate parseability, return the document handle.
- [x] Markdown branch: read UTF-8 with BOM stripping; return
      as a single-page document.
- [x] Text branch: same as markdown but mark `source_kind =
      "text"` so later stages know.
- [x] Hard-error on unknown extensions.
- [x] Unit tests for each branch with minimal fixtures.

Gate: `cargo test spec_ingest::stages::loading::` passes.

### Milestone 2.3: Stage 2 -- Page-chrome stripping

- [x] In `stages/chrome.rs`, implement `strip_chrome(pages:
      Vec<PageText>) -> (Vec<PageText>, ChromeRecord)`.
- [x] Detect lines appearing on >= 50% of pages with
      identical content after whitespace normalization.
- [x] Strip patterns matching `Page \d+ of \d+` and
      `\d+ / \d+` even when not appearing on all pages.
- [x] Record removed lines in `ChromeRecord` for the
      manifest.
- [x] Unit test on a synthetic three-page input with known
      chrome lines; assert stripping happens and the record
      captures them.
- [x] Markdown / text branch: no-op pass-through.

Gate: `cargo test spec_ingest::stages::chrome::` passes.

### Milestone 2.4: Stage 3 -- Hierarchical parsing (PDF)

- [x] In `stages/parse.rs`, implement
      `parse_hierarchy(loaded: &LoadedSource) -> SectionTree`.
- [x] For PDF: walk pages with pdfium, extract text with
      style information, infer heading levels from font
      size + style (bold / italic) clusters.
- [x] If the PDF has an outline (table of contents), use it
      as ground truth for the heading hierarchy and prefer it
      over inferred headings.
- [x] Construct `SectionTree` with each `Section` carrying
      heading text, level, full breadcrumb (ancestor chain),
      body text, and `page_range = (start, end)`.
- [x] Handle the degenerate case (no headings detected):
      produce a single root section spanning the whole
      document.
- [x] Markdown branch: use pulldown_cmark to walk H1-H6
      headings; produce the same tree.
- [x] Unit tests on a synthetic 3-page PDF (build inline via
      pdfium write API or use a checked-in tiny fixture) and
      on a synthetic markdown.

Gate: `cargo test spec_ingest::stages::parse::` passes.

### Milestone 2.5: Stage 4 -- Stub and TBD detection

- [x] In `stages/classify.rs`, implement `detect_stubs(tree:
      &SectionTree) -> Vec<StubRecord>`.
- [x] Stub criteria: section body is empty, contains only
      "TBD", or contains only placeholder phrases (regex over
      a small allow-list).
- [x] Implement `detect_tbds(tree: &SectionTree) ->
      Vec<TbdRecord>` scanning every section body for
      whole-word `TBD` with surrounding context.
- [x] Unit tests against fixtures with known stubs / TBDs.

Gate: `cargo test spec_ingest::stages::classify::` (stub /
TBD tests) passes.

### Milestone 2.6: Stage 4 -- Markdown table reassembly across pages

- [x] In `stages/classify.rs`, implement
      `reassemble_tables(section: &mut Section)`.
- [x] Detect tables that start in one section and continue in
      the next (a section ending with a partial table row +
      next section starting with the rest).
- [x] Stitch them and re-attach to the originating section
      with `page_range` extended to cover both.
- [x] Unit test on a fixture with a known cross-section
      table (synthetic).

Gate: table-reassembly unit test passes.

### Milestone 2.7: Stage 4 -- Signal-table extraction

- [x] In `stages/classify.rs`, implement
      `extract_signal_tables(section: &Section) ->
      Vec<SignalTable>`.
- [x] Match the table's header row against the canonical
      column set (Chapter 1 §1.7 `header_aliases`) with
      case-insensitive comparison and order-tolerance.
- [x] When matched, extract typed rows and emit a
      `SignalTable` with `breadcrumb = section.breadcrumb`,
      `stage = section.heading`.
- [x] Leave the matched table marked in the section body as
      `<!-- signal-table extracted to tables/signals/NNN.toml -->`.
- [x] Unit test against fixtures with two-column-order
      variations.

Gate: signal-table unit test passes.

### Milestone 2.8: Stage 4 -- Parameter, error, encoding, FSM table extraction

- [x] Implement `extract_parameter_tables` keyed on `Name |
      Default | Comment` or `Name | Type | Default | ...`
      header patterns.
- [x] Implement `extract_error_tables` keyed on `Error Type |
      Detecting Component | ...`.
- [x] Implement `extract_encoding_tables` keyed on `Value |
      Name | Abbreviation` or `Value | Name`.
- [x] Implement `extract_fsm_tables` keyed on `From | Input |
      To | Output` or similar transition shapes.
- [x] Per-kind unit tests.

Gate: per-kind unit tests pass.

### Milestone 2.9: Stage 5 -- Cross-spec reference parsing

- [x] In `stages/references.rs`, implement
      `parse_references(tree: &SectionTree) ->
      Vec<CrossSpecReference>`.
- [x] Pattern: `see <Title>[\s,:]+section\s+["']?<text>["']?`
      (case-insensitive).
- [x] Pattern: markdown links to peer spec files
      (`*.md` / `*.pdf` with relative-path heuristic).
- [x] Pattern: section content under a heading matching
      `^(References|Inherits[\s-]+from)$` (case-insensitive).
- [x] Each detected reference gets `peer_id = ""` (resolved
      later by the orchestrator from configured peer
      registrations).
- [x] Unit tests on a fixture with three reference styles.

Gate: references unit test passes.

### Milestone 2.10: Stage 6 -- Figure detection and page-region rendering

- [x] In `stages/figures.rs`, implement
      `detect_figure_pages(doc: &PdfDocument) -> Vec<u32>`
      returning page numbers that contain figure content.
- [x] Heuristic: a page contains figure content if either
      (a) it has at least one image XObject OR (b) the count
      of vector drawing operations (non-text path ops)
      exceeds a threshold (configurable; default 20).
- [x] Implement `render_figure_page(doc: &PdfDocument, page:
      u32, dpi: u32) -> Result<DynamicImage>` using pdfium's
      `render_with_config`.
- [x] Save as PNG to `figures/page-<NNN>.png`.
- [x] Emit `figures/page-<NNN>.caption.md` stub per Chapter 1
      §1.3.4.
- [x] Unit test: render a known fixture PDF page, verify the
      PNG is non-empty and has expected dimensions.

Gate: figure-rendering unit tests pass.

### Milestone 2.11: RV12 figure-render verification

- [x] Run the figure-rendering stage on
      `tests/fixtures/specs/rv12.pdf` and produce
      `figures/page-013.png`.
- [x] Compare against a golden snapshot (e.g. file size and
      perceptual hash). Alternatively, run an OCR pass
      (tesseract or a small VLM) and assert known labels
      (`if_nxt_pc`, `parcel_pc`) appear in the OCR output.
- [x] If OCR isn't available, fall back to: open the file,
      verify it's a valid PNG of at least 800x800 pixels and
      file size > 50 KB.
- [x] Add to integration tests.

Gate: RV12 figure-render verification passes.

### Milestone 2.12: Stage 7 -- Output emission

- [x] In `stages/emit.rs`, implement
      `emit_corpus(tree: &SectionTree, stubs: &[StubRecord],
      tbds: &[TbdRecord], refs: &[CrossSpecReference],
      figures: &[FigureOutput], out_dir: &Path)`.
- [x] Write each section as `chunks/NNN-<slug>.md` with YAML
      front matter per Chapter 1 §1.3.2.
- [x] Compute stable `chunk_id = sha256(breadcrumb ||
      source_page_range || body)` for each chunk.
- [x] Write structured tables to their typed locations under
      `tables/signals/`, `tables/parameters/`,
      `tables/errors/`, `tables/encodings/`, `tables/fsms/`.
- [x] Write `stubs.toml`, `tbds.toml`, `references.toml`,
      `manifest.toml`.
- [x] Implement atomic replace: write to
      `<out>.tmp/`, then `std::fs::rename` over `<out>/`.
- [x] Unit test: emit a synthetic corpus to a tmp dir and
      verify expected files exist with expected front-matter
      structure.

Gate: emit unit test passes.

### Milestone 2.13: Configuration loading

- [x] Implement `IngestConfig::load(project_root: &Path) ->
      IngestConfig` reading
      `<project>/.sim-flow/spec-ingest.config.toml`.
- [x] Apply defaults (Chapter 1 §1.7) for unset fields.
- [x] Unit test: load a fixture config; assert overrides
      apply and defaults fill in.

Gate: config-load unit test passes.

### Milestone 2.14: CLI subcommand

- [x] In `src/main.rs` or the appropriate CLI module, register
      the `ingest` subcommand with clap.
- [x] Implement `sim-flow ingest --source <path> [--peer
      <id>=<path>]... [--config <path>] [--out <project-root>]`.
- [x] Implement `sim-flow ingest --rebuild [--out
      <project-root>]` reading the existing manifest to
      recover source paths.
- [x] Implement `sim-flow ingest --status [--out
      <project-root>]` printing a manifest summary.
- [x] Integration test: shell out to the binary against a
      fixture and verify exit code + manifest contents.

Gate: CLI integration test passes.

### Milestone 2.15: Programmatic API

- [x] Expose `sim_flow::session::spec_ingest::pipeline::run`
      as the public Rust API per Chapter 1 §1.9.
- [x] Update `Cargo.toml` if needed to expose the symbol
      through the library target.
- [x] Unit test calling `run` programmatically.

Gate: programmatic-API unit test passes.

### Milestone 2.16: Integration test against four sample specs

- [x] Create `tests/spec_ingest_integration.rs`.
- [x] For each of the four sample specs:
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
- [x] Add the test fixtures (PDF files) to
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
