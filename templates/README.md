# sim-flow templates

Project templates consumed by `sim-flow new`. Templates are expanded by
`cargo generate`, with `sim-flow` supplying the runtime placeholder values.

| Directory | Status | Subcommand |
| --------- | ------ | ---------- |
| [model-project/](model-project/) | implemented (Phase 2) | `sim-flow new model <name>` |
| [study-project/](study-project/) | stub (Phase 5) | `sim-flow new study <name>` |
| [candidate-project/](candidate-project/) | stub (Phase 5) | `sim-flow new candidate <name>` |

## Placeholders

Placeholders are inert `{{token}}` sequences in template files. `sim-flow`
provides concrete values at generation time so the resulting project carries
its own framework pin and generator provenance.

Standard tokens:

- `{{project-name}}` - user-provided name (as typed)
- `{{crate_name}}` - snake_case of project-name
- `{{foundation_repo}}` - sim-foundation git URL
- `{{foundation_rev}}` - exact sim-foundation git revision from sim-flow's Cargo.lock
- `{{library_path}}` - relative path to `sim-models/library/` (default: `../../library`)
- `{{sim_flow_repo}}` - sim-flow git URL
- `{{sim_flow_rev}}` - sim-flow git revision that generated the project
- `{{sim_flow_version}}` - sim-flow crate version that generated the project
- `{{timestamp}}` - ISO-8601 generation timestamp (UTC)
