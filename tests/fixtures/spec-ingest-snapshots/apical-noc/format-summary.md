# Apical NoC — Format-Discovery Summary (Phase 9 Milestone 9.15)

This is a *first-cut* descriptor captured with `sim-flow ingest
--no-format-discovery`. The LLM critique pass (`format::discover`) is
not yet wired into the CLI — that plumbing was deferred at milestone
9.6. The numbers below therefore reflect the deterministic heuristic
classifier (`format::first_cut`) only. Future re-runs with the LLM
critique will likely lift many of the `unknown` items into typed
roles / kinds.

## Source

- Path: `/Users/mneilly/Downloads/apical noc spec.pdf`
- SHA256: `8b4c6a074339bfd82f9e2dfcac495091aee96f64ec69383b40bd8f140d02f04c`
- Pipeline commit (worktree HEAD at capture):
  `73f88232d925d1b431e06a96ca629c975bc6927a`

## Descriptor metadata

- `model`: `first-cut-builtin`
- `prompt_version`: `first-cut-v1`
- `discovered_at`: `1970-01-01T00:00:00Z` (redacted; first-cut sets epoch)

## Counts — `format.json`

- `section_roles` total: 19
  - assigned (non-unknown): 9
    - `connectivity`: 7
    - `reset_init_flush_drain`: 1
    - `power_domains`: 1
  - `unknown`: 10
- `tables` total: 9
  - all 9 currently `unknown`
- `figures`: 8
  - `connectivity_topology`: 1 (page 2 — the system NoC layout
    diagram. Heuristic correctly inferred from the adjacent
    "Physical Design of Apical NoC" heading.)
  - `generic`: 7
- `glossary`: 1
- `chrome`: 0
- `validation.warnings`: 0

## Counts — `manifest.toml`

- `primary_chunk_count`: 17
- `primary_figure_count`: 8
- `primary_signal_table_count`: 0
- `primary_parameter_table_count`: 0
- `primary_csr_count`: 0
- `primary_csr_field_count`: 0
- `primary_memory_region_count`: 0
- `primary_pmu_event_count`: 0
- `primary_latency_row_count`: 0
- `primary_glossary_count`: 1
- `primary_stub_count`: 0
- `primary_tbd_count`: 0
- `total_lines_stripped` (chrome): 69

## Pipeline warnings

None.

## Manual review

- **Parameter table on page 2**: the spec opens with a
  configuration / parameter table early in the document. The
  first-cut detected it (T01 page 2) but left `kind=unknown`.
  The columns probably read as `[Name, Value, Description]` or
  similar — close to the parameter heuristic but not a 4-of-4
  hit. Followup work for the LLM critique.
- **Memory map**: not detected as a typed section_role. The NoC
  spec discusses address spaces in the Section 4 "Noc
  Interfaces" body; there's no dedicated "Memory Map" heading.
  T05 (page 9) is a candidate memory-map table but remains
  `unknown`.
- **Connectivity coverage**: very strong — 7 of 9 assigned
  section_roles are `connectivity`. The Section 2 / 3 / 4 / 5
  headings on Physical Design, NoC Channels, NoC Interfaces,
  and NoC Routers all classified correctly. F02 (page 2) lifted
  to `connectivity_topology` is exactly the high-level system
  diagram one would want auto-populated.
- **Power domains**: page 1 mentions "domains" in the body and
  triggered `power_domains` on a sub-heading. Worth a sanity
  check via `--rediscover-format` (the heading is "domains. In
  contrast, the NoC operates within a si..." which suggests the
  span-clustering may have over-classified a body fragment as a
  heading).
- **Glossary**: only 1 entry detected (`REQ = Request`). The
  spec defines REQ / RSP / DAT channels and various router-side
  acronyms in prose without the canonical `Name (ACR)` pattern,
  so the regex misses them.
- **Outstanding `unknown` items to refine via `--rediscover-format`**:
  - All 9 tables. T01 (parameters), T05 (memory-ish), and the
    per-router interface tables (T07/T08) are highest value.
  - The `power_domains` false-positive on page 1.
  - The "domains. In contrast..." heading appearing as a section
    boundary suggests parse-stage span clustering may be picking
    up paragraph-start glyph weight changes as headings on this
    spec.

## How to reproduce

```
cargo build --release --package sim-flow
PROJ=/tmp/p9-15-apical-noc
rm -rf "$PROJ" && mkdir -p "$PROJ"
./target/release/sim-flow --project "$PROJ" ingest \
    --source "/Users/mneilly/Downloads/apical noc spec.pdf" \
    --no-format-discovery
```
