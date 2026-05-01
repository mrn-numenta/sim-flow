# sim-foundation templates

Project templates consumed by `sim-flow new`. Templates are expanded by a
simple internal engine that substitutes `{{placeholder}}` tokens; no
external `cargo generate` dependency is required.

See [docs/architecture/ai-flow/05-templates.md](../docs/architecture/ai-flow/05-templates.md)
for the design and [docs/plan/ai-flow/02-phase-project-templates.md](../docs/plan/ai-flow/02-phase-project-templates.md)
for the implementation plan.

| Directory | Status | Subcommand |
| --------- | ------ | ---------- |
| [model-project/](model-project/) | implemented (Phase 2) | `sim-flow new model <name>` |
| [study-project/](study-project/) | stub (Phase 5) | `sim-flow new study <name>` |
| [candidate-project/](candidate-project/) | stub (Phase 5) | `sim-flow new candidate <name>` |

## Placeholders

Placeholders are inert `{{token}}` sequences in any template file. The
engine performs whole-token substitution, leaving unknown tokens intact.

Standard tokens:

- `{{project-name}}` - user-provided name (as typed)
- `{{crate_name}}` - snake_case of project-name
- `{{foundation_path}}` - absolute path to the sim-foundation repo
- `{{library_path}}` - relative path to `sim-models/library/` (default: `../../library`)
- `{{timestamp}}` - ISO-8601 generation timestamp (UTC)
