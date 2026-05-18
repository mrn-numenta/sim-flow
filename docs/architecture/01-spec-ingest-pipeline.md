# Chapter 1: Spec Ingest Pipeline

This chapter specifies the replacement for the current
`spec_ingest.rs`. It is a multi-stage pipeline that consumes a
source spec (PDF, markdown, or plain text), with optional peer
specs declared via cross-references, and produces a structured
on-disk corpus the rest of the system consumes.

## 1.1 Purpose

The pipeline owns every transformation between "a file the user
handed us" and "structured artifacts the orchestrator, the
lance index, and the spec.md authoring loop can work with."
Specifically, it produces:

- **Section-anchored chunks** with full breadcrumb paths and
  source-page ranges that survive cross-page boundaries.
- **Structured tables** (signal tables, parameter tables, error
  tables, encoding tables, FSM tables) extracted from the
  source as typed TOML rather than markdown prose.
- **Faithful figure rasters** rendered from the source page,
  including all overlaid text and wires that the current
  embedded-image extractor drops.
- **Metadata signals** about ingest quality — stub sections,
  TBD occurrences, cross-spec references — that the
  orchestrator and DM0 consume directly.
- **A manifest** describing the corpus identity (source-spec
  hash, peer-spec links, ingest pipeline version, embedder
  expectations) that the lance index and the spec.md
  authoring loop key off of.

The pipeline is a CLI subcommand and a programmatic API; it is
not invoked implicitly. Re-ingestion is an explicit operation.

## 1.2 Inputs

Inputs to the pipeline:

- **Primary source spec**: one of
  - PDF file (`*.pdf`) — the dominant case.
  - Markdown file (`*.md`, `*.markdown`) — already structured
    text; most stages no-op.
  - Plain text file (`*.txt`, `*.text`) — treated as a single
    section of markdown after light normalization.
- **Optional peer specs**: zero or more additional files of the
  same kinds, registered as inherited / referenced specs (the
  SP→TM pattern). Peer specs are ingested with the same
  pipeline and recorded in the manifest. The agent's L2 RAG
  queries span the primary plus all registered peers.
- **Ingest configuration** (optional TOML) overriding defaults
  (DPI, chunk-size targets, language hints, etc.). Defaults
  apply when absent.

Inputs are passed by absolute path. The pipeline does not
fetch URLs or resolve relative paths against the project root;
that's the caller's responsibility.

If no primary source spec is provided (the user starts a
project from scratch with no document), the pipeline emits an
**empty corpus**: a manifest with `source_kind = "none"` and no
chunks, tables, or figures. Downstream consumers (the spec.md
authoring loop, the lance build) handle this gracefully —
DM0's interactive Q&A loop is the authoring path in that case.

## 1.3 Output Layout

All pipeline output lives under
`<project>/.sim-flow/spec-ingest/`. Directory contents:

```
.sim-flow/spec-ingest/
  manifest.toml                  -- corpus identity + peer links
  primary/
    chunks/
      000-<slug>.md              -- one file per section chunk
      ...
    tables/
      signals/
        000-<stage>.toml         -- one file per signal table
      parameters/
        000-<group>.toml
      errors/
        000.toml
      fsms/
        000-<machine>.toml
      encodings/
        000-<field>.toml
    figures/
      page-013.png               -- rendered figure raster
      page-013.caption.md        -- caption stub (free-form)
      ...
    stubs.toml                   -- list of stub sections
    tbds.toml                    -- list of TBD occurrences
    references.toml              -- cross-spec references found
  peers/
    <peer-id>/                   -- same layout as primary/
      manifest.toml
      chunks/
      tables/
      figures/
      ...
```

The directory structure is canonical: downstream consumers
(Ch. 2, Ch. 3, Ch. 6) hard-code these paths. The pipeline does
not write outside `.sim-flow/spec-ingest/`.

### 1.3.1 manifest.toml (top-level)

```toml
schema_version = 1
ingest_pipeline_version = "<sim-flow version>"
ingested_at = "<RFC3339 timestamp>"
source_kind = "pdf" | "markdown" | "text" | "none"
source_path = "<absolute path of original source>"
source_sha256 = "<hex digest>"
primary_chunk_count = 42
primary_figure_count = 8
primary_signal_table_count = 6
primary_stub_count = 0
primary_tbd_count = 11

[embedder_expected]
provider = "openai-compat"
model = "<model id>"
dimension = 1024

[[peers]]
id = "tm-spec"
source_path = "<absolute path>"
source_sha256 = "<hex digest>"
reason = "inherits-hardware-elements"   -- L0g classification
```

The `embedder_expected` block declares which embedder the lance
build will need to use. It is informational at ingest time;
Chapter 3 specifies how the lance index enforces it.

### 1.3.2 Per-chunk files (`chunks/NNN-<slug>.md`)

Each chunk is a markdown file with a YAML-fenced front matter
block followed by the chunk's markdown body. The numeric prefix
gives canonical ordering matching the source spec's TOC; the
slug is for human readability.

```
---
chunk_id: "<stable hash>"
breadcrumb: ["Introduction to the RV12", "Execution Pipeline", "Instruction Fetch (IF)"]
section_heading: "Instruction Fetch (IF)"
source_page_range: [13, 14]
kind: "prose"               -- prose | table | stub | figure | mixed
contained_signal_tables: ["signals/003-if.toml"]
contained_figures: ["figures/page-013.png"]
contained_table_refs: []
tbd_count: 0
---

<markdown body>
```

`chunk_id` is the SHA-256 of `breadcrumb || source_page_range
|| body`. Stable across re-ingests of the same source content;
changes if any of those three change. This is what L2 RAG
indexes; Chapter 3 specifies how.

`kind = "stub"` chunks have a `body` that is either empty or
contains only "TBD" / placeholder text. The chunk is preserved
so downstream consumers can enumerate it.

`kind = "mixed"` is used when a chunk contains both prose and
one or more structured tables that were extracted to
`tables/`. The chunk body retains the prose; the table refs
point at the extracted TOML.

### 1.3.3 Structured tables

Each extracted structured table is a TOML file with a header
identifying its kind and source.

Signal table (`tables/signals/003-if.toml`):

```toml
schema_version = 1
table_kind = "signal_table"
source_chunk_id = "<hash>"
source_page_range = [13, 14]
stage = "Instruction Fetch (IF)"   -- breadcrumb leaf
breadcrumb = ["Introduction to the RV12", "Execution Pipeline", "Instruction Fetch (IF)"]

[[rows]]
name = "if_nxt_pc"
direction = "out"           -- in | out | inout
peer = "Bus Interface"
description = "Next address to fetch parcel from"

[[rows]]
name = "parcel_pc"
direction = "in"
peer = "Bus Interface"
description = "Fetch parcel's address"
```

Parameter table (`tables/parameters/000-noc-channels.toml`):

```toml
schema_version = 1
table_kind = "parameter_table"
source_chunk_id = "<hash>"
source_page_range = [3, 4]
group = "NoC Channels"

[[rows]]
name = "NOC_REMOTE_NUM_CHANNELS"
default = "3"
comment = "If the mesh dimensions are modified, the number of channels must be scaled accordingly..."

[[rows]]
name = "NOC_REMOTE_DATA_WIDTH"
default = "64 Bytes"
comment = ""
```

Error table (`tables/errors/000.toml`):

```toml
schema_version = 1
table_kind = "error_table"
source_chunk_id = "<hash>"
source_page_range = [28, 29]

[[rows]]
error_type = "Wrong address / Address decode error"
detecting_component = "NoC / Slave Interface / Peripheral IP"
detecting_behavior = "Log Error"
bus_response = "Bus error"
master_behavior = "Log Error. Abort transaction"
software_response = "Interrupt"
```

Encoding table (`tables/encodings/000-privilege-levels.toml`):

```toml
schema_version = 1
table_kind = "encoding_table"
source_chunk_id = "<hash>"
source_page_range = [5]
field = "Privilege Level"
bit_width = 2

[[rows]]
value = "00"
name = "User/Application"
abbreviation = "U"

[[rows]]
value = "01"
name = "Supervisor"
abbreviation = "S"
```

FSM table (`tables/fsms/000-boot-fsm.toml`):

```toml
schema_version = 1
table_kind = "fsm_table"
source_chunk_id = "<hash>"
name = "Boot FSM"
reset_state = "IDLE"

[[transitions]]
from = "IDLE"
input = "power_on"
to = "RESET_HOLD"
output = "assert nReset"

[[transitions]]
from = "RESET_HOLD"
input = "stability_timer_done"
to = "RESET_RELEASE"
output = "deassert nReset"
```

Every structured-table TOML carries the same envelope:
`schema_version`, `table_kind`, `source_chunk_id`,
`source_page_range`, plus kind-specific top-level metadata and
`rows` / `transitions`. Schema versions are mutually
independent per kind; a v2 signal table can coexist with a v1
parameter table.

### 1.3.4 Figures

For each figure detected in the source:

- `figures/<id>.png` — the rendered raster. See §1.4 stage 6.
- `figures/<id>.caption.md` — a caption stub:

```
---
figure_id: "<stable id>"
source_page: 13
source_chunk_id: "<hash of the surrounding chunk>"
role: "<short label, e.g. 'IF block diagram'>"
referenced_elements: []        -- populated by captioning, empty at v1
---

<free-form caption; empty at v1 unless author supplies one>
```

The pipeline emits the raster and the stub. Captioning
(populating `role` and `referenced_elements`, and writing
caption body) is a separate concern with hooks in Chapter 6
but no v1 implementation.

### 1.3.5 stubs.toml / tbds.toml / references.toml

```toml
# stubs.toml
schema_version = 1

[[stubs]]
chunk_id = "<hash>"
breadcrumb = ["HTM (Hierarchical Temporal Memory) System"]
source_page = 5
hint = "section-heading-only"   -- section-heading-only | tbd-only | placeholder-text
```

```toml
# tbds.toml
schema_version = 1

[[tbds]]
chunk_id = "<hash>"
breadcrumb = ["IO System", "Boot ROM (Read Only Memory)"]
source_page = 10
context = "Numenta SoC will have TBD KB of Boot ROM in the IO System."
```

```toml
# references.toml
schema_version = 1

[[references]]
chunk_id = "<hash>"            -- chunk that contains the reference
peer_id = "tm-spec"            -- matches peers[].id in manifest.toml
reference_text = "see The Temporal Memory: A Detailed Description, sections 'Hardware Elements' and 'Default Parameters'"
referenced_breadcrumbs = ["Hardware Elements", "Default Parameters"]
```

## 1.4 Pipeline Stages

The pipeline is a fixed sequence of stages. Each stage reads
the previous stage's output and writes its own. Stages are
deterministic given the same input. The pipeline is restartable
from any stage given the preceding stages' outputs.

### Stage 1: Source loading

**Input:** raw source file path.
**Output:** normalized intermediate representation (in-memory):
either a `pdfium::Document` handle or a markdown / text string.
**Contract:** the source file is opened and validated. PDFs
that fail to parse return a hard error; markdown / text files
are read as UTF-8 with BOM stripped.

For PDF inputs, the loader uses pdfium-render and retains the
document handle through stages 2, 3, and 6.

### Stage 2: Page-chrome stripping

**Input:** the raw text content of each page (for PDFs,
extracted via pdfium's text extraction; for markdown / text,
the whole document treated as one "page").
**Output:** chrome-stripped page text plus a record of which
chrome lines were removed.
**Contract:** identify lines that appear on ≥50% of pages with
identical or near-identical content. Remove them from each
page's text before subsequent stages see it. Records the
removed lines in the manifest under
`[chrome_stripping]` for diagnostics.

Heuristic for "near-identical": exact match after collapsing
whitespace runs and stripping `Page \d+ of \d+` / `\d+ / \d+`
patterns. This catches the RV12-class repeated header banner
+ URL footer + page-number pattern.

Markdown and text inputs skip this stage (no chrome).

### Stage 3: Hierarchical parsing

**Input:** chrome-stripped page text.
**Output:** an in-memory tree of `Section { heading, level,
breadcrumb, body, page_range, children }`.
**Contract:** the parser walks the source recovering the
markdown-heading hierarchy. For PDFs, this means inferring
section structure from font sizes / styling / line patterns; a
PDF heading inference pass produces markdown-equivalent
headings before the tree is built. The TOC if present is used
as ground truth to validate inferred headings.

Each `Section`'s `body` is the markdown text between its
heading and the next sibling-or-higher heading. The tree is
flattened to chunks in Stage 7; this stage just builds it.

`page_range` tracks the source pages each section spans
(inclusive). A section starting on p13 and ending on p14
records `[13, 14]`.

The breadcrumb is the full ancestor chain — Chapter 2's spec.md
template expects this form. Heading collision (e.g. RV12's
"Instruction Fetch" appearing under both an overview section
and a deep-dive section) is disambiguated by the full
breadcrumb path; the leaf heading is preserved as-is.

### Stage 4: Structural classification

**Input:** the section tree.
**Output:** annotated section tree where each section carries
`kind: prose | table | stub | figure | mixed` and contained
structured tables are extracted.
**Contract:** the classifier walks each section and identifies:

- **Stub sections** (kind = `stub`): heading-only, or body
  contains only `TBD` / placeholder phrases. Recorded in
  `stubs.toml`.
- **Table-reassembly**: tables whose markdown rows span page
  boundaries are stitched together. The detector keys on
  consecutive markdown table rows separated by a page-break
  marker injected during Stage 1.
- **Signal-table detection**: tables whose header row matches
  the `Signal / Direction / To-From / Description` pattern (or
  variants — case-insensitive, optional column ordering). When
  matched, the table is extracted to `tables/signals/...toml`
  and the parent section's body is annotated with a marker
  referring to the extracted file.
- **Other structured-table detection**: parameter, error,
  encoding, FSM table patterns analogous to signals. Each kind
  has its own header-matching rules; mismatches stay as
  markdown tables in the prose body.
- **TBD detection**: regex `\bTBD\b` (whole-word) anywhere in
  the body. Recorded in `tbds.toml`.

The classifier is deliberately conservative: when in doubt,
leave the table as prose. Mis-extraction is worse than
under-extraction because downstream tools assume extracted
tables are typed correctly.

### Stage 5: Cross-spec reference parsing

**Input:** the classified section tree.
**Output:** the tree with `[[references]]` rows accumulated.
**Contract:** detect explicit cross-spec references using a
small set of patterns:

- "see <Spec Title>, section <breadcrumb>"
- "see <Spec Title>: <breadcrumb>"
- Markdown link to a peer spec file: `[<text>](<peer.md>)` or
  `[<text>](<peer.pdf>)`
- `## References` / `## Inherits-From` section content.

Each detected reference produces a row in `references.toml`.
The `peer_id` field is `null` until the caller registers a
peer spec via configuration; once registered, the pipeline
reconciles references to peer IDs.

This stage does not fetch or parse peer specs. Peer specs are
ingested by re-running the pipeline against each peer with the
primary's `peer_id` recorded in their manifest's
`[primary_link]` block.

### Stage 6: Figure extraction

**Input:** the pdfium document handle (PDF only — markdown /
text inputs skip this stage).
**Output:** rendered figure rasters under `figures/`, paired
with caption stubs.
**Contract:** for each PDF page, detect whether it contains
figure content (an embedded image XObject OR significant
non-text drawing ops). For pages with figure content:

1. Render the page at 150 DPI to a PNG using
   pdfium's `render_with_config` API.
2. Save as `figures/page-<NNN>.png`.
3. Generate the caption stub `figures/page-<NNN>.caption.md`
   with the surrounding chunk's id and source page.

v1 does NOT crop individual figures from a page; the whole
page is saved. Cropping requires per-figure bounding-box
detection, which is a future revision. Downstream consumers
(vision-model captioning, human review) work fine with whole-
page rasters at 150 DPI.

Pages with NO figure content do not produce a raster, even if
they contain prose. The current embedded-image extractor's
behavior of producing noise files (the 241-byte blue swatch
case) is eliminated by the "must contain figure content"
gate.

Rotation is NOT normalized at this stage. Captioning consumers
that need normalized orientation perform their own rotation.

DPI is configurable via the ingest configuration TOML; default
is 150. Format is PNG (lossless line art preservation). JPEG
is rejected as a v1 option because JPEG artifacts on hardware-
spec line drawings degrade legibility at the sizes specs use.

### Stage 7: Output emission

**Input:** the classified, annotated section tree plus the
extracted figures.
**Output:** all files under `.sim-flow/spec-ingest/` as
specified in §1.3.
**Contract:** the section tree is flattened to per-chunk
markdown files with YAML front matter (§1.3.2). Structured
tables are written to their typed locations. Metadata files
(`stubs.toml`, `tbds.toml`, `references.toml`, `manifest.toml`)
are written. The directory is atomic-replace: write to
`.sim-flow/spec-ingest.tmp/` then `rename` over
`.sim-flow/spec-ingest/`.

## 1.5 Failure Modes

The pipeline defines explicit behavior for each failure mode:

- **Source file missing or unreadable**: hard error. No partial
  output written. Caller (CLI / orchestrator) surfaces the
  error.
- **PDF parsing fails**: hard error. Recovery is not attempted.
- **Heading inference produces nonsense** (no headings found
  in a PDF): pipeline produces a single chunk covering the
  whole document, marked with `breadcrumb = ["<filename>"]`
  and `source_page_range = [1, N]`. A warning is recorded in
  `manifest.toml`. Downstream consumers can detect this case
  and degrade gracefully.
- **Signal-table extraction misfires** (header matched but
  body rows can't be parsed): the table is left as markdown
  prose; a warning is recorded in `manifest.toml`. No
  extracted-table file is produced.
- **Page rendering fails for a single page** (rare pdfium
  errors): the page-render is skipped, a warning is recorded,
  the rest of the pipeline continues.
- **Peer spec ingestion fails**: the primary spec's ingestion
  is not affected. The peer's failure is recorded in
  `manifest.toml.peers[].error`.

Any warning recorded in `manifest.toml` is surfaced to the
agent via DM0's initial state (Chapter 6 specifies).

## 1.6 Idempotence and Re-ingestion

The pipeline is idempotent given identical inputs:

- The same source spec + same configuration produces byte-
  identical output (modulo timestamps in `manifest.toml`).
- `chunk_id`s are stable across runs.
- Re-ingestion overwrites the previous output via atomic
  rename.

Re-ingestion is invoked explicitly via the CLI subcommand
(§1.8) or programmatically by the orchestrator when DM0
detects a stale source-spec hash. Stale detection: compare
`source_sha256` in the existing `manifest.toml` against a
fresh hash of the source file.

## 1.7 Configuration

A `<project>/.sim-flow/spec-ingest.config.toml` is read if
present. Defaults apply for all unset keys.

```toml
# defaults shown; all keys optional

[figures]
dpi = 150
format = "png"           -- only "png" supported in v1

[chunking]
max_chunk_chars = 8000   -- soft cap; sections larger than this trigger sub-chunking on the next-deepest heading level
min_chunk_chars = 200    -- chunks smaller than this are merged with the next sibling unless they're stubs

[chrome_stripping]
enabled = true
appearance_threshold = 0.5  -- line must appear on >= this fraction of pages

[signal_table_detection]
enabled = true
header_aliases = [
  ["Signal", "Direction", "To/From", "Description"],
  ["Signal", "Dir", "From/To", "Description"],
  -- additional patterns as discovered
]

[peers]
# Peer specs registered by the caller (orchestrator / CLI)
# Filled by the caller; not read from this file by default.
```

## 1.8 CLI Surface

```
sim-flow ingest \
    --source <path-to-primary> \
    [--peer <id>=<path>]... \
    [--config <path-to-config.toml>] \
    [--out <project-root>]

sim-flow ingest --rebuild [--out <project-root>]   -- re-ingest from the original source paths recorded in the existing manifest

sim-flow ingest --status [--out <project-root>]    -- emit manifest summary to stdout
```

The `ingest` subcommand operates on a project directory. The
project does not need to be initialized as a sim-flow project;
the subcommand creates `.sim-flow/spec-ingest/` if absent.

## 1.9 Programmatic API

The pipeline is exposed as a Rust API under
`sim_flow::session::spec_ingest::pipeline`:

```
pub struct IngestRequest {
    pub primary: SourceSpec,
    pub peers: Vec<(PeerId, SourceSpec)>,
    pub config: IngestConfig,
    pub project_root: PathBuf,
}

pub struct IngestOutcome {
    pub manifest_path: PathBuf,
    pub primary_chunk_count: usize,
    pub primary_figure_count: usize,
    pub primary_signal_table_count: usize,
    pub warnings: Vec<IngestWarning>,
}

pub fn run(request: IngestRequest) -> Result<IngestOutcome>;
```

The orchestrator's DM0 step invokes `run` programmatically; the
CLI subcommand is a thin wrapper. Both paths produce identical
output.

## 1.10 What This Chapter Does Not Specify

- The exact PDF heading-inference algorithm. The contract is
  "produces a section tree with `Section { heading, level,
  breadcrumb, body, page_range }`"; the implementation can use
  font-size analysis, table-of-contents matching, or both.
- The exact regex / parser shapes for each structured-table
  detector beyond the header-row alias list. Tuning is an
  implementation concern.
- The exact heuristic for "page contains figure content."
  Operational definition: any page with at least one image
  XObject OR more than a configurable threshold of vector
  drawing operations not classified as text. Threshold tuning
  is an implementation concern.
- Per-figure bounding-box cropping. Out of scope for v1; whole
  pages are rendered. Future revision.
- The captioning loop. Hooks (the `caption.md` stub) are
  defined here; the actual caption-generation flow is
  specified in Chapter 6.
- The implementation plan / sequencing. Subject of the
  implementation-plan doc that follows this architecture.
