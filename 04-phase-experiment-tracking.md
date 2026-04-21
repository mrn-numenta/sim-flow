# Phase 4 - Experiment Tracking

Phase dependency: Phase 1 (orchestrator), Phase 2 (model-project template,
`--run-id` contract). Also depends on the Foundation observability APIs
([observability.md](../../architecture/observability.md),
[observability-data-api.md](../../architecture/observability-data-api.md)).

## Problem Statement

DM4 (performance analysis), DS5b (candidate smoke validation), DS6
(comparison), DS7 (deep analysis), and DS8 (decision) all depend on being
able to record simulation runs, extract key metrics, and query across runs.
The framework already produces `.obsv` manifests and config snapshots, but
there is no cross-run index, no metrics extraction layer, and no baseline or
sweep coordination. This phase delivers the `tracking` submodule of
`sim-flow` and audits the observability data API for any gaps that block
the standard metrics set defined in
[04-experiment-tracking.md](../../architecture/ai-flow/04-experiment-tracking.md).

## Milestone 1 - Observability Data API Audit

- [ ] Audit the existing `ObservabilityReader` API against the standard
  metrics table: `throughput`, `latency_p50`, `latency_p99`,
  `utilization`, `stall_cycles`, `bottleneck_module`, `bubble_rate`.
- [ ] Document the gaps (metrics that cannot be extracted today) with
  file/function pointers in [observability-data-api.md](../../architecture/observability-data-api.md).
- [ ] Land framework additions required to close the gaps (new extraction
  helpers, new stats schemas, or convenience aggregators).
- [ ] Verify each metric can be produced from a sample run of the
  reference pipeline used in Phase 3 Milestone 5.

## Milestone 2 - experiments.db Schema And Lifecycle

- [ ] Implement the SQLite schema from
  [04-experiment-tracking.md](../../architecture/ai-flow/04-experiment-tracking.md#schema):
  `runs`, `baselines`, `ppa_estimates`.
- [ ] Add a migration harness so schema evolutions are non-destructive.
- [ ] Implement insert / query APIs in `crates/sim-flow/src/tracking/index.rs`.
- [ ] Ensure `sim-flow new model` initializes an empty `experiments.db`
  during post-generation (tie to Phase 2 Milestone 4).
- [ ] Add unit tests covering schema create, insert, sweep-lineage query,
  candidate/study filter query, and baseline create.

## Milestone 3 - Run Recording

- [ ] Implement run id generation: `<sequence>-<short-description>` with
  auto-incrementing sequence from `experiments.db`.
- [ ] Implement git state capture: `HEAD` commit, current branch, dirty
  flag (via `git status --porcelain`).
- [ ] Capture effective config via `ConfigManager::to_json_string()`.
- [ ] Implement the `.experiments/<run-id>/` artifact directory layout
  with `manifest.json`, `config.json`, `metrics.json`, `notes.md`.
- [ ] Implement the run-recording pipeline: orchestrator spawns the
  model binary with `--run-id`, waits for exit, then records results.
- [ ] Add a `sim-flow runs` CLI command with filters from doc 04.

## Milestone 4 - Metrics Extraction

- [ ] Implement `crates/sim-flow/src/tracking/metrics.rs` that opens the
  run manifest via `ObservabilityReader` and extracts the standard
  metrics set.
- [ ] Write `metrics.json` to the artifact directory and
  `metrics_summary` JSON to the `runs.metrics_summary` column.
- [ ] Add tolerant handling: if a metric is unavailable for a given
  run, record null rather than failing the pipeline.
- [ ] Add a unit test that uses a canned `.obsv` fixture from the
  Phase 3 reference pipeline and verifies the extracted JSON.

## Milestone 5 - Baselines And Comparison

- [ ] Implement `sim-flow baseline create <name>` that writes a row to
  the `baselines` table and optionally adds a `baseline/<name>` git
  tag.
- [ ] Implement `sim-flow baseline list`.
- [ ] Implement `sim-flow baseline compare <name>` producing the delta
  table from doc 04.
- [ ] Add tests for create, list, compare, and missing-baseline error
  paths.

## Milestone 6 - Parameter Sweeps

- [ ] Define the `sweep.toml` schema (name, parameter, values,
  workload) and load it into a typed struct.
- [ ] Implement `sim-flow sweep <sweep.toml>` that creates a parent run
  and child runs per value, applying config overlays through
  `ConfigManager::set_config_key`.
- [ ] Implement `sim-flow sweep results <run-id>` that queries child
  runs and prints the results table.
- [ ] Add a knee / Pareto hint helper (simple heuristic is fine for
  v1; surface the raw table and let the user judge).
- [ ] Add a sweep integration test that runs a 3-value sweep on the
  reference pipeline using the mock client and asserts the index rows.

## Milestone 7 - PPA Estimates Table

- [ ] Implement insert and query for the `ppa_estimates` table.
- [ ] Wire the table into the schema but leave the DM5 / DS7 write paths
  stubbed until Phase 7 finalizes DM5 scope.

## Milestone 8 - DMF / DSF Integration Hooks

- [ ] Wire DM4's gate check to query `experiments.db` for at least one
  run row under the current project.
- [ ] Publish a small tracking-query helper that DSF gate checks (DS5b,
  DS6, DS7) will consume in Phase 6.
- [ ] Add the documented DMF DMF4 unblock line to Phase 3 Milestone 4's
  status once this phase lands.

## Status

Not started. Schedule after Phase 3 Milestone 3 (DM3 end-to-end) so the
reference pipeline exists to produce real `.obsv` fixtures for Milestones
1 and 4.
