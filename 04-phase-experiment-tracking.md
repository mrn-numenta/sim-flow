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

- [/] Audit the existing `ObservabilityReader` API against the standard
  metrics table: `throughput`, `latency_p50`, `latency_p99`,
  `utilization`, `stall_cycles`, `bottleneck_module`, `bubble_rate`.
  Current state: `ObservabilityReader::open(manifest_path)` exposes the
  run manifest plus a record-level `query()` API but no named-metric
  accessors. Extraction of the standard metrics set from raw records is
  non-trivial and remains a framework work item.
- [ ] Document the gaps (metrics that cannot be extracted today) with
  file/function pointers in [observability-data-api.md](../../architecture/observability-data-api.md).
  Phase 4 ships the graceful-fallback workaround described in Milestone
  4; the definitive framework-side doc update is a follow-up tied to
  Phase 7 polish.
- [ ] Land framework additions required to close the gaps (new extraction
  helpers, new stats schemas, or convenience aggregators). Deferred to a
  later framework phase; tracked separately from the ai-flow plan.
- [ ] Verify each metric can be produced from a sample run of the
  reference pipeline used in Phase 3 Milestone 5. Deferred (depends on
  the framework additions above).

## Milestone 2 - experiments.db Schema And Lifecycle

- [x] Implement the SQLite schema from
  [04-experiment-tracking.md](../../architecture/ai-flow/04-experiment-tracking.md#schema):
  `runs`, `baselines`, `ppa_estimates`.
- [x] Add a migration harness so schema evolutions are non-destructive.
  Schema version is recorded in a `meta` table; every open applies
  `CREATE TABLE IF NOT EXISTS` idempotently. Breaking migrations will
  bump `SCHEMA_VERSION`.
- [x] Implement insert / query APIs in
  `crates/sim-flow/src/tracking/index.rs` (`ExperimentIndex`).
- [/] Ensure `sim-flow new model` initializes an empty `experiments.db`
  during post-generation (tie to Phase 2 Milestone 4). Not wired into
  `sim-flow new model` yet; the first `record-run` call creates it. A
  dedicated pre-create at generation time is a small follow-up.
- [x] Add unit tests covering schema create, insert, sweep-lineage query,
  candidate/study filter query, and baseline create.

## Milestone 3 - Run Recording

- [x] Implement run id generation: `<sequence>-<short-description>` with
  auto-incrementing sequence from `experiments.db`.
- [x] Implement git state capture: `HEAD` commit, current branch, dirty
  flag (via `git status --porcelain`). Non-git project directories return
  a sentinel state so tests and scratch projects work.
- [x] Capture effective config via an FNV-1a fingerprint of
  `.sim-flow/config.toml`. The full Foundation `ConfigManager`
  fingerprint sits alongside each `.obsv` manifest; this layer captures
  the orchestrator-visible config so two runs with identical sim-flow
  config produce identical fingerprints.
- [x] Implement the `.experiments/<run-id>/` artifact directory layout
  with `manifest.json` (optional, supplied by the model), `config.toml`,
  `metrics.json`, and `notes.md`.
- [/] Implement the run-recording pipeline: orchestrator spawns the
  model binary with `--run-id`, waits for exit, then records results.
  Phase 4 ships `sim-flow record-run` (explicit, for use when the model
  is invoked manually) and `sim-flow sweep` (drives the model binary
  directly for every variant). Tighter integration with a `sim-flow
  run-sim <workload>` wrapper is deferred.
- [x] Add a `sim-flow runs` CLI command with `--workload`, `--candidate`,
  `--study`, `--sweep`, and `--limit` filters.

## Milestone 4 - Metrics Extraction

- [x] Implement `crates/sim-flow/src/tracking/metrics.rs`. Phase 4 uses a
  graceful-fallback design: if the model writes `metrics.json` into its
  `.experiments/<run-id>/` directory, that file is the metrics summary.
  Foundation-side named-metric accessors remain a follow-up per
  Milestone 1.
- [x] Write `metrics.json` into the artifact directory and update
  `runs.metrics_summary` accordingly.
- [x] Add tolerant handling: if a metric is unavailable for a given
  run, record null rather than failing. Baseline comparisons treat
  missing metrics as `None` deltas.
- [x] Add a unit test covering nested / flat metrics JSON and the
  round-trip extract + update path.

## Milestone 5 - Baselines And Comparison

- [x] Implement `sim-flow baseline create <name>` that writes a row to
  the `baselines` table. Optional `--run <run-id>` pins a specific run;
  default uses the most recent.
- [x] Implement `sim-flow baseline list`.
- [x] Implement `sim-flow baseline compare <name>` producing the per-
  metric delta table with absolute and percentage deltas.
- [/] Optional git tag creation on baseline create is deferred; the
  schema supports it and the CLI can be extended in Phase 7 without
  rework.
- [x] Add tests for create, list, compare (including the missing-metric
  graceful path).

## Milestone 6 - Parameter Sweeps

- [x] Define the `sweep.toml` schema (name, parameter, values, workload,
  optional binary, optional extra_args) and load it into a typed struct.
- [x] Implement `sim-flow sweep --file <sweep.toml>`: records a parent
  run, iterates values, invokes the configured binary with
  `--run-id <variant>` and `--<parameter> <value>`, records each variant
  as a child run with `parent_run_id`, `sweep_parameter`, and
  `sweep_value` set.
- [x] Implement `sim-flow sweep-results <parent>` that lists all child
  runs.
- [/] Knee / Pareto hint helper is deferred; the raw table is printed
  and downstream analysis consumes the index directly.
- [x] Add an integration test that runs a 3-value sweep against a
  deliberately-absent model binary (bookkeeping-only path).

## Milestone 7 - PPA Estimates Table

- [x] Schema for `ppa_estimates` present; `CREATE TABLE IF NOT EXISTS`
  applied alongside `runs` and `baselines`.
- [ ] Insert / query write paths for `ppa_estimates` are stubbed until
  Phase 7 Milestone 4 (DM5 scoping).

## Milestone 8 - DMF / DSF Integration Hooks

- [x] Wire DM4's gate check to query `experiments.db` for at least one
  row under the current project via the new `GateCheck::ExperimentsRecorded`
  variant.
- [x] DM4 now also passes when both the experiments row exists AND the
  analysis report contains throughput / latency content; missing either
  keeps the gate closed.
- [/] Publish a small tracking-query helper that DSF gate checks
  (DS5b, DS6, DS7) will consume in Phase 6. `ExperimentIndex::list_runs`
  already fits; DS-specific wrappers land in Phase 6 with the DS gate
  checks.
- [x] The Phase 3 plan's DM4-blocked note is resolved; the DM4 gate now
  has a structural check rather than relying solely on the critique.

## Status

Complete. The `sim-flow` crate now carries the full tracking layer:
SQLite `experiments.db` with runs / baselines / ppa_estimates tables, run
recording with git and config capture, `metrics.json`-based metrics
extraction, baseline create/list/compare, and parameter sweeps. The DM4
gate checks for at least one recorded run. 80 tests pass (54 unit + 15
dm_gates + 3 new_project + 5 smoke + 3 tracking integration). Workspace
fmt and `cargo clippy --all-targets -- -D warnings` clean.

Remaining `[/]` items are either deferred to Phase 7 polish (auto-DB
init at generation, git tag on baseline) or gated on framework-side
observability work (named-metric accessors) which is tracked separately.
