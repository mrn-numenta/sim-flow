# Restructuring spec.md for Real Specs and LanceDB-Era RAG

**Status:** brainstorm. Explores needs and possibilities for
restructuring the DMF spec.md template. No code, schema, or
implementation plan in this doc — the architecture / template-
update work happens after we settle on a direction.
**Date:** 2026-05-17.
**Related:** [lancedb-rig-incorporation-plan.md](lancedb-rig-incorporation-plan.md),
[spec-ingest-figure-extraction.md](spec-ingest-figure-extraction.md),
the four-spec review thread in conversation.

## 1. What this doc is and isn't

The user's call from the spec.md review thread: **proceed with
the B + L2 + L7 direction** (table-driven spec.md + lance-backed
spec retrieval + signal-table structured index). This doc
brainstorms what that direction implies — what spec.md should
look like at a section-shape level, what structured artifacts
it should carry, and where the open questions are before the
architecture doc lands.

This is **not** a new template. It's an exploration of the
needs the new template has to satisfy and the tensions between
them.

## 2. The current template and what it asks of the agent

[`templates/model-project/docs/spec.md.tmpl`](../../templates/model-project/docs/spec.md.tmpl)
is 218 lines, prose-heavy, with sections for:

- Metadata
- Purpose And Scope (purpose / scope / non-goals)
- Assumptions And Constraints (tech node / clock freq / gate
  budget / environmental / architectural)
- External Interfaces, Internal Interfaces (per-interface:
  direction / protocol / clock domain / signal table /
  transaction semantics / timing & flow control / error
  behavior)
- Functional Behavior (end-to-end / operation flow / data
  movement / state & storage / arbitration & scheduling)
- Timing, Latency, And Throughput
- Pipeline And Hierarchy
- Reset, Initialization, Flush, And Drain
- Parameters And Configuration
- Corner Cases And Exceptional Behavior
- Worked Examples
- Open Questions, Auto-decisions

The template's implicit model: **spec.md is the complete prose
representation of the design.** DM1 / DM2 / DM3 read it as the
only authoritative source. Hence the prose density.

## 3. Why this needs to change

Two pressure points from the four-spec review (Apical NoC,
Numenta SoC, Spatial Pooler 2.1, RV12 95pp):

### 3a. Real specs carry structured content the template can't hold

The four specs together exhibit at least these structured forms
the current template flattens to prose:

- **Per-block I/O signal tables.** RV12 has six (IF / PD / ID /
  EX / MEM / WB) all following the same `Signal / Direction /
  To-From / Description` shape. The template has a generic
  signal table inside the "Interface" subsection, but no notion
  of "every pipeline stage has one of these as its interface
  contract."
- **Address / register maps.** Numenta SoC has a Memory Map +
  Register Definition section as first-class concepts. The
  template's "Parameters" covers RTL parameters, not address
  spaces or programmer-visible registers.
- **Encoding tables.** RV12's privilege levels (00→U, 01→S,
  10→H, 11→M), instruction encodings, CSR field maps. Numenta
  SoC's error-type → response table.
- **Finite-state machines.** Numenta Boot FSM stages. Hardware
  specs often have FSMs as `{state, input, next_state, output}`
  tables.
- **Data-structure layouts.** Spatial Pooler's `Structure 1`,
  `Structure 2` with bit-widths, bank striping, per-engine
  sizes.
- **Cycle-by-cycle diagrams.** RV12's "instructions in flight"
  pattern.

These are not exotic. They are the bread and butter of hardware
specs and a flat prose template cannot represent them in a way
DM1+ can mechanically consume.

### 3b. With L2 RAG, prose duplication is wasted work

L2 (semantic search over source-spec section chunks) means
DM1 / DM2 / DM3 can re-fetch any prose passage from the source
spec on demand. The implicit assumption that spec.md must be
**complete** stops being load-bearing. What spec.md needs to
carry is the **structured projection** of the source spec — the
typed artifacts gates and downstream tools can validate against
— plus the **anchors** that let the agent retrieve more on
demand.

Prose paraphrasing the source becomes redundant. Structured
tables become essential.

## 4. The B + L2 + L7 direction

From the prior review thread, three options:

- **A.** Keep prose-heavy spec.md; add `.toml` siblings for
  structured artifacts.
- **B.** Restructure spec.md to be table-driven, less prose,
  schemas-roundtrippable.
- **C.** Replace spec.md with a thin index pointing at per-block
  `docs/spec/<NN>-<block>.md` files.

The chosen direction is **B**, augmented by **L2** (the agent
can retrieve missing context from the source spec) and **L7**
(extracted signal tables are indexed in a structured Lance table
the agent queries directly).

### Why B over A

A keeps the human-readability of prose at the cost of doubling
the surface area. Every fact lives in both the prose and the
TOML sibling, and they will drift. B keeps a single source of
truth (the structured table) embedded inside human-readable
markdown.

### Why B over C

C is the right structure for VERY large specs but adds a layer
of indirection that's overkill for the typical project. Most
projects fit a single spec.md, and we can paginate per-block
later if and when a spec needs it (DM0 already supports
`docs/spec/<NN>-...` layout). B doesn't preclude C; it sets the
ground rules first.

### Why L2 changes the math

With L2 the spec.md doesn't need to be the only retrieval
surface. It can lean on the source spec for the long tail of
detail. This is what makes B viable — without L2, dropping
prose from spec.md would lose information the downstream steps
need.

### Why L7 deserves its own slice

The signal-table pattern is uniform enough across hardware
specs that a structured index (one row per signal, columns:
`stage`, `signal_name`, `direction`, `peer`, `description`,
`source_chunk_id`) gives the agent first-class queries like
"every signal driven from PD" or "every input to the ALU." This
is hybrid query in Lance (scalar filter + optional vector
similarity), not pure vector RAG.

## 5. What sections / structured artifacts the new spec.md
should carry

Brainstorm only — schemas are sketched, not specified. Each
entry calls out:

- What it captures
- What downstream consumer needs it
- Open shape question

### 5a. Metadata (keep)

Same as today — name, version, status, author, source documents,
last updated. Add an explicit `inherits_from` field for
SP→TM-style cross-spec inheritance (see L0g in the lancedb-rig
doc).

### 5b. Purpose, Scope, Non-goals (keep, trim)

Prose stays; tighten to one short paragraph each. The current
template encourages over-writing here; with L2 the agent can
re-fetch source context, so we don't need to restate the world.

### 5c. Assumptions and Constraints (keep, structure
quantitative fields)

Tech node / clock freq / gate budget become a small typed table
rather than free-form prose. Existing gate-budget-per-cycle
regex requirement keeps working against the table cell.
Environmental / architectural assumptions stay prose.

### 5d. Blocks (NEW — replaces / generalizes "Internal Interfaces")

Per-block subsections, each containing:

- Block identity (name, role, parent block, optional
  parameterization)
- I/O signal table (the RV12-style structured table — the
  single most valuable artifact for hardware specs)
- Internal state summary (storage elements + their reset
  behavior)
- Behavior summary (one or two paragraphs of prose, can be thin
  if the source spec covers it and L2 can retrieve it)
- Figure references (links to `images/<n>` + caption file — see
  [spec-ingest-figure-extraction.md](spec-ingest-figure-extraction.md))
- Source-spec anchors (page numbers + section breadcrumbs in
  the source — lets DM1+ re-fetch via L2 by quoting the anchor)

Open question: how deep does the per-block nesting go? RV12 has
two levels (pipeline → stages); Apical NoC has three (SoC →
NoC → router/channel/interface). The template should allow
nesting without imposing depth.

### 5e. External Interfaces (keep, table-driven)

Same shape as today, but the signal table becomes the canonical
structured form (matching the per-block signal tables in 5d).

### 5f. Parameters (keep, expand to typed table)

Each parameter as a row: `name`, `type`, `default`, `valid
range`, `behavioral impact`. RV12 has dozens
(`XLEN`, `HAS_BPU`, `BPU_LOCAL_BITS`, `DCACHE_SIZE`, ...).
Apical NoC has tens of `NOC_*_NUM_CHANNELS` / `NOC_*_DATA_WIDTH`
/ `NOC_*_FIFO_DEPTH`. The current template's "Parameter: <name>"
subsection works but is one-per-section; a single table is
denser.

### 5g. State Machines (NEW)

For each FSM the model must implement:

- FSM name and role
- States (with reset state called out)
- Transitions table: `from_state | input/event | to_state |
  output/action`
- Source-spec anchor

Numenta SoC's Boot FSM motivates this. Without it the spec.md
forces FSMs into prose, where DM2 has to re-parse them.

### 5h. Encodings (NEW)

For each bit-level encoding:

- Field name
- Bit width
- Encoding table: `value | name | meaning`
- Reserved / illegal values
- Source-spec anchor

RV12 privilege levels, instruction encodings, CSR fields all
fit here.

### 5i. Memory Map / Address Map (NEW, optional)

When the design has memory-mapped resources:

- Region table: `start | end | name | purpose | access (R/W)`
- Address translation notes (PCIe ATU, NoC routing modes,
  etc.)

Numenta SoC and RV12 both need this. Apical NoC needs the NoC-
equivalent: which engine owns which slice.

### 5j. Connectivity / Topology (NEW, optional)

For mesh / NoC / hierarchy designs:

- Node table or explicit edge list
- Routing rules (XY, YX, etc.)
- Domain crossings (clock / voltage / power)

Apical NoC's 5x5 mesh layout is the motivating case.

### 5k. Error Table (NEW)

Structured rows: `error_type | detecting_component |
detection_behavior | bus_response | master_behavior |
response_to_software`. Numenta SoC's error-handling section is
the model.

### 5l. Functional Behavior (keep, trim, anchor)

End-to-end behavior + operation flow + data movement. Stays
prose-with-bullets, but each operation in the flow gets a
named identifier so downstream artifacts (decomposition.toml,
pipeline-mapping) can reference operations by name. Includes
source-spec anchors per operation when the prose came from the
source.

### 5m. Pipeline / Hierarchy (keep, defer to Blocks)

Pipeline depth + stage boundaries + clock domains. Most of the
per-stage detail lives under 5d (Blocks). This section becomes
a short summary + a pointer.

### 5n. Reset / Init / Flush / Drain (keep, structure)

Per-element rather than per-section. Each storage / queue / FSM
declares its reset behavior in its 5d / 5g entry; this section
summarizes cross-cutting concerns (system reset sequencing,
flush propagation).

### 5o. Cycle-Accurate Behavior (NEW, optional)

For pipelined designs, a cycle-by-cycle table showing what each
stage does on each cycle. RV12's "instructions in flight"
diagram is the canonical example. Hard to author from prose;
easy to validate once you have it. Likely depends on the figure
extraction work to lift this from the source spec faithfully.

### 5p. Figures (NEW)

Per-figure entry:

- Figure name + role
- Source-page reference
- Rendered raster path (under `images/`)
- Caption (structured: elements, connections, controls)
- Referenced elements (signal / block names that appear in the
  figure, joinable against 5d signal tables)

Shape depends on the figure-extraction work; see
[spec-ingest-figure-extraction.md](spec-ingest-figure-extraction.md)
§7.

### 5q. Worked Examples (keep)

Already right — concrete traces driving behavior verification.

### 5r. Source-Spec Anchor Map (NEW)

A small section at the end mapping `spec.md section ID` →
`{source_spec_id, page, section_breadcrumb}`. Auto-generated
during DM0; consumed by DM1+ when they need to re-fetch
source-spec context via L2.

### 5s. Open Questions, Auto-decisions (keep)

Already right.

## 6. What we drop or change from the current template

- **Verbose "describe X" prose subsections** where the source
  spec already says it and L2 can retrieve. These currently
  encourage the agent to bulk-paraphrase. With L2, that work
  is wasted.
- **The single-interface-subsection-per-interface pattern.**
  Gets replaced by structured tables under 5d Blocks and 5e
  External Interfaces. The signal table is the contract; the
  surrounding prose is supporting context.
- **"Internal Interfaces" as a separate top-level section.**
  Folded into 5d Blocks.
- **Parameter-per-subsection.** Becomes a single typed table.
- **State / storage / arbitration prose subsections inside
  Functional Behavior.** Distributed across 5d Blocks (per-
  block state), 5g State Machines (FSMs), 5l Functional
  Behavior (cross-cutting flow).

## 7. What stays the same

- Metadata, Worked Examples, Open Questions, Auto-decisions.
- The gates-per-cycle requirement (now a table cell instead of
  prose, but the rule stays).
- The "Single-file vs paginated `docs/spec/`" choice in DM0.
- The principle that spec.md is gated on file existence + content
  checks.

## 8. Interaction with L2 (source-spec RAG)

Two design choices that depend on what the L2 RAG looks like in
practice:

### 8a. Anchor granularity

Source-spec anchors in spec.md need to point at something L2
can re-fetch. Options:

- **Page-level anchors** (`source_spec.md:p13`) — coarse,
  matches today's pagination, easy to mechanize. Loses
  structure when a page covers multiple concepts.
- **Section-breadcrumb anchors** (`source_spec.md§"Execution
  Pipeline > Instruction Fetch (IF)"`) — depends on L0a
  (section-based chunking) being live. More precise; survives
  re-pagination.
- **Chunk-id anchors** (`chunk_id: rv12_chunk_42`) — most
  precise but opaque to humans reading spec.md.

Probably want a mix: human-readable breadcrumb + a stable
chunk_id for mechanical re-fetch.

### 8b. When the spec.md gets out of sync with the source

If the source spec is updated after DM0 lands a spec.md, the
anchors become stale. Detection: hash the chunk content into
the anchor; if rebuild yields a different hash, flag stale.
Recovery: re-run DM0 on the changed chunks only. Out of scope
for this brainstorm but worth tracking.

## 9. Interaction with L7 (signal-table index)

L7 is the structured Lance table populated by the L0f signal-
table extractor. The relationship to spec.md:

- spec.md's 5d Blocks subsections **own** the signal tables for
  blocks DM0 has analyzed.
- L7 indexes the **same rows** for query purposes, with back-
  references to the spec.md location.
- Source-spec signal tables (extracted by L0f from the source)
  also land in L7 with provenance.
- The agent can ask "give me all signals driven from PD" and
  get rows that may live in spec.md, in the source spec, or
  both.

Open question: when spec.md's signal table conflicts with the
source spec's signal table (different signal name, different
direction), which wins? Probably spec.md (it's the
normalization); L7 should surface the conflict.

## 10. Interaction with figure extraction

The 5p Figures section assumes faithful figure rasters and
structured captions, both of which depend on the
figure-extraction work in
[spec-ingest-figure-extraction.md](spec-ingest-figure-extraction.md).

Until that work lands, 5p degrades gracefully: figure entries
can still carry source-page references and a free-form caption,
just without structured element / connection lists.

## 11. Open questions

1. **How much prose belongs in spec.md at all?** B says "less";
   how much less? A few sentences per block + the structured
   table? A paragraph per top-level concern? Need to author one
   spec.md against RV12 by hand and measure.
2. **Single template or per-flow templates?** DMF, DSF, SVF
   (and whatever else) may need different spec.md shapes. Or
   the same skeleton with optional sections. Tension between
   one-true-form and per-flow customization.
3. **Schema stability.** Tables in spec.md will get parsed by
   gate logic, by L7, by downstream artifacts. How rigid is
   the schema vs. how much do we let the agent improvise?
   Brittle schemas force prompt complexity; loose schemas
   defeat the structured-artifact win.
4. **Validation strategy.** Today the gate is "file exists +
   regex." With B, the gate could be "TOML-roundtrips +
   foreign keys valid (every signal table's `peer` references
   a real block)." Bigger investment; bigger payoff.
5. **How structured is too structured?** There's a slippery
   slope where spec.md becomes a YAML/TOML form fronted by
   markdown. We probably want a markdown table form that's
   parseable but still readable. Worth prototyping.
6. **Per-block depth cap.** Hardware hierarchies can nest 3–4
   levels (SoC → subsystem → block → submodule). Template
   needs to allow recursion without descending into
   `docs/spec/<a>/<b>/<c>/...` filesystem depth.
7. **What does the agent's authoring loop look like?** Today
   DM0 fills the template top-to-bottom. With structured
   tables, it might be more natural to fill in stages:
   metadata → blocks (one at a time) → cross-cutting concerns.
   Affects prompt design.
8. **Backward compatibility for existing projects.** rgb_toy
   already has a spec.md written against the old template.
   Restructure breaks it. Migration path? (Probably:
   regenerate via DM0 reset; cost is bounded since projects
   are small.)

## 12. Bottom line (brainstorm-level)

Three things this direction commits to without locking the
exact shape:

1. **spec.md becomes structured-artifact-bearing** — the per-
   block signal tables, FSMs, parameter tables, error tables,
   encoding tables that the four real specs all carry become
   first-class. The prose around them is supporting context,
   not the primary representation.
2. **Source-spec anchors are first-class.** Every structured
   artifact and every prose subsection points back to its
   source. DM1+ can re-fetch via L2; the human reviewer can
   audit the normalization.
3. **L7 (signal-table index) makes spec.md content
   query-friendly.** The agent doesn't have to read spec.md
   linearly; it queries by signal / block / interface and gets
   targeted rows. spec.md remains the human-readable
   normalized form; L7 is its machine-queryable mirror.

What this doc does NOT do:

- Pick the exact schema for each structured table.
- Specify gate-check changes.
- Define the agent's authoring prompt.
- Settle the figure / caption shape (deferred to the
  figure-extraction brainstorm).
- Commit to a migration path for existing projects.

Those are architecture / implementation questions for the next
documents. This brainstorm only stakes out the direction and
exposes the open questions that direction has to answer.
