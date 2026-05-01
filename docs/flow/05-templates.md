# 5. Project Templates

## Purpose

Define the cargo-generate templates that set up ai-flow projects for both the
Direct Modeling Flow and the Design Study Flow. Templates deliver the directory
structure, AI client configuration files, orchestrator state, Foundation model
skeleton, and build infrastructure so the user can start working immediately.

## User Workflow

Users work in the sim-models repository under their `users/<username>/`
directory. They may have multiple model projects and studies. A sim-foundation
CLI command wraps `cargo generate` and creates the project in the right
location.

Each study or standalone model is created once with a single `cargo generate`. All candidates, shared workloads, comparison artifacts, and the eventual `final-model/` produced by DS9 live inside that one project. There is no second `cargo generate` when DS9 transitions into the DMF.

```text
# Create a Direct Modeling Flow project (user already knows the design)
sim-flow new model <project-name>
  -> creates users/<username>/models/<project-name>/

# Create a Design Study Flow project (user wants to explore)
sim-flow new study <study-name>
  -> creates users/<username>/studies/<study-name>/
  -> includes candidates/, workloads/, comparisons/, final-model/

# Add a candidate to an existing study (scaffolding inside the study)
sim-flow new candidate <candidate-name>
  -> creates candidates/<candidate-name>/ within the current study
```

`sim-flow new candidate` is the only subcommand that creates sub-scaffolding inside an existing project. DS9 does not call `sim-flow new model` -- it populates `final-model/` within the existing study project and flips `.sim-flow/state.toml` from `design-study` to `direct-modeling`.

The `<username>` is determined from the git user config or a `.sim-flow/user`
file. If neither exists, the command prompts for it.

### Prerequisites

- `cargo-generate` must be installed (`cargo install cargo-generate`)
- The sim-models repo must be checked out (sim-foundation is pulled as a
  dependency via Cargo.toml)

## Template Types

Three templates serve the three project types:

| Template | Flow | Created By | Purpose |
| -------- | ---- | ---------- | ------- |
| `model-project` | Direct Modeling | `sim-flow new model` | Single model with full DM flow support |
| `study-project` | Design Study | `sim-flow new study` | Study container with workloads, comparisons |
| `candidate-project` | Design Study | `sim-flow new candidate` | Candidate model within a study |

Templates and step instruction files live in the sim-foundation repository alongside the orchestrator. sim-foundation owns all orchestration, schemas, templates, and AI instructions. sim-models owns model code that consumes them -- it never edits framework-owned files.

```text
sim-foundation/
    tools/sim-flow/        # orchestrator binary
    templates/              # cargo-generate templates
        model-project/
        study-project/
        candidate-project/
    instructions/           # step work/critique prompts (dm0..., ds0..., ...)
```

The orchestrator binary resolves its sibling `templates/` and `instructions/` directories at runtime (see Template Location and Ownership below).

## sim-models Library

The sim-models repository maintains a shared library of reusable model
components at `sim-models/library/`. Each subdirectory is an independent
Rust crate that depends on foundation-framework and can be imported by
any user project via path dependencies.

```text
sim-models/library/
    soc/                    # SoC-level composition and topology
    memory-controller/      # DDR/LPDDR memory controller model
    micron-lpddr5x/         # Micron LPDDR5X device model
    ...                     # Additional library modules over time
```

Library crates are workspace members of the sim-models Cargo workspace.
They use path dependencies between each other (e.g., `soc` depends on
`memory-controller` and `micron-lpddr5x`).

### How Generated Projects Use the Library

Generated projects reference library crates via relative path dependencies
in their Cargo.toml. The `sim-flow new` command computes the correct
relative path based on where the project is created:

| Project Location | Relative Path to Library |
| ---------------- | ------------------------ |
| `users/<u>/models/<m>/` | `../../library` |
| `users/<u>/studies/<s>/candidates/<c>/` | `../../../../library` |

The AI can also add library dependencies during model implementation (DM2c,
DS5a) when it identifies that an existing library component matches the
design's needs.

### Adding to the Library

New library modules are added by creating a crate under `sim-models/library/`
and adding it to the workspace `Cargo.toml` members list. Library modules
should be general-purpose enough to be useful across multiple projects.

## Template: model-project

Created by `sim-flow new model <name>`. This is the template for the Direct
Modeling Flow (DM0-DM5).

### Generated Directory Structure

```text
users/<username>/models/<project-name>/
    Cargo.toml
    src/
        lib.rs
        main.rs
        sim.rs
        model/
            mod.rs
            top.rs
    tests/
        elaboration.rs

    # AI client configuration (multi-client)
    CLAUDE.md                       # Claude Code instructions
    AGENTS.md                       # Codex / Copilot instructions
    .claude/
        settings.json
        scratchpad/
            .gitkeep
    .codex/                         # Codex-specific config (if needed)
    .github/
        copilot-instructions.md     # Copilot-specific instructions

    # Orchestrator state
    .sim-flow/
        state.toml                  # Flow state (current step, gates)
        config.toml                 # AI client selection and settings
        experiments.db              # Run index (initialized empty)
        critiques/                  # Critique output from each step
            .gitkeep

    # Project documentation
    docs/
        analysis/
            .gitkeep

    # Experiment artifacts
    .experiments/
        .gitkeep

    .gitignore
```

### Key Files

**Cargo.toml:**

```toml
[package]
name = "{{project-name | snake_case}}"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[dependencies]
foundation-framework = { git = "ssh://git@github.com/NumentaCorp/sim-foundation.git" }

# sim-models library modules (uncomment as needed)
# soc = { path = "{{library_path}}/soc" }
# memory-controller = { path = "{{library_path}}/memory-controller" }
# micron-lpddr5x = { path = "{{library_path}}/micron-lpddr5x" }
```

The `{{library_path}}` placeholder resolves to the relative path from the
generated project to `sim-models/library/`. For a model at
`users/<username>/models/<name>/`, this is `../../library`. For a candidate
at `users/<username>/studies/<study>/candidates/<name>/`, this is
`../../../../library`.

The generated Cargo.toml includes commented-out path dependencies for all
library crates. The user or AI uncomments the ones needed. The `sim-flow new`
command computes the correct relative path at generation time based on the
project's depth within the sim-models directory tree.

**CLAUDE.md** (Claude Code instructions):

```markdown
# {{project-name}}

## What This Is

A sim-foundation model project using the Direct Modeling Flow.
Models are cycle-accurate hardware simulations built on the
foundation-framework crate.

## Project Structure

- `src/model/` - Module definitions, topology, hierarchy
- `src/sim.rs` - Simulation harness, runtime wiring
- `src/main.rs` - CLI entrypoint
- `tests/` - Self-checking verification tests
- `docs/analysis/` - Performance analysis reports

## Key Patterns

- Modules implement `Module` with `HasLogic` and `HasInstances`
- Payload types flow through typed ports (input/output)
- Topology is declared via ConnectivityPlan
- Phase model: evaluate -> settle -> update
- Tests use UVM-lite: Sequencer, Driver, Monitor, Scoreboard, SimEnv

## Flow State

This project is managed by the sim-flow orchestrator.
Check `.sim-flow/state.toml` for the current step.

## Build Commands

- cargo build
- cargo test
- cargo run
- cargo run -- --dump-hierarchy
```

**AGENTS.md** (Codex and Copilot instructions):

```markdown
# {{project-name}}

This is a sim-foundation model project using the Direct Modeling Flow.

## Rules

- Modules implement `Module` with `HasLogic` and `HasInstances` traits
- Topology is declared via ConnectivityPlan
- Use UVM-lite (Sequencer, Driver, Monitor, Scoreboard, SimEnv) for testing
- Do not create custom scheduling or bypass the Foundation port system
- All modules have flopped inputs (framework invariant)
- Factor complex logic into helper functions called from evaluate()

## Project Structure

- `src/model/` - Module definitions, topology, hierarchy
- `src/sim.rs` - Simulation harness
- `tests/` - Verification tests using UVM-lite
- `.sim-flow/state.toml` - Current flow step

## Build

- cargo build
- cargo test
```

**.sim-flow/state.toml** (initial state):

```toml
flow = "direct-modeling"
current_step = "DM0"
started = "{{timestamp}}"

[gates]
```

**.sim-flow/config.toml** (client configuration):

```toml
[client]
name = "claude"          # "claude", "codex", or "copilot"

[client.claude]
model = "sonnet"
allowed_tools = ["Bash", "Read", "Edit", "Write", "Glob", "Grep"]

[client.codex]
sandbox = "workspace-write"
approval = "never"

[client.copilot]
mode = "autopilot"
```

**.gitignore:**

```gitignore
# Build artifacts
/target/

# AI scratchpad (session-ephemeral)
.claude/scratchpad/*
!.claude/scratchpad/.gitkeep

# Heavy binary observability artifacts (reproducible from code + config)
.experiments/*/*.obsv

# SQLite transient files
*.db-wal
*.db-shm

# Orchestrator critiques are committed (institutional memory)
# Orchestrator state is committed (tracks progress)
```

### cargo-generate.toml

```toml
[template]
cargo_generate_version = ">=0.20.0"

[placeholders.top_module_name]
type = "string"
prompt = "Top module name"
default = "generated_top"
```

## Template: study-project

Created by `sim-flow new study <name>`. This is the container for the Design
Study Flow (DS0-DS9).

### Generated Directory Structure

```text
users/<username>/studies/<study-name>/
    # Workspace Cargo.toml spans candidates/, workloads/, and final-model/
    Cargo.toml                      # [workspace] only, no package

    # AI client configuration (multi-client)
    CLAUDE.md
    AGENTS.md
    .claude/
        settings.json
        scratchpad/
            .gitkeep
    .github/
        copilot-instructions.md

    # Orchestrator state
    .sim-flow/
        state.toml                  # Flow state (DS0-DS9, then DM after DS9)
        config.toml                 # AI client selection
        experiments.db              # Run index
        critiques/
            .gitkeep

    # Study artifacts
    study.md                        # Problem definition (placeholder)
    spec.md                         # Specification (placeholder)
    targets.md                      # Verification targets (placeholder)
    testbench.md                    # Testbench requirements (placeholder)
    workloads/                      # Shared UVM-lite testbench crate
        Cargo.toml
        src/
            lib.rs
    candidates/                     # Candidate models go here (sim-flow new candidate)
        .gitkeep
    comparisons/                    # Cross-candidate analysis
        .gitkeep
    analysis/                       # Decomposition, pipeline mapping, screening
        .gitkeep
    final-model/                    # Populated by DS9 from the winning candidate
        .gitkeep

    .gitignore
```

### Key Differences from model-project

- Root is a Cargo workspace; per-candidate crates, the shared workloads crate, and `final-model/` are workspace members
- No top-level `src/` -- code lives in candidates and (eventually) `final-model/`
- Includes `workloads/`, `candidates/`, `comparisons/`, `analysis/`, `final-model/`
- Placeholder files for `study.md`, `spec.md`, `targets.md`, `testbench.md`
  that DS0 and DS1 will fill in
- `.sim-flow/state.toml` starts at DS0 with `flow = "design-study"`; DS9 flips it to `direct-modeling` in place

**CLAUDE.md:**

```markdown
# {{project-name}} Design Study

## What This Is

A design study comparing candidate architectures for {{project-name}}.
Built on the sim-foundation framework within the sim-models repository.

## Study Structure

- `study.md` - Problem definition, constraints, success criteria
- `spec.md` - Hardware requirements specification
- `targets.md` - Verification targets
- `testbench.md` - Testbench requirements
- `workloads/` - Shared stimulus definitions
- `candidates/` - Individual candidate models
- `comparisons/` - Cross-candidate analysis and decisions
- `analysis/` - Decomposition, pipeline mapping, screening

## Flow State

This study is managed by the sim-flow orchestrator.
Check `.sim-flow/state.toml` for the current step.
```

**.sim-flow/state.toml:**

```toml
flow = "design-study"
current_step = "DS0"
started = "{{timestamp}}"

[gates]
```

## Template: candidate-project

Created by `sim-flow new candidate <name>` within a study directory.
This is a lightweight model project for rapid prototyping during DS5.

### Generated Directory Structure

```text
candidates/<candidate-name>/
    Cargo.toml
    src/
        lib.rs
        main.rs
        sim.rs
        model/
            mod.rs
            top.rs
    tests/
        elaboration.rs

    # AI client configuration
    CLAUDE.md
    AGENTS.md

    # Per-candidate artifacts
    analysis/
        .gitkeep
    .experiments/
        .gitkeep

    .gitignore
```

### Key Differences from model-project

- No `.sim-flow/` directory -- the study's orchestrator manages candidates
- No `.claude/scratchpad/` or `.claude/settings.json` -- inherits from study
- Minimal AI configuration -- just enough to identify this as a candidate
- `analysis/` for per-candidate workload results and bottleneck reports

**Cargo.toml:**

```toml
[package]
name = "{{project-name | snake_case}}"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[dependencies]
foundation-framework = { git = "ssh://git@github.com/NumentaCorp/sim-foundation.git" }

# sim-models library modules (uncomment as needed)
# soc = { path = "{{library_path}}/soc" }
# memory-controller = { path = "{{library_path}}/memory-controller" }
# micron-lpddr5x = { path = "{{library_path}}/micron-lpddr5x" }
```

For candidates at `users/<u>/studies/<s>/candidates/<c>/`, the
`{{library_path}}` resolves to `../../../../library`.

**CLAUDE.md:**

```markdown
# {{project-name}}

## What This Is

A candidate model in the {{study-name}} design study. Built on the
sim-foundation framework for rapid architecture exploration.

## Project Structure

- `src/model/` - Module definitions, topology, hierarchy
- `src/sim.rs` - Simulation harness
- `tests/` - Verification tests
- `analysis/` - Per-candidate analysis results

## Key Patterns

- Modules implement `Module` with `HasLogic` and `HasInstances`
- Topology declared via ConnectivityPlan
- Phase model: evaluate -> settle -> update
- Use UVM-lite for testing

## Build Commands

- cargo build
- cargo test
- cargo run
```

### cargo-generate.toml

```toml
[template]
cargo_generate_version = ">=0.20.0"

[placeholders.top_module_name]
type = "string"
prompt = "Top module name"
default = "generated_top"

[placeholders.study-name]
type = "string"
prompt = "Parent study name"
```

## sim-flow new Command

The `sim-flow new` command wraps `cargo generate` with the correct template
path and destination:

```text
sim-flow new model <name>
    1. Determine username (git config user.name or .sim-flow/user)
    2. Locate sim-foundation/templates/model-project
    3. Run: cargo generate --path <foundation>/templates/model-project \
           --name <name> \
           --destination users/<username>/models
    4. Initialize .sim-flow/experiments.db (empty schema)
    5. Report: "Model project created at users/<username>/models/<name>/"
              "Run: sim-flow run DM0 to start"

sim-flow new study <name>
    1. Determine username
    2. Locate sim-foundation/templates/study-project
    3. Run: cargo generate --path <foundation>/templates/study-project \
           --name <name> \
           --destination users/<username>/studies
    4. Initialize .sim-flow/experiments.db
    5. Report: "Study created at users/<username>/studies/<name>/"
              "Run: sim-flow run DS0 to start"

sim-flow new candidate <name>
    1. Verify we are in a study directory (look for study.md)
    2. Get study name from study directory
    3. Run: cargo generate --path <foundation>/templates/candidate-project \
           --name <name> \
           -d study-name=<study-name> \
           --destination ./candidates
    4. Verify cargo build in the candidate directory
    5. Report: "Candidate created at candidates/<name>/"
```

## Multi-Client AI Configuration

Each template generates configuration for all three supported AI clients.
The user selects which client to use in `.sim-flow/config.toml`. The
instruction content is equivalent across all three:

| File | Client | How It's Used |
| ---- | ------ | ------------- |
| `CLAUDE.md` | Claude Code | Loaded automatically at session start |
| `AGENTS.md` | Codex, Copilot | Loaded automatically at session start |
| `.claude/settings.json` | Claude Code | Tool permissions, model selection |
| `.github/copilot-instructions.md` | Copilot | Additional path-specific instructions |

The orchestrator also passes step-specific instructions from
`docs/instructions/` as the system prompt (see doc 02), but the project-level
files provide baseline context that is always available regardless of how the
AI is invoked.

### Keeping Instructions in Sync

`CLAUDE.md` and `AGENTS.md` must contain equivalent instructions. They differ
only in format, not in content. When templates are updated, both files must
be updated together.

The orchestrator does not depend on either file -- it constructs its own
prompts from instruction files. These project-level files exist for when the
user interacts with the AI directly (outside the orchestrator).

## Post-Generation Initialization

After `cargo generate` runs, the `sim-flow new` command:

1. Initializes `.sim-flow/experiments.db` with the schema from doc 04
2. Sets the timestamp in `.sim-flow/state.toml`
3. Verifies `cargo build` succeeds for templates that include Rust code
   (model-project and candidate-project)
4. Prints the next step to run

## Template Location and Ownership

Templates live in **sim-foundation** alongside the orchestrator. This keeps
the framework/model separation clean -- sim-foundation owns the flow, the
orchestrator, and the templates that set up projects. sim-models is purely
for user model code and studies.

```text
sim-foundation/
    tools/sim-flow/               # orchestrator binary
    templates/                     # cargo-generate templates
        model-project/
        study-project/
        candidate-project/
```

The `sim-flow` command locates templates relative to the sim-foundation
repository root:

1. Walking up from the `sim-flow` binary's location to find the sim-foundation
   workspace `Cargo.toml`
2. Or reading `SIM_FOUNDATION_ROOT` environment variable
3. Or using the `--foundation-root` flag

## DS9 Transition (In-Place)

When a study selects a winner in DS9, there is no new project. The DS9 work session populates `final-model/` within the existing study project (see [03-design-study-flow.md](03-design-study-flow.md)). After the DS9 gate passes, the orchestrator flips `.sim-flow/state.toml` from `design-study` to `direct-modeling` and sets `current_step = "DM0"`. The next run (`sim-flow run DM0`) executes against `final-model/` using the now-detailed spec.md.

The study's gate history, critiques, experiments DB, workloads crate, and candidate directories are preserved -- nothing is copied, moved, or regenerated.
