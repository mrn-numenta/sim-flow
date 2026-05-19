# Spatial Pooler 2.1 — Format-Discovery Summary (Phase 9 Milestone 9.15)

This is a *first-cut* descriptor captured with `sim-flow ingest
--no-format-discovery`. The LLM critique pass (`format::discover`) is
not yet wired into the CLI — that plumbing was deferred at milestone
9.6. The numbers below therefore reflect the deterministic heuristic
classifier (`format::first_cut`) only. Future re-runs with the LLM
critique will likely lift many of the `unknown` items into typed
roles / kinds.

## Source

- Path: `/Users/mneilly/Downloads/Spatial Pooler 2.1.pdf`
- SHA256: `d9373aa33438533b15b9a7f672ad38e0241856ad1f9b974800fc12845cc3a51d`
- Pipeline commit (worktree HEAD at capture):
  `73f88232d925d1b431e06a96ca629c975bc6927a`

## Descriptor metadata

- `model`: `first-cut-builtin`
- `prompt_version`: `first-cut-v1`
- `discovered_at`: `1970-01-01T00:00:00Z` (redacted; first-cut sets epoch)

## Counts — `format.json`

- `section_roles` total: 0 (none detected — the document uses
  styled body text rather than distinguishable heading clusters)
- `tables` total: 1
  - `unknown`: 1
- `figures`: 1 (`kind=generic`)
- `glossary`: 1
- `chrome`: 0
- `validation.warnings`: 0

## Counts — `manifest.toml`

- `primary_chunk_count`: 3
- `primary_figure_count`: 1
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
- `total_lines_stripped` (chrome): 230
- `repeated_lines` (chrome): `[None]`

## Pipeline warnings

None.

## Manual review

- **Parameters**: not detected. The Spatial Pooler document
  describes algorithm parameters (potential pool, permanence
  increment / decrement, boosting, etc.) in body text rather
  than in a tabular form. T01 (page 5) is the only detected
  table and remains `unknown`; manual inspection is needed to
  decide whether it's a parameter listing or a worked example.
- **Glossary**: 1 entry (`MSB = CT`) — clearly a false positive
  from the regex picking up a stray `MSB (CT...` fragment. The
  Spatial Pooler paper introduces SDR / HTM / SP terminology in
  prose, none of which match the `Name (ACR)` declaration
  pattern.
- **Section roles**: zero — the document's heading hierarchy
  isn't being detected. Likely the font-clustering pass collapses
  every line into a single cluster on this short, body-text-heavy
  document. This is a known weakness of the current `parse_pdf`
  heuristic on academic-paper layouts.
- **Chunk count**: 3, with 230 chrome lines stripped — the
  ingest treated most of the document body as chrome (the literal
  string `None` repeats per page in the chrome list, suggesting
  blank-bbox lines are being mis-classified as repeated chrome).
  This is the most surprising number on this spec and warrants
  followup.
- **Outstanding `unknown` items to refine via `--rediscover-format`**:
  - Section detection: needs the LLM to recognise heading-like
    spans even when font clustering doesn't separate them.
  - Glossary false positive (`MSB = CT`).
  - The 230-line chrome strip — likely too aggressive, possibly
    eating real content (only 3 chunks survived from a multi-page
    spec). The `repeated_lines = ["None"]` value in the manifest
    strongly suggests the chrome detector is treating absent /
    placeholder text as repeated boilerplate.

This spec is the weakest first-cut result of the four. Manual
descriptor authoring (`--format <path>`) may be the practical
path forward until the parse-stage font heuristics improve.

## How to reproduce

```
cargo build --release --package sim-flow
PROJ=/tmp/p9-15-spatial-pooler
rm -rf "$PROJ" && mkdir -p "$PROJ"
./target/release/sim-flow --project "$PROJ" ingest \
    --source "/Users/mneilly/Downloads/Spatial Pooler 2.1.pdf" \
    --no-format-discovery
```
