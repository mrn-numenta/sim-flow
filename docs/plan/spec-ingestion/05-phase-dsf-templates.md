# Phase 5 - Design Study Flow Templates And Orchestrator Support

Phase dependency: Phase 2 (template machinery), Phase 4 (experiment
tracking). Benefits from Phase 3 but does not block on it.

## Problem Statement

Phases 2 and 3 deliver a working DMF against a single-project template.
The DSF needs its own project layout (Cargo workspace containing candidate
crates, a shared workloads crate, a comparisons tree, and a `final-model/`
slot for DS9), a per-candidate session management model in the orchestrator,
and a `sim-flow new candidate` subcommand for adding candidates to an
existing study. This phase delivers those pieces without yet authoring the
DS step instruction content (that is Phase 6).

## Milestone 1 - study-project Template

- [ ] Author `templates/study-project/` with the layout in
  [05-templates.md](../../architecture/ai-flow/05-templates.md#template-study-project).
- [ ] Include a root `Cargo.toml` with a `[workspace]` table listing
  `candidates/*`, `workloads`, and `final-model` members (the members
  may be empty stubs at generation time).
- [ ] Include placeholder `study.md`, `spec.md`, `targets.md`,
  `testbench.md` with structured TODO headings that DS0 and DS1 will
  fill in.
- [ ] Include the shared `workloads/` crate skeleton (library crate
  that depends on `foundation-framework` and exposes UVM-lite
  Sequencer/Driver/Monitor/Scoreboard scaffolding).
- [ ] Include `candidates/` and `comparisons/` with `.gitkeep`.
- [ ] Include `final-model/` with a `.gitkeep` and a README explaining
  DS9 populates this directory.
- [ ] Include `.sim-flow/state.toml` initialized at DS0 with
  `flow = "design-study"`, plus the standard `config.toml`,
  `experiments.db` placeholder, `critiques/`, `logs/` directories.
- [ ] Include `CLAUDE.md`, `AGENTS.md`, `.claude/settings.json`,
  `.github/copilot-instructions.md` tailored to DSF language (explore
  multiple candidates, pick a winner).
- [ ] Add a `cargo-generate.toml` with `project-name` placeholder.

## Milestone 2 - candidate-project Template

- [ ] Author `templates/candidate-project/` as a minimal Rust crate
  layout (Cargo.toml, src/, tests/) with a path dependency on the
  parent study's `workloads` crate.
- [ ] Use `cargo-generate`'s path variables to compute the
  relative-path dep for the study's `workloads/` and for
  `sim-models/library/`.
- [ ] Do not include `.sim-flow/` -- candidates inherit the study's
  orchestrator state.
- [ ] Include a slimmed `CLAUDE.md` / `AGENTS.md` pointing to the parent
  study for flow context.

## Milestone 3 - sim-flow new study

- [ ] Implement `sim-flow new study <name>`:
  - resolve username
  - locate `templates/study-project/`
  - invoke `cargo generate` into `users/<username>/studies/<name>/`
  - post-generation init: `experiments.db`, `state.toml` timestamp,
    `cargo build` at the workspace root
  - print the next-action hint (`sim-flow run DS0`)
- [ ] Add an integration test for the command.

## Milestone 4 - sim-flow new candidate

- [ ] Implement `sim-flow new candidate <name>`:
  - verify the current directory is a study (look for `study.md` and
    `.sim-flow/state.toml` with `flow = "design-study"`)
  - read the parent study name
  - invoke `cargo generate` for `templates/candidate-project/` into
    `candidates/<name>/`
  - update the workspace `Cargo.toml` `members` list to include the
    new candidate
  - verify `cargo build` succeeds
- [ ] Add an integration test that creates a study, adds two
  candidates, and confirms both build.

## Milestone 5 - Per-Candidate State Schema And Execution

- [ ] Extend `state.toml` support for nested candidate subtables:
  `[gates.DS5a.candidates.<name>]` entries plus an aggregate
  `[gates.DS5a]` row.
- [ ] Implement aggregate-pass semantics: the aggregate gate flips to
  `passed = true` only when every candidate subtable has
  `passed = true`.
- [ ] Implement `sim-flow run DS5a` iteration: read the candidate list
  from `analysis/screening-decision.md`, iterate sequentially, run the
  work+critique pair per candidate, record per-candidate gate status.
- [ ] Implement `sim-flow run DS5a --candidate <name>` to target a
  single candidate.
- [ ] Emit a status summary after each candidate so the user can see
  progress without tailing logs.
- [ ] Add tests covering: all-pass aggregate, one-fail aggregate,
  explicit `--candidate` targeting, and reset cascade that includes
  per-candidate subtables.

## Milestone 6 - DS9 State Transition Plumbing

- [ ] Implement the orchestrator's DS9 post-gate action: on DS9 gate
  pass, rewrite `state.toml` to set `flow = "direct-modeling"` and
  `current_step = "DM0"`, preserving the prior DS gate history under a
  `[gates.ds]` subtable.
- [ ] Implement `final-model/` validation: ensure the directory has
  been populated with the winning candidate's source tree and builds.
- [ ] Add a test that exercises the full DSF->DMF flip using a stubbed
  DS9 work session.

## Milestone 7 - Template Validation In CI

- [ ] Extend the Phase 2 CI validation to cover `study-project` and
  `candidate-project` templates.
- [ ] Add a golden-directory regression for both templates.

## Status

Not started. Schedule after Phase 4 so new study projects can initialize a
real `experiments.db` during post-generation.
