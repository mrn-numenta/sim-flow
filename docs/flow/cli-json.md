# `sim-flow` JSON Output Schemas

Every subcommand with a `--json` flag emits a stable, machine-readable
representation of its result. The VS Code extension and other
non-interactive consumers rely on these shapes. Changes to any schema
here must be done via an explicit `SCHEMA_VERSION` bump in the
corresponding sim-flow module, and both CLI output and this document
updated together.

Conventions:

- All JSON output is pretty-printed with 2-space indent.
- Optional fields are emitted as `null` when absent; they are never
  omitted.
- Timestamps are ISO-8601 UTC (`"2026-04-22T21:00:52Z"`).
- `git_commit` is either a full SHA (40 hex chars) or the sentinel
  `"unknown-not-a-git-repo"` if the project is not a git repo.
- On failure (non-zero exit), JSON output is still emitted on stdout
  where possible, with a non-zero exit code signaling the failure.
  `sim-flow gate <step> --json` is the canonical example.

## `sim-flow status --json`

Snapshot of `.sim-flow/state.toml`.

```json
{
  "flow": "direct-modeling",
  "current_step": "DM0",
  "started": "2026-04-22T21:00:51Z",
  "gates": {
    "DM0": {
      "passed": true,
      "timestamp": "2026-04-22T21:00:52Z",
      "candidates": {}
    }
  },
  "archived_gates": {}
}
```

Fields:

| Field | Type | Notes |
| --- | --- | --- |
| `flow` | `"direct-modeling" \| "design-study"` | Active flow. |
| `current_step` | string | Step id (e.g. `"DM0"`, `"DS5a"`). |
| `started` | string or null | Project init timestamp. |
| `gates` | object | Map of step-id to Gate. |
| `archived_gates` | object | Populated by DS9 in-place transition. |

Gate shape:

```json
{
  "passed": false,
  "timestamp": null,
  "candidates": {
    "mesh": { "passed": true, "timestamp": "...", "candidates": {} }
  }
}
```

## `sim-flow runs --json`

Array of runs from `.sim-flow/experiments.db`, ordered by insertion id
descending. Filters (`--workload`, `--candidate`, `--study`,
`--sweep`, `--limit`) apply as with the human output.

```json
[
  {
    "id": 1,
    "run_id": "001-throughput",
    "timestamp": "2026-04-22T21:00:51Z",
    "git_commit": "unknown-not-a-git-repo",
    "git_branch": null,
    "git_dirty": false,
    "config_fingerprint": "59543c11940739d8",
    "manifest_path": null,
    "workload": "throughput-stress",
    "candidate": null,
    "study": null,
    "metrics_summary": "{\"throughput\":0.88}",
    "parent_run_id": null,
    "sweep_parameter": null,
    "sweep_value": null,
    "tags": null,
    "notes": null,
    "lifecycle": "active"
  }
]
```

`metrics_summary` is a JSON-encoded string (not a nested object) so the
DB schema stays simple. Consumers should `JSON.parse()` it after
reading.

## `sim-flow gate <step> --json`

Result of running gate checks for a step without spawning AI sessions.
Always emits on stdout; exit code is 0 if `clean` is true, non-zero
otherwise.

```json
{
  "step": "DM0",
  "clean": false,
  "failures": [
    {
      "description": "spec.md exists and is non-empty",
      "reason": "file missing: /path/to/project/spec.md"
    }
  ]
}
```

## `sim-flow baseline list --json`

```json
[
  { "name": "v1", "run_id": "001-seed", "timestamp": "2026-04-22T21:00:52Z" }
]
```

## `sim-flow baseline compare <name> [--current <id>] --json`

```json
{
  "baseline_run_id": "001-a",
  "current_run_id": "002-b",
  "entries": [
    {
      "metric": "throughput",
      "baseline": 0.80,
      "current": 0.88,
      "delta": 0.08,
      "delta_pct": 10.0
    },
    {
      "metric": "latency_p99",
      "baseline": null,
      "current": 11,
      "delta": null,
      "delta_pct": null
    }
  ]
}
```

`baseline`, `current`, `delta`, and `delta_pct` are `null` when the
metric is missing on one side or not numeric. Consumers must handle
this case explicitly; a missing metric is not zero.

## `sim-flow baseline create <name> [--run <id>] --json`

```json
{ "name": "v1", "run_id": "001-seed", "timestamp": "2026-04-22T21:00:52Z" }
```

## `sim-flow new model <name> --json`

```json
{
  "project_dir": "/Users/.../smoke-model",
  "crate_name": "smoke_model",
  "next_step": "DM0"
}
```

`project_dir` is absolute. `crate_name` is the snake_case identifier
used inside the generated `Cargo.toml`.

## Stability and Evolution

- Every field listed above is part of the public contract.
- New fields may be added without bumping the schema version if
  consumers treat unknown fields as opaque (recommended pattern).
- Removing or renaming a field is a breaking change and requires a
  `SCHEMA_VERSION` bump, an updated section here, and a `--json-v2`
  (or similar) opt-in on the CLI.
- Test coverage lives in `tools/sim-flow/tests/cli_json.rs` and
  runs on every PR; failing that suite blocks merge.
