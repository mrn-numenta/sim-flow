# 4. Experiment Tracking

## Purpose

Define how simulation runs are recorded, indexed, and queried so that nothing
is ever lost and every result is reproducible. Experiment tracking bridges the
gap between Foundation's per-run observability primitives (RunManifest, .obsv
artifacts, config snapshots) and the multi-run workflows defined in the Direct
Modeling Flow and Design Study Flow.

## Design Principles

1. **Git stores the truth. An index makes it queryable.** Git branches capture
   the exact, reproducible state that produced results. A lightweight index
   (SQLite or TOML) provides fast metadata queries without requiring branch
   checkouts.

2. **Nothing is ever lost.** Every simulation run is recorded with enough
   information to reproduce it: the effective config snapshot, the git commit,
   and the manifest pointing to artifacts.

3. **The framework is the source of truth.** Binary `.obsv` artifacts produced
   by Foundation's ObservabilityRunWriter are the authoritative data. Index
   entries are derived summaries -- if there is a discrepancy, the `.obsv`
   data wins.

4. **Tracking is automatic.** The orchestrator records every run without
   requiring the user to do anything. Manual annotation (tags, notes) is
   optional.

## What Already Exists in Foundation

The orchestrator builds on these existing framework primitives:

| Primitive | Location | What It Provides |
| --------- | -------- | ---------------- |
| `RunManifest` | `observability_data.rs` | Per-run JSON manifest with run_id, format_version, artifact list, clock signals, config metadata |
| `ObservabilityRunWriter` | `observability_data.rs` | Writes trace.obsv, stats.obsv, activity.obsv, sanity.obsv, checkpoint_index.obsv |
| `ObservabilityReader` | `observability_data.rs` | Query API for reading artifacts, config snapshots, and export to JSONL/Parquet/Perfetto/FST |
| `ConfigManager` | `config.rs` | Effective config capture with FNV-1a fingerprint, source tracking (CLI/File/Default), JSON serialization |
| `CheckpointManager` | `manager.rs` | Checkpoint save/restore with path-based filtering, tick tracking |
| `compare_records()` | `observability_data.rs` | Binary comparison of two runs' trace records, produces TraceDiffReport |

## What Needs to Be Built

The experiment tracking layer adds:

1. **Run ID generation** -- deterministic, ordered identifiers for each run
2. **Run index** -- queryable metadata across all runs in a project
3. **Git branch management** -- snapshot branches for reproducibility
4. **Result extraction** -- key metrics extracted from .obsv for quick comparison
5. **Tagging and baselines** -- named reference points for comparison
6. **Sweep coordination** -- parameter variation with per-variant recording

## Run Identity

Each simulation run gets a unique identifier:

```text
<sequence>-<short-description>

Examples:
  001-throughput-stress
  002-hotspot-25pct
  003-sweep-buffer-depth-4
  003-sweep-buffer-depth-8
  003-sweep-buffer-depth-16
```

The sequence number is auto-incremented from the run index. The description
is derived from the workload name or sweep parameters. Sweep variants share
a parent sequence number with a distinguishing suffix.

The run_id is passed to Foundation's `RunManifest::new(run_id)` and
`ObservabilityRunWriter::new(output_dir, run_id)`.

## Run Index

A SQLite database at `.sim-flow/experiments.db` indexes all runs in the
project. The orchestrator writes to this after every simulation run.

### Schema

```sql
CREATE TABLE runs (
    id INTEGER PRIMARY KEY,
    run_id TEXT NOT NULL UNIQUE,
    timestamp TEXT NOT NULL,
    git_commit TEXT NOT NULL,
    git_branch TEXT,
    config_fingerprint TEXT NOT NULL,
    manifest_path TEXT NOT NULL,
    workload TEXT,
    candidate TEXT,
    study TEXT,
    -- Key metrics extracted from .obsv for quick comparison
    metrics_summary TEXT,           -- JSON: throughput, latency percentiles, etc.
    -- Sweep lineage
    parent_run_id TEXT,             -- parent run for sweep variants
    sweep_parameter TEXT,           -- parameter name being swept
    sweep_value TEXT,               -- parameter value for this variant
    -- User annotation
    tags TEXT,                      -- comma-separated tags
    notes TEXT,
    -- Lifecycle
    lifecycle TEXT DEFAULT 'active' -- active, archived, prunable
);

CREATE TABLE baselines (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    run_id TEXT NOT NULL REFERENCES runs(run_id),
    timestamp TEXT NOT NULL,
    notes TEXT
);

CREATE TABLE ppa_estimates (
    id INTEGER PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(run_id),
    level INTEGER NOT NULL,         -- 1 (analytical), 2 (calibrated), 3 (synthesis)
    technology_node TEXT NOT NULL,
    area_estimate REAL,
    power_estimate REAL,
    timing_met BOOLEAN,
    source TEXT,                     -- tool that produced the estimate
    timestamp TEXT NOT NULL
);
```

### Column Details

- `config_fingerprint`: The FNV-1a hash from `ConfigManager::snapshot_metadata()`.
  Two runs with the same fingerprint used the same effective config.
- `manifest_path`: Relative path to `run.obsv.manifest.json`. The manifest
  points to all .obsv artifacts for the run.
- `metrics_summary`: JSON extracted from .obsv via `ObservabilityReader` after
  the run completes. Contains key scalar metrics for quick cross-run comparison.
- `parent_run_id`: For sweep variants, points to the parent run that defined
  the sweep. For standalone runs, NULL.
- `candidate`: For Design Study runs, the candidate name. NULL for Direct
  Modeling runs.
- `study`: For Design Study runs, the study name. NULL for Direct Modeling runs.

## Git Recording

The orchestrator does not create branches or worktrees. The user is expected to be on a branch they created for the study/model work. Each run is recorded with whatever git state exists at the time of the run.

### Run Recording Workflow

```text
1. User initiates a run (via orchestrator or directly)

2. Orchestrator captures state:
   - Current git HEAD commit hash (recorded as git_commit)
   - Current branch name (recorded as git_branch)
   - Whether the working tree is dirty (recorded as a warning if so)
   - Effective config snapshot (ConfigManager::to_json_string())

3. Simulation runs:
   - Foundation writes .obsv artifacts and manifest using --run-id

4. Orchestrator records results:
   - Extract key metrics from .obsv via ObservabilityReader
   - Insert row into experiments.db with the git state from step 2
```

A dirty working tree is allowed but surfaced in `sim-flow status` and in the run's index row so later readers know the recorded commit does not fully describe the code that ran. If full reproducibility is required for a particular run (baseline, decision, publication), the user commits before running; the orchestrator does not commit on their behalf.

### Tags

Git tags are optional and applied by the user via `sim-flow baseline create --tag` (see Baseline Comparison below). Conventional names:

```text
baseline/<name>                  Named baseline
decision/<study>/<candidate>     Architecture selection
```

## Experiment Artifact Directory

Each run's artifacts are stored in a per-run directory:

```text
.experiments/<run-id>/
    run.obsv.manifest.json       # Foundation manifest
    trace.obsv                   # Binary trace (gitignored, reproducible)
    stats.obsv                   # Binary stats (gitignored, reproducible)
    activity.obsv                # Binary activity (gitignored, reproducible)
    config.json                  # Effective config snapshot
    metrics.json                 # Extracted key metrics (committed)
    notes.md                     # AI-generated interpretation (committed)
```

Heavy `.obsv` binaries are gitignored (reproducible by re-running). Config,
metrics, and notes are committed (institutional memory).

```gitignore
# Heavy binary artifacts (reproducible from code + config)
.experiments/*/*.obsv

# Keep metadata (not reproducible)
!.experiments/*/run.obsv.manifest.json
!.experiments/*/config.json
!.experiments/*/metrics.json
!.experiments/*/notes.md
```

## Metrics Extraction

After each run, the orchestrator extracts key metrics from .obsv artifacts
using the Foundation observability data API ([observability.md](../observability.md),
[observability-data-api.md](../observability-data-api.md)) and stores them as JSON in the run index
and as `metrics.json` in the artifact directory.

### Extraction Process

```text
ObservabilityReader::open(manifest_path)
    |
    +-- Query stats artifacts for throughput, latency, utilization
    |
    +-- Query activity artifacts for toggle counts, operation counts
    |
    +-- Compute derived metrics (bottleneck module, bubble rate, etc.)
    |
    +-- Write metrics.json
    |
    +-- Store metrics_summary in experiments.db
```

### Standard Metrics

| Metric | Source | Description |
| ------ | ------ | ----------- |
| throughput | stats.obsv | Items output per cycle |
| latency_p50 | stats.obsv | Median end-to-end latency |
| latency_p99 | stats.obsv | 99th percentile latency |
| utilization | stats.obsv | Per-module active fraction |
| stall_cycles | stats.obsv | Per-module stall count |
| bottleneck_module | derived | Module with highest stall count |
| bubble_rate | derived | Idle lanes / total lane-cycles |

The specific metrics extracted depend on what the model reports. The
orchestrator extracts whatever is available and stores it uniformly.

### Dependency on the Observability Data API

The metrics listed above assume the Foundation observability data API exposes named queries for throughput, latency percentiles, per-module utilization/stall, and lane-level activity. Any metric in the table that the current API cannot produce is an explicit framework pre-requisite and must be added to sim-foundation (new extraction helpers or new stats schemas) before the orchestrator can populate that column. The implementation plan should treat "audit the observability-data-api against this table" as a distinct task and schedule API additions ahead of the DM4 / DS7 gate work that depends on them.

## Parameter Sweeps

A sweep runs the same model with different config values, recording each
variant as a related run.

### Sweep Definition

```toml
# sweep.toml
[sweep]
name = "buffer-depth"
parameter = "noc.router.buffer_depth"
values = [4, 8, 16, 32]
workload = "throughput-stress"
```

### Sweep Execution

```text
1. Orchestrator reads sweep.toml
2. Creates parent run entry in experiments.db
3. For each value:
   a. Apply config overlay (ConfigManager::set_config_key)
   b. Run simulation
   c. Record as child run (parent_run_id = parent, sweep_parameter, sweep_value)
4. Aggregate results across variants
5. Identify best value (knee in the curve, Pareto optimal, etc.)
```

### Sweep Results

```text
  buffer_depth  throughput  latency_p99  area_estimate
  4             0.72        18           280 kGE
  8             0.85        12           340 kGE
  16            0.88        11           460 kGE
  32            0.89        10           700 kGE

  Knee: buffer_depth=8 (diminishing returns beyond)
```

## Baseline Comparison

A baseline is a named reference run. The orchestrator compares new runs
against baselines to detect regressions or improvements.

### Creating a Baseline

```text
sim-flow baseline create <name> [--run <run-id>]
```

If `--run` is omitted, uses the most recent run. Records the baseline in
the `baselines` table and optionally creates a git tag.

### Comparing Against a Baseline

```text
sim-flow baseline compare <name>
```

Reads the baseline's metrics and the current run's metrics, produces a delta
report:

```text
  Metric          Baseline    Current     Delta
  throughput      0.85        0.88        +3.5%
  latency_p99     12          11          -8.3%  (improved)
  area_estimate   340 kGE     340 kGE     --
```

## Cross-Run Queries

The SQLite index enables queries across all runs:

```sql
-- Best throughput per candidate
SELECT candidate, MAX(json_extract(metrics_summary, '$.throughput'))
FROM runs WHERE study = 'noc-exploration' GROUP BY candidate;

-- All runs for a specific workload
SELECT run_id, candidate, metrics_summary
FROM runs WHERE workload = 'throughput-stress' ORDER BY timestamp;

-- Sweep results for buffer depth
SELECT sweep_value, json_extract(metrics_summary, '$.throughput')
FROM runs WHERE parent_run_id = '003-sweep-buffer-depth' ORDER BY sweep_value;
```

## Integration with Flow Steps

### Direct Modeling Flow

| Step | Tracking Behavior |
| ---- | ----------------- |
| DM2c | Smoke test runs recorded but no branches |
| DM3c | Validation runs recorded, failures tracked |
| DM4 | Performance runs recorded, baselines created, sweeps executed |
| DM5 | PPA estimates recorded in ppa_estimates table |

### Design Study Flow

| Step | Tracking Behavior |
| ---- | ----------------- |
| DS5a | Per-candidate smoke runs recorded |
| DS5b | Per-candidate workload runs recorded with candidate tag |
| DS6 | Cross-candidate comparison queries from index |
| DS7 | Deep analysis runs and sweeps recorded |
| DS8 | Decision references run evidence from index |

## Orchestrator CLI

```text
sim-flow runs                           List recent runs
sim-flow runs --candidate mesh-noc      Filter by candidate
sim-flow runs --workload throughput     Filter by workload
sim-flow runs --sweep 003               Show sweep variants

sim-flow baseline create <name>         Create named baseline
sim-flow baseline compare <name>        Compare current vs baseline
sim-flow baseline list                  List all baselines

sim-flow sweep <sweep.toml>             Execute parameter sweep
sim-flow sweep results <run-id>         Show sweep results
```

## Implementation Location

Experiment tracking is part of the `sim-flow` orchestrator crate:

```text
sim-foundation/tools/sim-flow/src/
    tracking/
        mod.rs           # Public API
        index.rs         # SQLite experiments.db management
        metrics.rs       # Metrics extraction from .obsv
        sweep.rs         # Sweep coordination
        baseline.rs      # Baseline management
        git.rs           # Git branch/tag management
```

The tracking module depends on Foundation's `ObservabilityReader` for metrics
extraction and `ConfigManager` for config snapshots. It does not depend on
any specific AI client.
