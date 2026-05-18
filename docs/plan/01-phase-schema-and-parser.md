# Phase 1: Schema and Parser

## Goal

Implement the typed `SpecMd` Rust struct hierarchy, a markdown
parser that produces `SpecMd` from a spec.md file, a writer
that produces a spec.md file from `SpecMd` with round-trip
identity, and the new `spec.md.tmpl` that conforms to the
schema. This phase produces the foundation that Phases 2, 6,
and 8 consume.

## Inputs

- Architecture Chapter 2 (spec.md schema), specifically
  sections 2.2 through 2.9.
- The existing template at
  `templates/model-project/docs/spec.md.tmpl` (for reference;
  it will be replaced).

## Outputs

- New module `src/__internal/session/spec_md/` with:
  - `mod.rs` (public surface).
  - `types.rs` (Rust type definitions).
  - `parser/mod.rs` plus per-section parser modules.
  - `writer.rs` plus per-section writer logic.
  - `traversal.rs` (required-field traversal).
  - `validate.rs` (cross-reference validation).
- New `templates/model-project/docs/spec.md.tmpl`.
- Fixtures under `tests/fixtures/spec_md/` (3-5 example
  spec.md files of varying completeness).
- Unit tests under `src/__internal/session/spec_md/*` with
  per-section coverage.
- Integration test `tests/spec_md_round_trip.rs`.

## Acceptance Gate

- [ ] `cargo build --package sim-flow` succeeds.
- [ ] `cargo test --package sim-flow spec_md::` passes (all
      unit tests).
- [ ] `cargo test --package sim-flow --test spec_md_round_trip`
      passes (round-trip identity on all fixtures and on the
      new template).
- [ ] `cargo clippy --package sim-flow -- -D warnings` passes.

## Milestones

### Milestone 1.1: Type definitions

- [x] Create `src/__internal/session/spec_md/mod.rs` with
      module wiring and public re-exports.
- [x] Create `src/__internal/session/spec_md/types.rs`.
- [x] Define `SpecMd` top-level struct holding every section.
- [x] Define `Metadata`, `SourceDocument` (with `role` enum:
      `primary | peer`).
- [x] Define `AssumptionsAndConstraints`, `QuantitativeRow`.
- [x] Define `ExternalInterface`, `ExternalSignalRow` (six-
      column form: Signal / Direction / Width / Type /
      Required / Description).
- [x] Define `Block`, `BlockSignalRow` (four-column form:
      Signal / Direction / Peer / Description), `BlockState`.
- [x] Define `Parameter` (single typed table row).
- [x] Define `StateMachine`, `FsmState`, `FsmTransition`.
- [x] Define `Encoding`, `EncodingValue`.
- [x] Define `MemoryRegion`.
- [x] Define `Connectivity`, `Node`, `Edge`.
- [x] Define `ErrorEntry`.
- [x] Define `FunctionalBehavior`, `Operation`.
- [x] Define `TimingAndThroughput`, `LatencyRow`.
- [x] Define `PipelineAndHierarchy` (prose-only).
- [x] Define `ResetInitFlushDrain` (prose-only).
- [x] Define `CycleAccurateScenario`, `CycleAccurateRow`.
- [x] Define `FigureEntry`, `FigureElement`.
- [x] Define `WorkedExample`.
- [x] Define `SourceSpecAnchor` (with three forms: page,
      page-range, chunk).
- [x] Define `OpenQuestion`, `AutoDecision`.
- [x] Derive `serde::Serialize`, `serde::Deserialize`, `Clone`,
      `Debug`, `PartialEq` on all types.

Gate: `cargo build --package sim-flow` succeeds.

### Milestone 1.2: Parser foundation

- [x] Add `pulldown_cmark` dependency to
      `tools/sim-flow/Cargo.toml` if not already present.
- [x] Create `src/__internal/session/spec_md/parser/mod.rs`
      with the public entry point: `parse(input: &str) ->
      Result<SpecMd, SpecMdParseError>`.
- [x] Define `SpecMdParseError` with variants for missing
      section, malformed table, bad anchor, etc., each
      carrying line and column.
- [x] Implement a markdown event-stream walker that segments
      the document into H2-delimited sections.
- [x] Implement section-header dispatch: match heading text
      against a fixed table of section names; route to the
      appropriate per-section parser stub.
- [x] Stub each per-section parser function with an empty
      default return so the dispatch wiring compiles end-to-
      end.

Gate: `cargo build` succeeds; `parser::parse("")` returns
`Ok(SpecMd::default())` with all sections empty.

### Milestone 1.3: Markdown table parser helper

- [x] Create `src/__internal/session/spec_md/parser/table.rs`.
- [x] Implement a `MarkdownTable` parser that takes a
      pulldown_cmark event range and produces a
      `MarkdownTable { headers: Vec<String>, rows: Vec<Vec<String>> }`.
- [x] Implement `normalize_header(name: &str) ->
      CanonicalColumn` matching Chapter 2 §2.5 aliases.
- [x] Implement column-presence check helpers
      (`require_columns`, `optional_column`).
- [x] Unit test on three table variants: signal table,
      parameter table, error table.

Gate: `cargo test --package sim-flow spec_md::parser::table::`
passes.

### Milestone 1.4: Per-section parsers — Metadata + prose sections

- [x] Implement `parser/metadata.rs` parsing the
      definition-list shape (Chapter 2 §2.3.1).
- [x] Implement `parser/prose.rs` for `Purpose`, `Scope`,
      `Non-goals`, `Functional Behavior > End-to-end
      behavior`, `Pipeline and Hierarchy`, `Reset / Init /
      Flush / Drain` (single-paragraph or multi-paragraph
      prose).
- [x] Implement `parser/assumptions.rs` for `Assumptions and
      Constraints` (the quantitative table + the two prose
      subsections).
- [x] Add unit tests for each, using minimal hand-authored
      fixtures inline.

Gate: per-section unit tests pass.

### Milestone 1.5: Per-section parsers — External Interfaces

- [x] Implement `parser/external_interfaces.rs`.
- [x] Handle the `### Interface: <name>` heading pattern.
- [x] Parse the property block (Direction / Protocol / Clock
      domain / Connected peer).
- [x] Parse the six-column signal table.
- [x] Parse transaction-semantics, timing / flow control,
      error subsection prose.
- [x] Parse the source-spec-anchors list.
- [x] Unit test against a fixture with two interfaces.

Gate: external_interfaces unit test passes.

### Milestone 1.6: Per-section parsers — Blocks

- [x] Implement `parser/blocks.rs`.
- [x] Handle the `### Block: <name>` heading pattern.
- [x] Parse the property block (Role / Parent / Clock domain /
      Parameterized by).
- [x] Parse the four-column I/O signal table.
- [x] Parse the State subsection (bulleted list of state
      elements with `name (width, reset value)` shape).
- [x] Parse the Behavior summary prose.
- [x] Parse the Source-spec anchors and Figures lists.
- [x] Parse the optional Sub-blocks list.
- [x] Unit test against a fixture with three blocks in a
      two-level hierarchy.

Gate: blocks unit test passes.

### Milestone 1.7: Per-section parsers — Parameters

- [x] Implement `parser/parameters.rs`.
- [x] Parse the single typed table (Name / Type / Default /
      Valid range / Behavioral impact / Source-anchor).
- [x] Unit test against a fixture with five parameters.

Gate: parameters unit test passes.

### Milestone 1.8: Per-section parsers — State Machines

- [x] Implement `parser/state_machines.rs`.
- [x] Handle the `### FSM: <name>` heading pattern.
- [x] Parse the property block (Reset state / Source-spec
      anchor).
- [x] Parse the States bulleted list (`<state> - <description>`).
- [x] Parse the Transitions table (From / Input / To / Output).
- [x] Unit test against a fixture with one FSM.

Gate: state_machines unit test passes.

### Milestone 1.9: Per-section parsers — Encodings

- [x] Implement `parser/encodings.rs`.
- [x] Handle the `### Encoding: <field>` heading pattern.
- [x] Parse the property block (Bit width / Source-anchor).
- [x] Parse the values table (Value / Name / Abbreviation).
- [x] Parse the optional Reserved / illegal line.
- [x] Unit test against a fixture with one encoding.

Gate: encodings unit test passes.

### Milestone 1.10: Per-section parsers — Memory Map, Connectivity, Error Handling

- [x] Implement `parser/memory_map.rs` (single typed table).
- [x] Implement `parser/connectivity.rs` (Nodes table + Edges
      table + Routing-rules prose).
- [x] Implement `parser/errors.rs` (single typed error table).
- [x] Unit test each.

Gate: per-section unit tests pass.

### Milestone 1.11: Per-section parsers — Functional Behavior, Timing, Cycle-Accurate, Figures

- [x] Implement `parser/functional_behavior.rs` (end-to-end
      prose + operation flow numbered list with `id - purpose
      (anchor: ...)` shape + data movement prose).
- [x] Implement `parser/timing.rs` (latency table + prose).
- [x] Implement `parser/cycle_accurate.rs` (per-scenario
      heading + the cycle-by-cycle table).
- [x] Implement `parser/figures.rs` (per-figure heading +
      properties + caption + elements-depicted table).
- [x] Unit tests for each.

Gate: per-section unit tests pass.

### Milestone 1.12: Per-section parsers — Worked Examples, Anchors, Questions, Decisions

- [x] Implement `parser/worked_examples.rs` (per-example
      heading + Inputs / Expected flow / Expected outputs
      subsections).
- [x] Implement `parser/anchors.rs` (the anchor-map table).
- [x] Implement `parser/open_questions.rs` (bulleted list).
- [x] Implement `parser/auto_decisions.rs` (bulleted list,
      each line: `decided <decision>; rationale: <one
      sentence>`).
- [x] Unit tests.

Gate: per-section unit tests pass.

### Milestone 1.13: Anchor format parser

- [x] Implement `SourceSpecAnchor::parse(&str) ->
      Result<SourceSpecAnchor, AnchorParseError>` handling
      the three forms (page, page-range, chunk).
- [x] Implement `SourceSpecAnchor::to_string(&self) -> String`
      producing the canonical form.
- [x] Unit tests covering each form, plus malformed inputs.

Gate: anchor unit tests pass.

### Milestone 1.14: Cross-reference validation

- [x] Create `validate.rs`.
- [x] Implement `SpecMd::validate(&self) -> Vec<ValidationIssue>`.
- [x] Check every Block's `parent` references an existing
      block or the literal `(none -- top-level)`.
- [x] Check every signal-row's `peer` references an existing
      block or external interface.
- [x] Check every source-spec anchor parses cleanly.
- [x] Check required Quantitative rows are present (Clock
      frequency matching `\d+\s*(MHz|GHz)`, Gate budget per
      cycle matching `\d+`).
- [x] Unit tests for each check.

Gate: validate unit tests pass.

### Milestone 1.15: Writer (SpecMd to markdown)

- [x] Create `writer.rs`.
- [x] Implement `SpecMd::to_markdown(&self) -> String`.
- [x] Implement per-section writers mirroring the parser
      structure: one function per section.
- [x] Use a markdown formatter helper for tables (handles
      column alignment heuristically).
- [x] Unit tests for each section writer (input: typed
      struct; output: markdown string; gate: contains the
      expected heading and table rows).

Gate: writer unit tests pass.

### Milestone 1.16: Round-trip identity test

- [x] Create `tests/spec_md_round_trip.rs`.
- [x] Author three fixture files under
      `tests/fixtures/spec_md/`:
  - `minimal.md` — only required sections, minimal content.
  - `rv12-extract.md` — a subset of an RV12-style spec.md
    exercising Blocks, Parameters, Encodings, Worked Examples.
  - `numenta-stubby.md` — heavy on optional sections with
    Open Questions and Auto-decisions.
- [x] For each fixture, assert
      `parse(write(parse(fixture))) == parse(fixture)` (round
      trip stability; byte equality is NOT required).
- [x] For each fixture, assert
      `parse(write(parse(fixture))).validate().is_empty()`.

Gate: round-trip tests pass.

### Milestone 1.17: New spec.md template

- [x] Replace
      `templates/model-project/docs/spec.md.tmpl` with the
      new structured template per Chapter 2 §2.8.
- [x] Include every REQUIRED section heading.
- [x] Include table header rows (column headers + separator
      rows) for every REQUIRED table.
- [x] Include OPTIONAL section headings as HTML comments
      (`<!-- ## State Machines (uncomment if applicable) -->`).
- [x] Top-of-file comment block summarizes the authoring loop
      and points at Architecture Chapter 2.
- [x] Add a unit test that parses the template, asserts no
      errors, and asserts the round-trip identity holds.

Gate: template unit test passes.

### Milestone 1.18: Required-field traversal

- [ ] Create `traversal.rs`.
- [ ] Define `MissingField { section_path: String,
      prompt_template: String, kind: MissingFieldKind }`.
- [ ] Define `MissingFieldKind` (Scalar / Prose /
      ConstrainedScalar { regex } / TableRow {
      column_names } / SectionApplicability).
- [ ] Implement `SpecMd::missing_required_fields(&self) ->
      Vec<MissingField>` walking the schema in
      template-order.
- [ ] Implement the prompt-template strings as a const table.
- [ ] Unit test: empty `SpecMd::default()` produces the
      expected ordered list of MissingFields; a fully-
      populated `SpecMd` produces an empty list.

Gate: traversal unit tests pass.

### Milestone 1.19: Public API surface and doc comments

- [ ] Add doc-comments to all public types and functions in
      `spec_md/mod.rs`.
- [ ] Re-export the canonical public surface:
      `parse`, `SpecMd`, `SpecMdParseError`, `ValidationIssue`,
      `MissingField`.
- [ ] Run `cargo doc --package sim-flow --no-deps` and
      confirm no warnings.

Gate: `cargo doc` succeeds without warnings on the spec_md
module.

## Out of Scope (deferred to later phases)

- **Wiring into DM0.** Phase 6 owns this.
- **Wiring into the lance index.** Phase 4 / 5 own this.
- **Authoring the actual prompts.** Phases 6 and 7 own this.
- **Source-spec ingestion.** Phase 2 owns this.
- **Pretty-printer formatting policy.** Round-trip identity
  is the contract; whitespace / column-width choices are
  implementation detail.
- **Migrating existing projects' spec.md files.** Phase 8
  owns this.
