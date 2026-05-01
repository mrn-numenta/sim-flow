# Copilot Instructions for {{project-name}}

Mirrors `AGENTS.md`; see that file for the full project context. This file
exists so Copilot's path-sensitive instruction discovery picks up the same
guidance when editing files under this project.

## Key Rules

- Do not bypass the Foundation port system or implement custom scheduling.
- Modules have flopped inputs (framework invariant).
- Testbench components follow UVM-lite: Sequencer, Driver, Monitor,
  Scoreboard, SimEnv.
- The `{{crate_name}}` binary must honor `--run-id <id>` from sim-flow.
