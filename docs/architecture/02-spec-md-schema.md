# Chapter 2: spec.md Schema

This chapter specifies the structured spec.md template that
replaces [`templates/model-project/docs/spec.md.tmpl`](../../templates/model-project/docs/spec.md.tmpl).
spec.md is the **structured normalization** of the source spec
— typed tables for the recurring hardware-spec content, prose
only where it carries meaning that doesn't fit a table, plus
explicit anchors back to the source spec for everything else.

## 2.1 Purpose

spec.md is the artifact DM0 produces and DM1 / DM2 / DM3 / the
critique pass / the gate engine all consume. It is:

- **Human-readable markdown.** Engineers review it directly.
- **Mechanically parseable.** A small parser produces typed
  Rust structs from the markdown by keying on heading
  patterns and column-header conventions. The parser
  round-trips spec.md tables to the same TOML shapes Chapter
  1 emits, so the gate engine and lance index treat spec.md
  signal tables identically to source-spec signal tables.
- **Anchor-bearing.** Every block, table, figure, and prose
  subsection carries an explicit reference back to the
  source-spec chunk it normalizes. DM1+ can re-fetch source
  context via L2 RAG by following the anchor.
- **Structured-artifact authoritative.** When a value appears
  in both spec.md and the source spec, spec.md is the
  normalized truth. Conflicts surface as warnings; spec.md
  wins.

## 2.2 Top-Level Structure

spec.md sections appear in fixed order. Required sections must
be present; optional sections are present only when relevant
to the design.

```
1.  # <Project Name> Design Specification    (H1, the document title)
2.  ## Metadata                              (REQUIRED)
3.  ## Purpose                               (REQUIRED)
4.  ## Scope                                 (REQUIRED)
5.  ## Non-goals                             (REQUIRED)
6.  ## Assumptions and Constraints           (REQUIRED)
7.  ## External Interfaces                   (REQUIRED if any)
8.  ## Blocks                                (REQUIRED)
9.  ## Parameters                            (REQUIRED if any)
10. ## State Machines                        (OPTIONAL)
11. ## Encodings                             (OPTIONAL)
12. ## Memory Map                            (OPTIONAL)
13. ## Connectivity                          (OPTIONAL)
14. ## Error Handling                        (OPTIONAL)
15. ## Functional Behavior                   (REQUIRED)
16. ## Timing, Latency, and Throughput       (REQUIRED)
17. ## Pipeline and Hierarchy                (REQUIRED)
18. ## Reset, Initialization, Flush, Drain   (REQUIRED)
19. ## Cycle-Accurate Behavior               (OPTIONAL)
20. ## Figures                               (OPTIONAL)
21. ## Worked Examples                       (REQUIRED)
22. ## Source-Spec Anchors                   (REQUIRED)
23. ## Open Questions                        (REQUIRED)
24. ## Auto-decisions                        (REQUIRED)
```

The order is fixed because the parser walks sections
sequentially. The presence of each REQUIRED section is checked
by the gate engine. OPTIONAL sections may be omitted entirely;
when present they are validated.

## 2.3 Sections in Detail

Each subsection below specifies:

- The section heading text (exact match required).
- Whether sub-structure is markdown prose or a typed table.
- Column-header conventions for tables.
- The TOML shape the parser round-trips to.
- Anchor placement rules.

### 2.3.1 Metadata (REQUIRED)

A definition-list-style block of key/value pairs:

```markdown
## Metadata

- Design name: RV12 RISC-V CPU Core
- Version: 1.0
- Status: draft
- Authors: Mike Neilly <mneilly@numenta.com>
- Source documents:
  - primary: docs/RV12 RISC-V CPU Core.pdf
  - peer: tm-spec → docs/temporal-memory.pdf
- Last updated: 2026-05-17
```

`Status` is one of `draft | reviewed | approved`. `Source
documents` lists the primary and any peer specs registered with
the ingest pipeline (matching `manifest.toml.peers[].id`).

Parses to:

```toml
[metadata]
design_name = "RV12 RISC-V CPU Core"
version = "1.0"
status = "draft"
authors = ["Mike Neilly <mneilly@numenta.com>"]
last_updated = "2026-05-17"

[[metadata.source_documents]]
role = "primary"
path = "docs/RV12 RISC-V CPU Core.pdf"

[[metadata.source_documents]]
role = "peer"
peer_id = "tm-spec"
path = "docs/temporal-memory.pdf"
```

### 2.3.2 Purpose, Scope, Non-goals (REQUIRED)

Each is one to three short paragraphs of prose. No tables.
Conciseness matters: with L2 RAG, verbose paraphrasing of the
source spec is wasted work.

### 2.3.3 Assumptions and Constraints (REQUIRED)

A small table of quantitative constraints plus prose for
environmental and architectural assumptions:

```markdown
## Assumptions and Constraints

### Quantitative

| Constraint | Value | Source-anchor |
| --- | --- | --- |
| Technology node | 7nm | source primary p3 |
| Clock frequency | 1 GHz | source primary p3 |
| Gate budget per cycle | 50-100 | derived (FO4 ~10ps at 7nm) |
| XLEN | 32 or 64 (parameterized) | source primary p3 |

### Environmental

<prose>

### Architectural

<prose>
```

The `Quantitative` table is required; clock frequency and gate
budget per cycle are required rows (the current
[`prompts/dm0-specification.md`](../../prompts/dm0-specification.md)
hard gate-checks for these). Source-anchor entries follow §2.4.

Parses each row to:

```toml
[[assumptions.quantitative]]
constraint = "Clock frequency"
value = "1 GHz"
source_anchor = "primary:p3"
```

### 2.3.4 External Interfaces (REQUIRED if any)

For each externally visible interface (top-level ports on the
design):

```markdown
## External Interfaces

### Interface: Instruction Interface

**Direction:** bidirectional
**Protocol:** AHB / Wishbone (parameterized)
**Clock domain:** core
**Connected peer:** instruction memory bus

#### Signals

| Signal | Direction | Width | Type | Required | Description |
| --- | --- | --- | --- | --- | --- |
| `inst_addr` | out | XLEN | logic | yes | Instruction address |
| `inst_data` | in | XLEN | logic | yes | Fetched instruction |

#### Transaction semantics

<prose>

#### Source-spec anchors

- primary:p2 (Product Brief block diagram)
- primary:p11 (RV12 Execution Pipeline overview)
```

The `### Interface: <name>` heading starts a new interface.
`<name>` is the unique interface identifier. The properties
list (Direction / Protocol / Clock domain / Connected peer) is
a fixed key-value block.

Signal-table column conventions (canonical, alias rules
specified in §2.5):

- `Signal` — string, code-styled signal identifier
- `Direction` — `in | out | inout`
- `Width` — string (parameter name like `XLEN`, or numeric like
  `64`, or expression like `XLEN-1:0`)
- `Type` — string (`logic`, `wire`, `bit`, payload-type name)
- `Required` — `yes | no` (or `y | n`)
- `Description` — free-form short text

Parses to:

```toml
[[external_interfaces]]
name = "Instruction Interface"
direction = "bidirectional"
protocol = "AHB / Wishbone (parameterized)"
clock_domain = "core"
peer = "instruction memory bus"
source_anchors = ["primary:p2", "primary:p11"]

[[external_interfaces.signals]]
name = "inst_addr"
direction = "out"
width = "XLEN"
type = "logic"
required = true
description = "Instruction address"
```

### 2.3.5 Blocks (REQUIRED)

The heart of the new template. Per-block subsections, nested
to whatever depth the design needs:

```markdown
## Blocks

### Block: Execution Pipeline

**Role:** Top-level pipeline orchestrating IF/PD/ID/EX/MEM/WB stages.
**Parent:** (none — top-level)
**Clock domain:** core
**Parameterized by:** `XLEN`, `HAS_BPU`, `HAS_RVC`

#### Source-spec anchors

- primary:p2 (Product Brief)
- primary:p6 (Execution Pipeline overview)

#### Sub-blocks

- [Instruction Fetch (IF)](#block-instruction-fetch-if)
- [Pre-Decode (PD)](#block-pre-decode-pd)
- [Instruction Decode (ID)](#block-instruction-decode-id)
- [Execute (EX)](#block-execute-ex)
- [Memory (MEM)](#block-memory-mem)
- [Write Back (WB)](#block-write-back-wb)

### Block: Instruction Fetch (IF)

**Role:** Loads instruction parcels from program memory.
**Parent:** Execution Pipeline
**Clock domain:** core

#### I/O Signals

| Signal | Direction | Peer | Description |
| --- | --- | --- | --- |
| `if_nxt_pc` | out | Bus Interface | Next address to fetch parcel from |
| `parcel_pc` | in | Bus Interface | Fetch parcel's address |
| `parcel_valid` | in | Bus Interface | Valid indicator for parcel |
| `parcel` | in | Bus Interface | Fetched parcel |
| `Flush` | in | EX/State | When asserted, flushes the pipe |
| `Stall` | in | PD | When asserted, stalls the pipe |
| `pd_branch_pc` | in | PD | New program counter for a branch instruction |
| `if_pc` | out | PD | Instruction Fetch program counter |
| `if_instr` | out | PD | Instruction Fetch instruction |
| `if_bubble` | out | PD | Instruction Fetch bubble |
| `if_exception` | out | PD | Instruction Fetch exception status |

#### State

- `pc` (XLEN-wide register, reset to RESET_VECTOR)

#### Behavior summary

<one to three short paragraphs of prose>

#### Source-spec anchors

- primary:p12-13 (IF section + block diagram)

#### Figures

- IF block diagram → figures/page-013.png

### Block: Pre-Decode (PD)

...
```

The Block I/O signal table uses the canonical column set:
`Signal | Direction | Peer | Description`. (External interfaces
use the longer six-column form because they include Width /
Type / Required at the SoC boundary; internal blocks use the
shorter four-column form because Width / Type are inherited
from the connected block.)

Parses each block to:

```toml
[[blocks]]
name = "Instruction Fetch (IF)"
parent = "Execution Pipeline"
role = "Loads instruction parcels from program memory."
clock_domain = "core"
source_anchors = ["primary:p12-13"]
figures = ["figures/page-013.png"]

[[blocks.signals]]
name = "if_nxt_pc"
direction = "out"
peer = "Bus Interface"
description = "Next address to fetch parcel from"

[[blocks.state]]
name = "pc"
width = "XLEN"
reset_value = "RESET_VECTOR"
description = ""
```

Nesting is by `parent` reference, not by markdown nesting
depth — all blocks are at heading level 3 (`### Block: <name>`)
regardless of where they sit in the hierarchy. This keeps the
parser simple and lets blocks reference parents that haven't
been declared yet.

### 2.3.6 Parameters (REQUIRED if any)

A single typed table:

```markdown
## Parameters

| Name | Type | Default | Valid range | Behavioral impact | Source-anchor |
| --- | --- | --- | --- | --- | --- |
| `XLEN` | int | 32 | 32 \| 64 | Sets data width and register width | primary:p3 |
| `HAS_BPU` | bool | true | true \| false | Enables branch prediction unit | primary:p9 |
| `BPU_LOCAL_BITS` | int | 8 | 0..16 | PC LSBs used for branch-prediction-table index | primary:p9 |
```

Parses to:

```toml
[[parameters]]
name = "XLEN"
type = "int"
default = "32"
valid_range = "32 | 64"
behavioral_impact = "Sets data width and register width"
source_anchor = "primary:p3"
```

### 2.3.7 State Machines (OPTIONAL)

Per-FSM subsections, each with a transitions table:

```markdown
## State Machines

### FSM: Boot FSM

**Reset state:** IDLE
**Source-spec anchor:** primary:p8-9

#### States

- `IDLE` — pre-power-on; waiting for power valid + Refclk
- `RESET_HOLD` — nReset asserted, awaiting stability duration
- `RESET_RELEASE` — nReset deasserted, FSM transitions through stages
- `BP_RUN` — Boot Processor running BootROM code

#### Transitions

| From | Input/Event | To | Output/Action |
| --- | --- | --- | --- |
| `IDLE` | power_on | `RESET_HOLD` | assert nReset |
| `RESET_HOLD` | stability_timer_done | `RESET_RELEASE` | begin reset deassertion |
| `RESET_RELEASE` | all_blocks_ready | `BP_RUN` | deassert BP reset |
```

Parses to:

```toml
[[state_machines]]
name = "Boot FSM"
reset_state = "IDLE"
source_anchor = "primary:p8-9"

[[state_machines.states]]
name = "IDLE"
description = "pre-power-on; waiting for power valid + Refclk"

[[state_machines.transitions]]
from = "IDLE"
input = "power_on"
to = "RESET_HOLD"
output = "assert nReset"
```

### 2.3.8 Encodings (OPTIONAL)

Per-field subsections:

```markdown
## Encodings

### Encoding: Privilege Level

**Bit width:** 2
**Source-anchor:** primary:p5

| Value | Name | Abbreviation |
| --- | --- | --- |
| `00` | User/Application | U |
| `01` | Supervisor | S |
| `10` | Hypervisor | H |
| `11` | Machine | M |

Reserved / illegal: none.
```

Parses to:

```toml
[[encodings]]
field = "Privilege Level"
bit_width = 2
source_anchor = "primary:p5"
reserved = []

[[encodings.values]]
value = "00"
name = "User/Application"
abbreviation = "U"
```

### 2.3.9 Memory Map (OPTIONAL)

```markdown
## Memory Map

| Start | End | Name | Purpose | Access | Source-anchor |
| --- | --- | --- | --- | --- | --- |
| `0x0000_0000` | `0x0FFF_FFFF` | BootROM | Initial boot code | R | primary:p10 |
| `0x1000_0000` | `0x1FFF_FFFF` | SRAM | System RAM | RW | primary:p11 |
```

Parses to:

```toml
[[memory_map]]
start = "0x0000_0000"
end = "0x0FFF_FFFF"
name = "BootROM"
purpose = "Initial boot code"
access = "R"
source_anchor = "primary:p10"
```

### 2.3.10 Connectivity (OPTIONAL)

For mesh / NoC / topology designs. Two sub-sections: nodes and
edges (or routing-rules description).

```markdown
## Connectivity

### Nodes

| Id | Type | Coordinate | Role |
| --- | --- | --- | --- |
| `CE0` | compute | `(1,3)` | Compute Engine |
| `CE1` | compute | `(2,3)` | Compute Engine |
| `ME0` | memory | `(0,3)` | Memory Engine |

### Edges

| From | To | Channel | Source-anchor |
| --- | --- | --- | --- |
| `CE0` | `CE1` | remote-W2E | primary:p4 |
| `CE0` | `ME0` | direct-W2E | primary:p5 |

### Routing rules

<prose: XY for remote, YX for sys, etc.>
```

### 2.3.11 Error Handling (OPTIONAL)

Single table:

```markdown
## Error Handling

| Error type | Detecting component | Detection behavior | Bus response | Master behavior | Software response | Source-anchor |
| --- | --- | --- | --- | --- | --- | --- |
| Wrong address / Address decode error | NoC / Slave Interface | Log Error | Bus error | Log Error. Abort transaction | Interrupt | primary:p28 |
```

### 2.3.12 Functional Behavior (REQUIRED)

Three subsections, prose-with-anchors:

```markdown
## Functional Behavior

### End-to-end behavior

<one paragraph>

### Operation flow

1. `Fetch` — Load instruction parcel from program memory (anchor: primary:p7)
2. `Pre-Decode` — Translate 16-bit compressed to 32-bit (anchor: primary:p7-8)
3. `Decode` — Read register file, calculate immediates (anchor: primary:p8)
...

### Data movement

<prose with anchors>
```

Each operation in the numbered list gets a stable identifier
(the backtick-quoted name) plus a source anchor. The parser
turns this into:

```toml
[[functional_behavior.operations]]
id = "Fetch"
purpose = "Load instruction parcel from program memory"
source_anchor = "primary:p7"
```

### 2.3.13 Timing, Latency, and Throughput (REQUIRED)

Prose subsections plus optional tables:

```markdown
## Timing, Latency, and Throughput

### Latency

| Operation | Best-case | Worst-case | Notes |
| --- | --- | --- | --- |
| Instruction fetch | 2 cycles | N cycles (ICache miss) | 2-cycle fetch when hit; cache miss stalls IF |

### Throughput

<prose>

### Stall and backpressure

<prose>
```

### 2.3.14 Pipeline and Hierarchy (REQUIRED)

Short prose summary that points at the Blocks section for
detail:

```markdown
## Pipeline and Hierarchy

The RV12 implements a 6-stage folded pipeline: IF → PD → ID →
EX → MEM → WB. Each stage is specified in detail under
[Blocks](#blocks). The Memory stage is folded into Execute
and Write-Back; the IF stage takes 2 cycles for compressed-
instruction recoding.
```

### 2.3.15 Reset, Initialization, Flush, Drain (REQUIRED)

Prose, cross-referencing the per-block reset behavior declared
in §2.3.5:

```markdown
## Reset, Initialization, Flush, Drain

### Reset

System reset is active-low (`nReset`), asynchronously asserted,
synchronously deasserted. Reset propagates to all blocks
declared in [Blocks](#blocks); per-block reset values are
declared in each block's `State` subsection.

### Initialization

<prose>

### Flush and drain

<prose>
```

### 2.3.16 Cycle-Accurate Behavior (OPTIONAL)

For pipelined designs. A table showing what each stage does on
each cycle for a representative scenario:

```markdown
## Cycle-Accurate Behavior

### Scenario: 6 instructions in flight (RV12 datasheet p7)

| Cycle | IF | PD | ID | EX | MEM | WB |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | I_A | — | — | — | — | — |
| 2 | I_B | I_A | — | — | — | — |
| 3 | I_C | I_B | I_A | — | — | — |
| 4 | I_D | I_C | I_B | I_A | — | — |
| 5 | I_E | I_D | I_C | I_B | I_A | — |
| 6 | I_F | I_E | I_D | I_C | I_B | I_A |

**Source-anchor:** primary:p7
```

### 2.3.17 Figures (OPTIONAL)

One entry per figure:

```markdown
## Figures

### Figure: IF block diagram

**Source page:** 13
**Raster:** [figures/page-013.png](figures/page-013.png)
**Role:** Instruction Fetch internal block diagram
**Referenced blocks:** Instruction Fetch (IF), Bus Interface

#### Caption

<free-form prose; for v1 either author-supplied, DM0-elicited via Q&A, or empty>

#### Elements depicted

| Element | Kind | Notes |
| --- | --- | --- |
| `if_nxt_pc` | signal | output of mux selecting pc+2 / pc+4 / ex_nxt_pc / st_nxt_pc |
| Mux (4:1) | block | next-PC selector |
| Flush/Stall | control | drives mux select |
```

The `Elements depicted` table is populated when captioning has
been done (author or future vision-model captioning); empty
when not. The figure raster always exists if the figure was
extracted by Chapter 1's pipeline.

### 2.3.18 Worked Examples (REQUIRED)

Prose with structure:

```markdown
## Worked Examples

### Example 1: Single ADD instruction through the pipeline

**Inputs:** PC=0x1000, instruction `add x1, x2, x3` at 0x1000
**Expected flow:**
1. Cycle 1: IF fetches parcel from 0x1000
2. Cycle 2: PD decodes (32-bit, no compression)
3. ...

**Expected outputs:** x1 ← x2 + x3 at cycle 6.
```

### 2.3.19 Source-Spec Anchors (REQUIRED)

An index mapping each spec.md section to its source-spec
chunks. Auto-generated by DM0 from per-section anchors; the
section is the canonical lookup table for L2 RAG re-fetch.

```markdown
## Source-Spec Anchors

| spec.md section | Source | Chunk id | Page range |
| --- | --- | --- | --- |
| External Interfaces > Instruction Interface | primary | chunk-0042 | 2-3 |
| Blocks > Instruction Fetch (IF) | primary | chunk-0118 | 12-14 |
| Parameters > XLEN | primary | chunk-0007 | 3 |
```

Parses to a flat lookup:

```toml
[[source_spec_anchors]]
section_path = "External Interfaces > Instruction Interface"
source = "primary"
chunk_id = "chunk-0042"
page_range = "2-3"
```

### 2.3.20 Open Questions (REQUIRED)

```markdown
## Open Questions

- BPU table size at default `BPU_LOCAL_BITS=8` not specified (primary:p9)
- Reset value for `if_exception` not stated (primary:p13)
```

Parses to a flat list of strings. The DM0 manual-mode Q&A loop
populates this from TBDs detected by the ingest pipeline (see
§2.7 and Chapter 6).

### 2.3.21 Auto-decisions (REQUIRED)

```markdown
## Auto-decisions

- Decided XLEN default = 32; rationale: source spec lists 32 and 64; embedded-market default is 32 per source p3.
- Decided BPU enabled by default; rationale: source spec p3 lists BPU as default-enabled feature.
```

Parses to:

```toml
[[auto_decisions]]
decision = "XLEN default = 32"
rationale = "source spec lists 32 and 64; embedded-market default is 32 per source p3"
```

## 2.4 Source-Spec Anchor Format

Anchors are short strings that point at a source-spec chunk
(matching `chunk_id` from Chapter 1's `chunks/NNN-<slug>.md`
front matter). Three forms:

- **Page form**: `<source>:p<N>` — e.g. `primary:p13`,
  `tm-spec:p7`. Resolves to the chunk whose
  `source_page_range` contains page N.
- **Page-range form**: `<source>:p<N>-<M>` — e.g.
  `primary:p12-13`. Resolves to the chunk whose
  `source_page_range` matches.
- **Chunk form**: `<source>:chunk-<NNN>` — direct reference.
  Used in §2.3.19 (Source-Spec Anchors) and anywhere precision
  matters.

`<source>` is either `primary` or a peer ID from
`manifest.toml.peers[].id`.

Anchor resolution is one-way: spec.md → source spec. The
reverse (which spec.md sections reference a given source
chunk) is computed by inverting §2.3.19's index at lance-build
time (see Chapter 3).

## 2.5 Column Alias Rules

Markdown tables in spec.md are parsed by matching the header
row against per-table-kind canonical column sets. Aliases are
permitted to ease authoring but produce a warning on the gate
check.

Signal table canonical columns (per §2.3.4, §2.3.5):

- `Signal` (aliases: `Name`, `Identifier`)
- `Direction` (aliases: `Dir`)
- `Peer` (aliases: `To/From`, `From/To`, `Connected to`) — Blocks
  only
- `Width` — External Interfaces only
- `Type` — External Interfaces only
- `Required` — External Interfaces only
- `Description` (aliases: `Notes`, `Meaning`)

Encoding table canonical columns:

- `Value`
- `Name`
- `Abbreviation` (aliases: `Abbr`)

Parameter table canonical columns:

- `Name`
- `Type`
- `Default`
- `Valid range` (aliases: `Range`, `Values`)
- `Behavioral impact` (aliases: `Impact`, `Effect`)
- `Source-anchor`

Error table, transitions table, memory map: see the respective
sections.

The parser produces a warning when an alias is used; the value
is normalized to the canonical column name on round-trip.

## 2.6 Validation Rules

The gate engine validates spec.md against this schema. A spec
passes the DM0 gate when:

1. **All REQUIRED sections present** with the correct heading
   text.
2. **All tables parse cleanly** — every row has the right
   column count and the right cell types.
3. **All cross-references resolve**:
   - Every block's `parent` references an existing block or
     `(none — top-level)`.
   - Every signal table row's `peer` references an existing
     block or external interface.
   - Every source-anchor resolves to a chunk in `manifest.toml`
     (primary or a registered peer).
4. **The Assumptions/Quantitative table contains required
   rows**: `Clock frequency` (matching `\d+\s*(MHz|GHz)`) and
   `Gate budget per cycle` (matching `\d+`). These are the
   current DM0 gate checks, preserved.
5. **No orphan figures**: every figure raster present under
   `figures/` is referenced from §2.3.17 Figures or marked as
   intentionally-skipped in `Open Questions`.
6. **Auto-decisions are populated when running in automated
   mode** (operator override allowed in manual mode).

Validation warnings (don't block the gate):

- Use of an alias column name instead of canonical.
- Empty `Behavior summary` on a block.
- Empty `Caption` on a figure (acceptable for v1).
- Missing anchor on a non-Stub section.

## 2.7 Required-Field Traversal (Authoring Loop Support)

DM0's manual-mode interactive Q&A loop (Chapter 6) needs to
know which fields are missing so it can ask the user about
them one at a time. The schema supports this via a
deterministic traversal:

1. Walk REQUIRED sections in order; flag any missing.
2. For each present REQUIRED section, walk its required
   subsections / rows; flag any missing or empty.
3. Walk OPTIONAL sections; do not require, but note their
   presence in the traversal record for "is this section
   applicable?" prompts.
4. Walk Open Questions; each entry is a candidate Q&A turn
   topic.
5. Walk Auto-decisions in automated mode; each non-trivial
   inference should produce an entry.

The traversal produces an ordered list of `MissingField`
entries that the DM0 manual-mode loop consumes (see Chapter 6
for the loop logic and the user-facing prompt templates).

In the **no-source-spec case** (the user starts from scratch),
the traversal IS the authoring loop: every REQUIRED field
starts empty, the agent asks the user about each one in turn,
and answers populate the corresponding section. Optional
sections start with the agent asking "is this section
applicable to your design?" before drilling in.

## 2.8 The Template File

The template file at
[`templates/model-project/docs/spec.md.tmpl`](../../templates/model-project/docs/spec.md.tmpl)
implements this schema. The template:

- Includes every REQUIRED section heading, even if the body is
  placeholder text.
- Includes table headers (column rows + separator rows) for
  every REQUIRED table, with example rows commented out.
- Includes OPTIONAL section headings as commented-out markdown
  (so the author or DM0 can uncomment as relevant).
- Carries a top-of-file comment block summarizing the
  authoring loop and pointing at this chapter.

The template MUST match the schema. A unit test in the
ingest-pipeline crate parses the template file with the spec.md
parser and asserts it round-trips cleanly to the expected
empty-but-valid TOML form.

## 2.9 The spec.md Parser

A parser module at
`sim_flow::session::spec_md::parser` reads spec.md and emits
a typed `SpecMd` struct. The parser:

- Walks markdown using `pulldown_cmark` (already a transitive
  dep via the existing markdown rendering).
- Keys on heading text (exact match) and column-header
  patterns (with alias normalization).
- Returns either `Ok(SpecMd)` or `Err(SpecMdParseError)` with
  the offending location (line + column).
- Round-trips: `SpecMd::to_markdown()` produces text that
  re-parses to the same `SpecMd`. Round-trip stability is the
  basis for gate validation that doesn't require the agent to
  match formatting byte-for-byte.

The parser is the single source of truth for spec.md ↔ TOML
conversion. Both the gate engine (validation) and the lance
build (Chapter 3) call into it.

## 2.10 What This Chapter Does Not Specify

- The exact parser implementation. Contract is "produces
  `SpecMd`; round-trips."
- The CLI for invoking the parser standalone. The implementation
  plan may add one for debugging; not required.
- Pretty-printer formatting choices (column widths, blank-line
  policies). Implementation concern; round-trip stability is
  the only formal requirement.
- The migration path for existing projects' spec.md files
  written against the old template. Treated in the
  implementation plan.
- The exact wording of DM0 prompts. Subject of Chapter 6.
