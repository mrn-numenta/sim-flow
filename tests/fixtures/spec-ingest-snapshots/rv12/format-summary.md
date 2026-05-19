# RV12 — Format-Discovery Summary (Phase 9 Milestone 9.15)

This is a *first-cut* descriptor captured with `sim-flow ingest
--no-format-discovery`. The LLM critique pass (`format::discover`) is
not yet wired into the CLI — that plumbing was deferred at milestone
9.6. The numbers below therefore reflect the deterministic heuristic
classifier (`format::first_cut`) only. Future re-runs with the LLM
critique will likely lift many of the `unknown` items into typed
roles / kinds.

## Source

- Path: `/Users/mneilly/nta/sim-models/users/mneilly/rv12/docs/RV12 RISC-V CPU Core.pdf`
- SHA256: `af487088fe5bc42610b40cea36860e3567e7d4295b587e2038999bcb9935c8bd`
- Pipeline commit (worktree HEAD at capture):
  `73f88232d925d1b431e06a96ca629c975bc6927a`

## Descriptor metadata

- `model`: `first-cut-builtin`
- `prompt_version`: `first-cut-v1`
- `discovered_at`: `1970-01-01T00:00:00Z` (redacted; first-cut sets epoch)

## Counts — `format.json`

`format.json` carries one record per detected entity. The `validation`
block embedded in `format.json` is left at zeros by the first-cut
path; the stderr summary printed at the end of `sim-flow ingest` is
the authoritative live count. Stable counts reproducible from the
committed `format-firstcut.json`:

- `section_roles` total: 236
  - assigned (non-unknown): 31
    - `security_boundaries`: 6
    - `csrs`: 8
    - `block`: 6
    - `reset_init_flush_drain`: 5
    - `pipeline_and_hierarchy`: 3
    - `parameters`: 1
    - `encodings`: 1
    - `external_interfaces`: 1
  - `unknown`: 205
- `tables` total: 82
  - `signal_table`: 5
  - `encoding_table`: 2
  - `csr_table`: 2
  - `parameter_table`: 2
  - `pmu_event_table`: 1
  - `error_table`: 1
  - `csr_field_table`: 1
  - `unknown`: 68
- `figures`: 55 (all `kind=generic` — figure-kind heuristics
  don't fire on a body-heavy spec)
- `glossary`: 11
- `chrome`: 0 explicit regex entries (chrome-stripping handled by
  the legacy positional-banding pass)
- `validation.warnings`: 0

## Counts — `manifest.toml`

- `primary_chunk_count`: 52
- `primary_figure_count`: 55
- `primary_signal_table_count`: 2
- `primary_parameter_table_count`: 0
- `primary_csr_count`: 13
- `primary_csr_field_count`: 5
- `primary_memory_region_count`: 0
- `primary_pmu_event_count`: 3
- `primary_latency_row_count`: 0
- `primary_glossary_count`: 11
- `primary_stub_count`: 0
- `primary_tbd_count`: 0
- `total_lines_stripped` (chrome): 594
- `repeated_lines` (chrome): `[/26, 11:15 PM, RV12 RISC-V -bit CPU
  Core | RV12 RISC-V CPU Core, https://roalogic.github.io/RV12/DATASHEET.html]`

## Pipeline warnings (printed to stderr; copied into `manifest.toml`)

Seven `classify_unknown_canonical` warnings emitted (all in stage 4).
These are first-cut limitations where source headers don't yet map to
spec_md canonicals:

- T01 `Abbreviation → description` (glossary table — `Abbreviation`
  should canonicalise to `term`/`acronym`).
- T18, T24, T40 — `Description` column failing to canonicalise on
  non-row-typed tables (likely encoding / error contexts).
- T42 — `Interrupt`, `Exception Code`, `Description` columns all
  unmapped (RV12 trap-cause table; should classify as
  `error_table` with `name` / `code` / `description`).

## Manual review

- **Glossary**: populated with 11 expansion entries:
  `BPU`, `CPU`, `EX`, `ID`, `IF`, `MEM`, `PC`, `PD`, `PLIC`,
  `PMA`, `WB`. Captures every pipeline stage and the key
  acronyms. Missing: `CSR`, `RISC-V`, `MMU`, `PMP`, `IRQ`,
  `EBR` — body text mentions these without an explicit
  `Foo (BAR)` declaration, so the first-cut regex doesn't pick
  them up. The LLM critique should fix this.
- **Signal tables → block names**: 5 signal tables emit, all
  with the correct pipeline-stage `spec_md_target.block_name`:
  - T02 → `Instruction Fetch (IF)`
  - T05 → `Pre-Decode (PD)`
  - T07 → `Instruction Decode (ID)`
  - T10 → `Execute (EX)`
  - T17 → `Write-Back (WB)`
  - However only **2** files land under
    `primary/tables/signals/` (`000-instruction-fetch-if.toml`,
    `001-pre-decode-pd.toml`). The other 3 signal tables exist
    in `format.json` but their typed-emit path skipped them —
    likely because the classify-stage wrap-strategy or
    column-mapping rejected the rows. Followup work.
  - Memory-Access (MEM) is missing from the signal-table list
    entirely; the heuristic flagged it as a block via the
    acronym pattern but didn't pair it with a signal table.
- **CSR tables**: 2 detected (T25 page 39, T28 page 41) — these
  are the standard RISC-V machine-mode CSR block and the PMP
  block, both correctly classified. `primary_csr_count = 13`
  (rows across both tables; aligns with the M-mode `mvendorid /
  marchid / … / mtvec / mcounteren / mnmivec` set + PMP CSRs).
  `primary_csr_field_count = 5` — the per-field decomposition
  table (T-csr_field_table) found only one of the many register
  layouts.
- **PMU events**: 3 events recorded — first-cut found one
  `pmu_event_table` on page 32-ish (machine-mode counter
  description). RV12 doesn't define many HPMCounters, so 3 may
  be the whole set.
- **Outstanding `unknown` items to refine via `--rediscover-format`**:
  - 68 / 82 tables remain `unknown`. The headers in question include
    `Bit / Field / Description` style register-layout tables (these
    *are* CSR-field tables and should reclassify after column-map
    expansion).
  - 205 / 236 section_roles unclassified — top-of-document
    sections (Contents, Product Brief, Introduction, References)
    and many sub-sections. The role heuristics need to extend to
    a few additional phrasings, but this also reflects normal
    "narrative" sections that simply don't carry a spec_md role.
  - The trap-cause table (T42) — should reclassify as
    `error_table` once the LLM lifts the `Interrupt` column to
    canonical `kind` and `Exception Code` to `code`.

## How to reproduce

```
cargo build --release --package sim-flow
PROJ=/tmp/p9-15-rv12
rm -rf "$PROJ" && mkdir -p "$PROJ"
./target/release/sim-flow --project "$PROJ" ingest \
    --source "/Users/mneilly/nta/sim-models/users/mneilly/rv12/docs/RV12 RISC-V CPU Core.pdf" \
    --no-format-discovery
```

Compare `$PROJ/.sim-flow/spec-ingest/format.json` against the
checked-in `format-firstcut.json` (after redacting `discovered_at` and
adding a trailing newline). Compare
`$PROJ/.sim-flow/spec-ingest/manifest.toml` against the checked-in
`manifest.toml` after redacting `ingested_at`.
