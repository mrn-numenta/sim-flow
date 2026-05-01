# 2. Direct Modeling Flow

## Purpose

Define the detailed implementation of the Direct Modeling Flow (DM0-DM5): the
session structure, orchestrator behavior, gate validation, AI client invocation, and command specifications for each step. This document is the implementation specification for the `sim-flow` orchestrator crate.

## Overview

The Direct Modeling Flow turns a design specification into a cycle-accurate
sim-foundation model through six steps (DM0-DM5). The `sim-flow` orchestrator
manages state, enforces step ordering, invokes AI sessions, and validates
outputs.

Each step runs as two **sessions**: a work session that produces the step's artifacts, and a critique session that reviews them. The two are logically independent (different prompts, distinct artifacts, independent review).

1. **Work session** -- the agent receives the step's work prompt and produces the named artifacts. When the artifacts are written it stops; it does not author the critique file and does not `/exit` on its own.
2. **Critique session** -- the agent receives the step's critique prompt. It reads the work artifacts (treating them as work produced by a third party even if it produced them itself), evaluates them against the step's gate criteria, and writes `docs/critiques/<step>-critique.md`.

### Mode × session-policy matrix

The orchestrator has two **orthogonal** axes the user (or caller) selects independently:

- **Mode** -- whether the agent is supposed to be operating without a human in the loop.
  - `automated` (`auto: true`): no human responder. The agent must auto-decide on ambiguities and document choices in `## Auto-decisions`. Inlined via `tools/sim-flow/prompts/_conventions/auto-mode.md` when this mode is active.
  - `manual` (`auto: false`): human is in the loop and may answer questions; clarifying questions are appropriate.

- **Session policy** -- how the orchestrator realizes the work/critique boundary at the process level.
  - `per-step`: each session is a fresh AI CLI subprocess; conversation state does not carry over. Independent review is enforced by process isolation.
  - `single`: one long-lived AI CLI process for the whole flow; the orchestrator injects each new prompt into the same process. Independent review is enforced by prompt structure -- the critique prompt asks the agent to bracket any prior reasoning rather than relying on it.

All four combinations are valid:

|               | per-step                                                                                          | single-session                                                                                                              |
| ------------- | ------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| **automated** | sim-flow walks DM0→DM4b spawning a fresh subprocess per session; agent never asks questions.      | sim-flow walks DM0→DM4b in one long-lived agent; conversation history accumulates; agent never asks questions.              |
| **manual**    | user `/exits` between sessions; sim-flow waits, then spawns the next subprocess.                  | dashboard buttons drive one long-lived agent (Run Step, Run Critique, Advance, etc.); conversation history accumulates.     |

The agent's per-session contract is identical in all four cells: produce/review artifacts, stop when done, don't author the wrong file, don't `/exit` on its own. Mode-specific rules live in the mode convention file (`auto-mode.md`); session-policy mechanics are an orchestrator implementation detail, not something the agent needs to special-case.

Automated callers (tests, the mock client) can opt into one-shot non-interactive invocation for the same prompt content; the interactive/one-shot distinction is captured by the `SessionMode` dimension of the client abstraction.

```text
sim-flow run <step>                         # per-step session-policy example
    |
    1. Check prerequisite gate (previous step must have passed)
    |
    2. Load work prompt and step instructions from sim-foundation
    |
    3. Start AI client work session
       - per-step:  spawn a fresh subprocess inheriting the user's TTY
       - single:    inject the work prompt into the long-lived agent
       - Agent produces the artifacts and stops
    |
    4. Load critique prompt
    |
    5. Start AI client critique session
       - per-step:  spawn a fresh subprocess with no shared state
       - single:    inject the critique prompt into the same agent
       - Critique writes docs/critiques/<step>-critique.md
    |
    6. Orchestrator runs gate validation (automatic)
       - Artifact checks (files exist, cargo builds, etc.)
       - Scans critique file for UNRESOLVED: or BLOCKER: lines
    |
    7. If gate passes: update state, done
       If gate fails: report what failed
```

## AI Client Abstraction

The orchestrator supports multiple AI clients. Each client implements the same
interface: accept a prompt, a system prompt (or appended instructions), tool
permissions, and output format. The orchestrator constructs the prompt from
step-specific markdown files and passes it to whichever client is configured.

### Client Invocation

Each client has an interactive and a one-shot invocation form. The
orchestrator uses interactive by default and falls back to one-shot only
for automated testing.

| Client | Interactive | One-shot | Tool permissions |
| ------ | ----------- | -------- | ---------------- |
| Claude Code | `claude "<prompt>"` with inherited stdio | `claude -p "<prompt>"` | `--allowedTools` (one-shot only) |
| Codex | `codex "<prompt>"` with inherited stdio | `codex exec "<prompt>"` | `-a never -s workspace-write` (one-shot) |
| Copilot | `copilot "<prompt>"` with inherited stdio | `copilot -p "<prompt>" --allow-all-tools` | `--allow-all-tools` (one-shot) |

Interactive invocations do not pass tool-permission flags because the AI
CLI applies its own project-level settings (e.g. `.claude/settings.json`)
when running as a TUI.

### Structured Output

| Client | JSON output | Parsing |
| ------ | ----------- | ------- |
| Claude Code | `--output-format json` | `jq -r '.result'` |
| Codex | `--json` | JSONL events, final message on stdout |
| Copilot | `--output-format json` | JSONL |

### Instruction Files

Step instructions are stored as markdown files in **sim-foundation** alongside the orchestrator (see [05-templates.md](05-templates.md) for the layout). The orchestrator reads these and passes them to the AI client as the system prompt or appended instructions.

For Claude Code: passed via the appropriate system-prompt CLI flag (exact flag resolved at implementation time).

For Codex and Copilot: written to `AGENTS.md` in the project directory before
the session starts, then restored after the session ends. Alternatively, the
orchestrator concatenates the instruction content into the prompt itself.

The orchestrator always builds the full prompt from instruction files and passes it to the AI client non-interactively. There are no slash commands. All three clients receive equivalent instructions.

### Configuration

The AI client is configured in `.sim-flow/config.toml`. This file is committed so the configuration is tracked with the project:

```toml
[client]
name = "claude"          # "claude", "codex", or "copilot"

[client.claude]
model = "sonnet"
max_turns = 50
allowed_tools = ["Bash", "Read", "Edit", "Write", "Glob", "Grep"]

[client.codex]
model = "o3"
sandbox = "workspace-write"
approval = "never"

[client.copilot]
model = "claude-sonnet-4.5"
mode = "autopilot"
```

### Configuration Precedence

Where the same setting is specified in multiple places, precedence is:

1. `.sim-flow/config.toml` (committed, project-owned source of truth)
2. `sim-flow` CLI flags (per-invocation overrides)
3. Environment variables (`SIM_FOUNDATION_ROOT`, etc.)

Per-step client overrides (e.g., "use Claude for DM2, Codex for DM3") are supported by adding a `[steps.<step-id>]` table that overrides `client.name` or model choice for that step.

### Session Lifecycle

Timeout, abort, and retry behavior for AI client sessions is TBD. For the initial implementation the orchestrator will invoke the client synchronously with no timeout and surface a non-zero exit from the client as a gate failure. Refinements (interrupt handling, per-step timeouts, automatic retry on transient errors) are a follow-up item.

## State Management

### State File

The orchestrator stores state in `.sim-flow/state.toml`:

```toml
flow = "direct-modeling"
current_step = "DM2a"
started = "2026-04-17T10:00:00Z"

[gates]
DM0 = { passed = true, timestamp = "2026-04-17T10:15:00Z" }
DM1 = { passed = true, timestamp = "2026-04-17T11:00:00Z" }
DM2a = { passed = false }
```

### State Transitions

Forward transitions require the current step's gate to pass. Back transitions
(re-entering a previous step) are always allowed. When a step is re-entered,
its gate status and all downstream gate statuses are reset to `passed = false`.

### CLI Interface

```text
sim-flow init --flow direct-modeling     Create .sim-flow/ and state.toml
sim-flow status                          Show current step, gate status, history
sim-flow run [step]                      Run step (default: current). Validates
                                         prerequisite, spawns session, validates gate.
sim-flow gate [step]                     Run gate validation only (no AI session)
sim-flow reset [step]                    Reset step and all downstream to not-passed
sim-flow config                          Show/edit client configuration
```

## Session Decomposition

Each DM step is split into sub-steps where natural (DM2a, DM2b, DM2c, DM2d). Every sub-step runs as a pair of sessions: one **work** session plus one **critique** session. The orchestrator treats each pair as a single gated unit.

```text
DM0    Specification                     (work + critique)
DM1    Modeling Setup                    (work + critique)
DM2a   Functional Decomposition          (work + critique)
DM2b   Pipeline Mapping                  (work + critique)
DM2c   Implementation Plan               (work + critique)
DM2d   Model Implementation              (work + critique)
DM3a   Test Plan                         (work + critique)
DM3b   Testbench Implementation          (work + critique)
DM3c   Test Execution and Coverage       (1+ work + critique pairs)
DM4    Performance Analysis              (work + critique)
DM5    External PPA Analysis             (TBD)
```

## Step Specifications

Each step specification defines:
- **Purpose**: What the step accomplishes
- **Prerequisites**: Which gate must have passed
- **Instruction files**: Which markdown files provide detailed guidance
- **Work prompt**: What the work session is told to do
- **Critique prompt**: What the critique session evaluates
- **Gate checks**: What the orchestrator validates independently

The work session produces artifacts; the critique session reads those artifacts and writes the critique file. Both sessions are logically independent (different prompts, distinct artifacts, independent review); how that separation is realized at the process level depends on the **session policy** (see the matrix above). Work prompts do not ask the agent to critique its own output -- that is the critique session's job.

### DM0: Specification

**Purpose:** Create or validate the design specification document.

**Prerequisites:** None (entry point).

**Instruction files:** `instructions/dm0-specification.md`

**Work prompt:**

```text
You are executing step DM0 (Specification) of the Direct Modeling Flow.

Read the step instructions provided. Produce or validate `docs/spec.md`
using `docs/spec.md.tmpl` as the required structure:

- if `docs/spec.md` does not exist, copy the template and fill it in
- if a user-provided source spec was ingested, treat it as authoritative
  and derive `docs/spec.md` from it
- preserve explicit requirements faithfully
- infer only secondary details that a competent modeling agent can
  reasonably fill in without changing architectural behavior
- if a missing detail would likely lead to materially different models,
  ask the user in manual mode or record an auto-decision in automated mode

The goal is a model-ready spec, not an exhaustively detailed one. It must
include enough information for later steps to decompose, pipeline, and
implement the design safely. In particular, it must contain either an
explicit gate-budget-per-cycle target or enough information to derive one,
usually via frequency plus technology target.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DM0 work
session. Read spec.md and evaluate it:
- Is the spec clear enough that a competent modeling agent can infer the
  rest reasonably without guessing at core behavior?
- Does it specify clock frequency and technology node?
- Does it contain an explicit gate budget per cycle or enough information
  for DM1 to derive a reasonable estimate?
- Are interfaces, functional behavior, dataflow, and pipeline intent clear
  enough for DM2a / DM2b?
- Are reset, state, flow-control, and exceptional behaviors specified
  well enough to avoid incorrect modeling assumptions where they matter?
- Are ambiguities, contradictions, or unresolved conflicts called out
  explicitly?

Write findings to .sim-flow/critiques/DM0-critique.md. Prefix every
unresolved issue line with `UNRESOLVED:` and every gate-blocking issue
line with `BLOCKER:`.
```

**Gate checks (orchestrator-validated):**

1. `spec.md` exists and is non-empty
2. `spec.md` contains a frequency value (regex match for MHz/GHz pattern)
3. `spec.md` contains a technology node value (regex match for nm pattern)
4. `spec.md` contains either an explicit gate-budget-per-cycle target or
   enough information for DM1 to derive one
5. `.sim-flow/critiques/DM0-critique.md` exists
6. Critique file does not contain unresolved issues (no lines starting with
   `UNRESOLVED:` or `BLOCKER:`)

---

### DM1: Modeling Setup

**Purpose:** Establish modeling targets and verification strategy.

**Prerequisites:** DM0 gate passed.

**Instruction files:** `instructions/dm1-modeling-setup.md`

**Work prompt:**

```text
You are executing step DM1 (Modeling Setup) of the Direct Modeling Flow.

Read the step instructions provided. Read spec.md and:

1. Create `targets.md` as the target-and-metrics strategy document.
   Start from `docs/targets.md.tmpl`.
   Capture explicit, derived, inferred, unconstrained, or deferred
   targets with provenance, rationale, and measurement method.
2. Include a gate-budget-per-cycle target or estimate. If the spec gives
   one explicitly, preserve it. Otherwise derive it from the frequency
   and technology target and explain the basis.
3. Create `testbench.md` as the verification-strategy and
   testbench-architecture document. Start from `docs/testbench.md.tmpl`.
   Describe what behaviors, guarantees,
   scenarios, observability, and checking strategies the later testbench
   must support, plus the likely UVM-lite structure.
4. Do not write the detailed test plan or implement tests here; that
   belongs in DM3.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DM1 work
session. Read targets.md, testbench.md, and spec.md and evaluate:
- Does every target trace back to spec.md with appropriate status /
  provenance?
- Does `targets.md` include a gate-budget-per-cycle target or estimate,
  and is its basis reasonable and clearly explained?
- Is anything important from spec.md missing from the target strategy?
- Does `testbench.md` describe a real verification strategy, not just a
  shallow component list?
- Are the proposed testbench structures appropriate for the interfaces and
  behaviors described in the spec?

Write findings to .sim-flow/critiques/DM1-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. `targets.md` exists with traceable targets, statuses, and rationale
2. `targets.md` includes a gate-budget-per-cycle target or estimate
3. `testbench.md` exists with verification-strategy and testbench-architecture content
4. `.sim-flow/critiques/DM1-critique.md` exists without blockers

---

### DM2a: Functional Decomposition

**Purpose:** Decompose the design into functional units with data movement
characterization.

**Prerequisites:** DM1 gate passed.

**Instruction files:** `instructions/dm2a-decomposition.md`

**Work prompt:**

```text
You are executing step DM2a (Functional Decomposition) of the Direct
Modeling Flow.

Read the step instructions provided. Read `spec.md` and `targets.md` and:

1. Use the gate-budget-per-cycle target or estimate from DM1 as a hard
   input to decomposition granularity.
2. Break the design into discrete operations that are meaningful for both
   architectural understanding and later stage mapping.
3. For each operation, capture purpose, inputs, outputs, dominant cost,
   statefulness / buffering / arbitration character, likely timing
   pressure, and natural architectural boundaries.
4. Write the decomposition to `analysis/decomposition.md`, starting from
   `docs/analysis/decomposition.md.tmpl`, including a short summary of
   the decomposition strategy and the gate budget used.
5. Write the data movement characterization to
   `analysis/data-movement.md`, including producer, consumer, payload
   meaning, widths, rates, burst patterns, fanout, and relevant
   ordering / flow-control / CDC notes, starting from
   `docs/analysis/data-movement.md.tmpl`.

The data movement characterization will become the payload types and port
definitions for sim-foundation models in DM2d.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DM2a work
session. Read analysis/decomposition.md, analysis/data-movement.md, and
spec.md and evaluate:
- Are all functions in spec.md represented as operations?
- Are there operations that are not in the spec (invented functionality)?
- Is the decomposition clearly grounded in the gate-budget-per-cycle
  target or estimate from DM1?
- Are data dependencies correct and complete?
- Is the data movement characterization complete and implementation-ready?
- Are important state / buffering / arbitration / CDC boundaries
  represented where they materially matter?
- Is the decomposition at the right granularity -- not too fine (one gate
  per operation) and not too coarse (entire pipeline as one operation)?

Write findings to .sim-flow/critiques/DM2a-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. `analysis/decomposition.md` exists with operation list
2. `analysis/data-movement.md` exists with data characterization
3. `.sim-flow/critiques/DM2a-critique.md` exists without blockers

---

### DM2b: Pipeline Mapping

**Purpose:** Map operations to pipeline stages at the target frequency and
technology node.

**Prerequisites:** DM2a gate passed.

**Instruction files:** `instructions/dm2b-pipeline-mapping.md`

**Work prompt:**

```text
You are executing step DM2b (Pipeline Mapping) of the Direct Modeling Flow.

Read the step instructions provided. Read spec.md and
targets.md, analysis/decomposition.md, and analysis/data-movement.md and:

1. Use the gate-budget-per-cycle target or estimate from DM1 as the
   canonical budget for this step.
2. Map operations from the decomposition to pipeline stages. Each stage
   must fit within the gate budget and form a sensible implementation
   boundary.
3. Preserve important DM2a boundaries such as buffering, arbitration,
   queueing, storage, feedback, and CDC boundaries where they materially
   matter.
4. Write the mapping to `analysis/pipeline-mapping.md`, starting from
   `docs/analysis/pipeline-mapping.md.tmpl`.
5. For each stage, record purpose, operations, gate estimate, latency
   contribution, buffering/register assumptions, and boundary rationale.
6. Verify there are no combinational loops in the pipeline.
7. The mapping must respect the pipelining and hierarchy described in the
   spec.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DM2b work
session. Read targets.md, analysis/decomposition.md,
analysis/data-movement.md, and analysis/pipeline-mapping.md and evaluate:
- Does the mapping use the canonical DM1 gate-budget-per-cycle target or
  estimate?
- Does each stage fit within that budget?
- Are there combinational loops?
- Does the mapping match the spec's prescribed pipelining and hierarchy?
- Are all operations from the decomposition mapped to a stage?
- Are important DM2a boundaries preserved where they materially matter?
- Is the stage rationale strong enough for DM2d to implement the intended
  structure without rediscovering it?

Write findings to .sim-flow/critiques/DM2b-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. `analysis/pipeline-mapping.md` exists
2. Pipeline stages map to decomposition operations (orchestrator can check
   that operation names from decomposition.md appear in pipeline-mapping.md)
3. `.sim-flow/critiques/DM2b-critique.md` exists without blockers

---

### DM2c: Implementation Plan

**Purpose:** Produce the implementation plan that DM2d will execute.

**Prerequisites:** DM2b gate passed.

**Instruction files:** `instructions/dm2c-model-impl-plan.md`

**Work prompt:**

```text
You are executing step DM2c (Implementation Plan) of the Direct Modeling
Flow.

Read the step instructions provided. Read spec.md, targets.md,
testbench.md, analysis/decomposition.md, analysis/pipeline-mapping.md,
and analysis/data-movement.md and:

1. Create an implementation plan under `docs/plan/` that breaks the work
   into ordered milestones and concrete tasks.
2. Trace every decomposition operation and every payload to at least one
   task.
3. Cover payload types, module skeletons + connectivity, per-stage logic,
   and smoke / unit tests.
4. Use `targets.md` and `testbench.md` where they materially affect the
   implementation structure, but do not turn this into a full DM3
   verification-plan step.
5. Do not write code in this step.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DM2c work
session. Read the plan files plus spec.md, targets.md, testbench.md,
decomposition.md, pipeline-mapping.md, and data-movement.md and
evaluate:
- Does the plan follow the plan-management conventions?
- Are tasks concrete and traceable to operations, payloads, and required
  implementation artifacts?
- Does the plan cover the required smoke tests without pre-empting DM3?
- Does it account for target- and verification-sensitive implementation
  concerns where they materially affect DM2d?
- Are open implementation decisions called out explicitly rather than
  silently deferred?

Write findings to .sim-flow/critiques/DM2c-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. `docs/plan/plan.md` exists
2. `docs/plan/milestone-*.md` exists
3. Plan tasks trace to the decomposition / data-movement artifacts
4. `.sim-flow/critiques/DM2c-critique.md` exists without blockers

---

### DM2d: Model Implementation

**Purpose:** Build the sim-foundation model from the implementation plan.

**Prerequisites:** DM2c gate passed.

**Instruction files:** `instructions/dm2d-model-implementation.md`

**Work prompt:**

```text
You are executing step DM2d (Model Implementation) of the Direct
Modeling Flow.

Read the step instructions provided. Read the implementation plan in
docs/plan/, plus spec.md, targets.md, testbench.md, decomposition.md,
pipeline-mapping.md, and data-movement.md.

1. Execute the plan milestone by milestone.
2. If framework API guidance is needed, start with `fw:api/toc.md`,
   then read only the specific `fw:api/pages/...` files you need.
   Drop into `fw:src/...` only when exact signatures or source-level
   examples are needed.
3. Use `targets.md` and `testbench.md` where they materially affect
   implementation structure, but do not turn this into a full DM3
   verification step.
4. Create payload types, connectivity, modules, smoke tests, and minimal
   unit tests.
5. Verify with cargo build and cargo test as you go.

Stop after each completed milestone for a paired critique before moving
to the next milestone. After the final milestone, stop for a final DM2d
critique that checks end-to-end integration/regression rather than
acting as the first serious review.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DM2d work
session. Read the model source, tests, plan files, targets.md,
testbench.md, and analysis docs and
evaluate:
- Does the implementation match the pipeline mapping and decomposition?
- Are payloads consistent with data-movement.md?
- Are framework conventions respected?
- Do smoke tests pass, and are they meaningful?
- Does the implementation preserve target-sensitive and plan-sensitive
  structural decisions rather than drifting away from them?
- Does the implementation stay within the DM2d scope defined by the plan?
- Is the just-completed milestone sound enough to support the next one,
  or, on the final critique, do the milestone-local decisions compose
  cleanly end-to-end?

Write findings to docs/critiques/DM2d-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. `src/model/` directory exists with module source files
2. `cargo build` succeeds
3. `cargo test` passes (at minimum elaboration test)
4. Source contains a ConnectivityPlan
5. `docs/critiques/DM2d-critique.md` exists without blockers

**Re-entry / cadence:** DM2d is intentionally iterative. After each
milestone-complete checkpoint, the orchestrator should run the critique
before allowing the next milestone to begin. After the final milestone,
run one last DM2d critique as the end-to-end integration/regression
gate.

---

### DM3a: Test Plan

**Purpose:** Produce the verification plan that DM3b and DM3c will
execute.

**Prerequisites:** DM2d gate passed.

**Instruction files:** `instructions/dm3a-test-plan.md`

**Work prompt:**

```text
You are executing step DM3a (Test Plan) of the Direct Modeling Flow.

Read the step instructions provided. Start from
`docs/plan/test-plan.md.tmpl`, then produce `docs/plan/test-plan.md`
covering:

1. Testbench architecture (Sequencers, Drivers, Monitors, Scoreboards,
   and `SimEnvBuilder` wiring).
2. If framework API guidance is needed, start with `fw:api/toc.md`,
   then read only the specific `fw:api/pages/...` files you need.
   Use `fw:src/...` only as a secondary source for exact signatures.
3. Test enumeration across Smoke, Edge, Stress, and Random categories.
4. Coverage strategy using `cargo-tarpaulin`.
5. Traceability from tests back to spec requirements, targets, and
   decomposition operations.

Do not write test code in this step.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DM3a work
session. Read `docs/plan/test-plan.md` plus the spec, targets, and
analysis docs and evaluate:
- Is the testbench architecture concrete enough for DM3b?
- Are all four test categories present with concrete pass criteria?
- Is the coverage strategy explicit and measurable?
- Is traceability complete and specific?
- Does the plan stay out of test-code territory?
```

**Gate checks:**

1. `docs/plan/test-plan.md` exists
2. The plan contains `## Testbench`, `## Smoke`, `## Edge`,
   `## Stress`, `## Random`, `## Coverage`, and `## Traceability`
   sections
3. `docs/critiques/DM3a-critique.md` exists without blockers

---

### DM3b: Testbench Implementation

**Purpose:** Implement the UVM-lite testbench scaffolding from DM3a's
test plan.

**Prerequisites:** DM3a gate passed.

**Instruction files:** `instructions/dm3b-testbench-impl.md`

**Work prompt:**

```text
You are executing step DM3b (Testbench Implementation) of the Direct Modeling
Flow.

Read the step instructions provided. Read `docs/plan/test-plan.md`.

1. Implement the named testbench components and `SimEnvBuilder` wiring.
2. If framework API guidance is needed, start with `fw:api/toc.md`,
   then read only the specific `fw:api/pages/...` files you need.
   Use `fw:src/...` only as a secondary source for exact signatures.
3. Add only the basic data-flow smoke test.
4. Confirm the scaffolding builds and the smoke test passes.

Do not implement the full test suite in this step.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DM3b work
session. Read the testbench source plus `docs/plan/test-plan.md`,
consult `fw:api/toc.md` / specific `fw:api/pages/...` files as needed,
and
evaluate:
- Are all planned testbench components implemented?
- Is the UVM-lite topology intact?
- Is the `SimEnvBuilder` wiring complete?
- Is the basic smoke test meaningful and passing?
- Does the testbench stay within the public framework API surface?
- Do payload and port usages match the model and analysis docs?
- Did DM3b stay out of DM3c territory?

Write findings to docs/critiques/DM3b-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. Testbench source files exist
2. `cargo build` succeeds
3. The basic smoke test exists and passes
4. `docs/critiques/DM3b-critique.md` exists without blockers

---

### DM3c: Test Execution and Coverage

**Purpose:** Write tests from the test plan, execute them, and achieve
coverage targets.

**Prerequisites:** DM3b gate passed.

**Instruction files:** `instructions/dm3c-test-execution.md`

**Work prompt:**

```text
You are executing step DM3c (Test Execution and Coverage) of the Direct
Modeling Flow.

Read the step instructions provided. Read docs/plan/test-plan.md.

1. Implement planned tests category by category: Smoke, then Edge, then
   Stress, then Random.
2. Use the DM3b testbench scaffolding; if the scaffolding is wrong, flag
   it rather than silently redesigning it here.
3. Run tests, fix design or test bugs as needed, and mark completed rows
   in docs/plan/test-plan.md.
4. Measure coverage with the plan's `cargo-tarpaulin` strategy and meet
   the declared threshold.
5. If coverage is below threshold, add tests or document concrete
   exclusions in the plan.

Stop after each completed category for a paired critique before moving
to the next category. After the final category, full-suite rerun, and
coverage pass, stop for a final DM3c critique that checks end-to-end
regression and coverage closure rather than acting as the first serious
review.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DM3c work
session. Read the test sources, coverage report, and
docs/plan/test-plan.md
and evaluate:
- Are all plan rows resolved or explicitly deferred with a concrete
  `defer reason:`?
- Are all four categories represented in the implemented suite?
- Do tests pass end-to-end, and are random tests reproducible?
- Is coverage at or above the plan's threshold?
- Are uncovered lines justified with concrete exclusions?
- Did any design bugs found during testing get properly fixed and
  re-verified?
- Were the DM3b testbench helpers preserved rather than modified here?
- Is the just-completed category sound enough to support the next one,
  or, on the final critique, do the category-local additions compose
  cleanly end-to-end?

Write findings to docs/critiques/DM3c-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. `cargo test` passes (all tests)
2. Coverage measurement exists (coverage report file)
3. Coverage meets the plan's declared threshold, with any uncovered lines
   either tested or documented in `## Coverage > Exclusions`
4. `docs/plan/test-plan.md` has completed or explicitly deferred
   checklist entries
5. `docs/critiques/DM3c-critique.md` exists without blockers

**Re-entry:** If the test plan is large, DM3c may need multiple sessions.
The orchestrator should run a critique after each completed category
before allowing the next category to begin. Each re-entry session picks
up where the previous left off by reading the test plan checklist. After
the final category, run one last DM3c critique as the end-to-end
coverage/regression gate.

---

### DM4: Performance Analysis

**Purpose:** Analyze design performance and identify bottlenecks.

**Prerequisites:** DM3c gate passed.

**Instruction files:** `instructions/dm4-performance-analysis.md`

**Work prompt:**

```text
You are executing step DM4 (Performance Analysis) of the Direct Modeling
Flow.

Read the step instructions provided. Read targets.md for the performance
targets.

1. Run simulations with appropriate workloads to collect performance data.
2. Analyze results: throughput, latency, memory and communication
   bandwidth, utilization per module, pipeline bubbles, and bottlenecks.
3. If the design is parameterizable, perform parameter sweeps.
4. Report results using Foundation standard reporting mechanisms: obsv
   artifacts, charts, and report templates.
5. Write the analysis report to docs/analysis/.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DM4 work
session. Read the analysis report in docs/analysis/ and evaluate:
- Are all target metrics from targets.md measured?
- Are bottlenecks identified with supporting evidence?
- Do parameter sweep results make physical sense?
- Are any results inconsistent with the spec or the model's intended
  behavior?
- Does the report use distributions (percentiles) rather than just
  scalar summaries?

Write findings to .sim-flow/critiques/DM4-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. At least one experiment run recorded in tracking
2. Analysis report exists in `docs/analysis/`
3. Report contains throughput and latency metrics
4. `.sim-flow/critiques/DM4-critique.md` exists without blockers

---

### DM5: External PPA Analysis

**Purpose:** Generate RTL and support external synthesis for PPA.

**Prerequisites:** DM4 gate passed.

**Instruction files:** `instructions/dm5-ppa-analysis.md`

**Session structure:** TBD. Will be defined in collaboration with the PPA
flow engineer. Expected sub-sessions:

- DM5a: Level 1 analytical PPA estimation
- DM5b: LLM-assisted SystemVerilog generation
- DM5c: External synthesis flow guidance and result import

---

## Instruction File Location

Step instruction files live in **sim-foundation**, alongside the orchestrator crate and the project templates:

```text
sim-foundation/
    tools/sim-flow/
    instructions/
        dm0-specification.md
        dm1-modeling-setup.md
        dm2a-decomposition.md
        dm2b-pipeline-mapping.md
        dm2c-model-impl-plan.md
        dm2d-model-implementation.md
        dm3a-test-plan.md
        dm3b-testbench-impl.md
        dm3c-test-execution.md
        dm3-critique.md         # shared critique-session preamble, if any
        dm4-performance-analysis.md
        dm5-ppa-analysis.md
```

These files contain the detailed guidance for the AI -- more explicit and
procedural than the architecture documents. They include:

- Step-by-step instructions for what to do
- Foundation framework patterns and APIs to use
- File naming conventions and locations
- Examples of expected output format
- Common pitfalls and how to avoid them

The orchestrator reads these files and passes them to the AI client as part
of the system prompt or appended instructions. sim-models never edits these files -- they are owned by sim-foundation and are the framework's source of truth for step guidance. See [05-templates.md](05-templates.md) for the sim-foundation layout.

## Critique and Gate Interaction

Every step runs a critique as a separate AI session after the work session exits. The critique serves two purposes:

1. **Independent review:** A fresh session with no context from the work session reduces self-bias. It reads the artifacts the work session produced and judges them against the step's critique prompt.

2. **Input to gate validation:** The critique is written to `.sim-flow/critiques/<step>-critique.md`. The orchestrator scans this file after the critique session exits. If any line starts with `UNRESOLVED:` or `BLOCKER:`, the gate fails.

### Critique File Format

The critique file is free-form markdown. The only gate contract is line-prefixed issue markers:

```markdown
# DM2c Critique

## Consistency
- ConnectivityPlan matches pipeline-mapping.md: YES
- Payload types match data-movement.md: YES

## Completeness
- All pipeline stages implemented: YES
- All operations from decomposition covered: YES

## Findings
- RESOLVED: FetchModule needed settle() for redirect feedback
- RESOLVED: Missing backpressure test for output port
- UNRESOLVED: Pipeline bubble rate higher than estimated in DM2b
- BLOCKER: Scoreboard does not verify output ordering
```

The orchestrator grep rule is strict: any line whose first non-whitespace token is `UNRESOLVED:` or `BLOCKER:` fails the gate. `RESOLVED:` lines are ignored. The content beyond the prefix is for humans; the orchestrator does not parse it.

## Run Identification

The orchestrator passes each simulation invocation a `--run-id <id>` CLI flag. Every model binary generated from the project templates (see [05-templates.md](05-templates.md)) must accept this flag and propagate it into Foundation's `RunManifest::new(run_id)` and `ObservabilityRunWriter::new(output_dir, run_id)`. This is how experiment tracking correlates a simulation's `.obsv` artifacts with its index row.

## Orchestrator Crate

The `sim-flow` orchestrator lives as a crate in the sim-foundation workspace:

```text
sim-foundation/
    crates/
        sim-flow/
            Cargo.toml
            src/
                main.rs          # CLI entry point
                state.rs         # State machine and persistence
                gate.rs          # Gate validation logic
                client.rs        # AI client abstraction
                clients/
                    claude.rs    # Claude Code invocation
                    codex.rs     # Codex CLI invocation
                    copilot.rs   # Copilot CLI invocation
                steps/
                    mod.rs       # Step registry
                    dm.rs        # DM0-DM5 step definitions
                    ds.rs        # DS0-DS9 step definitions
                critique.rs      # Critique file parsing
    instructions/                # Step prompts (dm0..., ds0..., critique prompts)
    templates/                   # cargo-generate templates (see 05-templates.md)
```

### Dependencies

- `clap` for CLI argument parsing
- `toml` for state and config file parsing
- `serde` for serialization
- `std::process::Command` for AI client subprocess invocation
- `regex` for gate validation checks (frequency, node patterns)
- `rusqlite` for the experiments index (see [04-experiment-tracking.md](04-experiment-tracking.md))
- No async runtime needed -- sessions are sequential
