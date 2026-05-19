# Numenta SoC — Format-Discovery Summary (Phase 9 Milestone 9.15)

This is a *first-cut* descriptor captured with `sim-flow ingest
--no-format-discovery`. The LLM critique pass (`format::discover`) is
not yet wired into the CLI — that plumbing was deferred at milestone
9.6. The numbers below therefore reflect the deterministic heuristic
classifier (`format::first_cut`) only. Future re-runs with the LLM
critique will likely lift many of the `unknown` items into typed
roles / kinds.

## Source

- Path: `/Users/mneilly/Downloads/Numenta SoC Specification.pdf`
- SHA256: `e4a0b8d4094196525f9dba59ec6ed2cec3c67c2d4c794d782dfb8cd656715d36`
- Pipeline commit (worktree HEAD at capture):
  `73f88232d925d1b431e06a96ca629c975bc6927a`

## Descriptor metadata

- `model`: `first-cut-builtin`
- `prompt_version`: `first-cut-v1`
- `discovered_at`: `1970-01-01T00:00:00Z` (redacted; first-cut sets epoch)

## Counts — `format.json`

- `section_roles` total: 23
  - assigned (non-unknown): 5
    - `reset_init_flush_drain`: 3
    - `connectivity`: 1
    - `memory_map`: 1
  - `unknown`: 18
- `tables` total: 6
  - all 6 currently `unknown`
- `figures`: 13 (all `kind=generic`)
- `glossary`: 15
- `chrome`: 0
- `validation.warnings`: 0

## Counts — `manifest.toml`

- `primary_chunk_count`: 47
- `primary_figure_count`: 13
- `primary_signal_table_count`: 0
- `primary_parameter_table_count`: 0
- `primary_csr_count`: 0
- `primary_csr_field_count`: 0
- `primary_memory_region_count`: 0
- `primary_pmu_event_count`: 0
- `primary_latency_row_count`: 0
- `primary_glossary_count`: 15
- `primary_stub_count`: 0
- `primary_tbd_count`: 0
- `total_lines_stripped` (chrome): 17

## Pipeline warnings

None.

## Manual review

- **System block diagram detection**: the spec's title page +
  early figures landed in the manifest as 13 raster figures, all
  `kind=generic`. None were elevated to `block_diagram` /
  `connectivity_topology` by the first-cut. The Section 8 "NoC
  (Network-on-Chip)" heading did get a `connectivity` role,
  which is the closest hint a downstream consumer has.
- **Privilege levels**: not detected. There is no section headed
  with the canonical "Privilege Level" / "Mode" wording — the
  document uses "Boot Processor" / "Device Management Processor"
  framing. The LLM critique will need to pick these up via
  rationale-rich adjustments; the first-cut has no signal.
- **Memory map**: correctly identified — page 24 "Memory Map"
  section_role classified as `memory_map`. No table on that page
  was lifted into `memory_map_table` kind, however: the 6
  tables all remain `unknown`. The actual memory-map table is
  likely T01 (page 24) — header words probably weren't a strong
  match for the heuristic's `[Region, Base/Start, Size]` set.
- **Glossary**: 15 entries detected. Coverage looks reasonable
  (`BAR`, `BP`, `CRU`, `DMA`, `EP`, `GPIO`, `HTM`, `ISR`, `NIC`,
  `PIO`, `PLL`, `RC`, `SP`, `SRAM`, `WDT`). Several have noisy /
  truncated expansions where the PDF's multi-line definition
  broke the regex's single-line capture:
  - `BP = Device Management\nProcessor` (newline in expansion)
  - `EP = End\nPoint`
  - `GPIO = Output` (truncated; actual is "General-Purpose IO")
  - `HTM = Memory` (truncated; actual is "Host Transfer Memory")
  - `NIC = Card` (truncated; actual is "Network Interface Card")
  - `SP = The Boot Processor`
  - `WDT = The Watchdog Timer`
  These should clean up with multi-line glossary heuristics or
  via the LLM critique.
- **Outstanding `unknown` items to refine via `--rediscover-format`**:
  - All 6 tables. T01 (page 24, memory map) is highest-value —
    once classified, `populate_memory_regions` lights up.
  - 18 / 23 section_roles. The heuristic only fires on exact
    keyword phrasings; the Numenta SoC uses descriptive headings
    ("Boot Sequence and Boot Vector", "PCIe Device Reset
    Overview") that don't always trigger.

## How to reproduce

```
cargo build --release --package sim-flow
PROJ=/tmp/p9-15-numenta-soc
rm -rf "$PROJ" && mkdir -p "$PROJ"
./target/release/sim-flow --project "$PROJ" ingest \
    --source "/Users/mneilly/Downloads/Numenta SoC Specification.pdf" \
    --no-format-discovery
```
