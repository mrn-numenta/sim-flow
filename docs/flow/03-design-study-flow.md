# 3. Design Study Flow

## Purpose

Define the detailed implementation of the Design Study Flow (DS0-DS9): the
session structure, orchestrator behavior, gate validation, AI client invocation,
and command specifications for each step. This document is the implementation
specification for the `sim-flow` orchestrator's design study mode.

## Overview

The Design Study Flow selects a winning candidate from multiple architecture
alternatives through structured exploration. The flow shares the same
orchestrator infrastructure as the Direct Modeling Flow (doc 02): state
management, AI client abstraction, work+critique session pairs, gate validation,
and critique-file format.

The key differences from the Direct Modeling Flow:

- Multiple candidates are created, prototyped, and compared
- The specification is intentionally open (requirements, not a design)
- Analytical screening eliminates infeasible candidates before simulation
- Comparison and decision phases select a winner
- The winner transitions into the Direct Modeling Flow in the same project for production-quality modeling

The entire study, from DS0 through the eventual DM4/DM5 work, lives in a single `cargo generate`d project. DS9 does not create a new project -- it flips the orchestrator state and promotes the winning candidate into a `final-model/` slot in the existing study directory.

## Session Decomposition

Every step runs as a work + critique session pair, matching the DMF pattern (see [02-direct-modeling-flow.md](02-direct-modeling-flow.md)).

```text
DS0    Specification                     (work + critique)
DS1    Study Setup                       (work + critique)
DS2    Functional Decomposition          (work + critique)
DS3    Pipeline Mapping                  (work + critique, produces N candidates)
DS4    Analytical Screening              (work + critique)
DS5a   Candidate Prototyping             (work + critique per candidate)
DS5b   Candidate Smoke Validation        (work + critique per candidate)
DS6    Comparison and Narrowing          (work + critique)
DS7    Deep Analysis                     (1+ work + critique pairs)
DS8    Decision                          (work + critique)
DS9    Formalize                         (work + critique; in-place transition to DMF)
```

## Step Specifications

### DS0: Specification

**Purpose:** Create or validate a requirements specification for the study.
Unlike the Direct Modeling Flow spec, this spec describes *what* the hardware
must do without prescribing *how*. It leaves the architecture open for
exploration.

**Prerequisites:** None (entry point).

**Instruction files:** `instructions/ds0-specification.md`

**Work prompt:**

```text
You are executing step DS0 (Specification) of the Design Study Flow.

Read the step instructions provided. The user needs to provide or create
a specification document (spec.md) for the design study.

This is a requirements document, not a design document. Guide the user
to include:
- Clock frequency and technology node
- Functional description (what the hardware does, not how)
- Quantitative constraints (throughput, latency, area, power)
- Operating environment (surrounding blocks, interfaces, traffic patterns)
- Open questions and unknowns (expected at this stage)

The specification must NOT prescribe an architecture (pipeline depth,
arbiter type, etc.) -- that is what the study will determine.

If spec.md already exists, review it against these requirements. Flag
any content that prematurely closes the design space.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DS0 work
session. Read spec.md and evaluate:
- Does it specify clock frequency and technology node?
- Is the functional description clear enough to decompose into operations?
- Are quantitative constraints specified (throughput, latency, area, power)?
- Is the operating environment described?
- Does the spec leave the architecture open? Flag any premature design
  decisions.
- Are unknowns explicitly called out?
- Is anything missing that would block DS1?

Write findings to .sim-flow/critiques/DS0-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. `spec.md` exists and is non-empty
2. Contains frequency value (regex match)
3. Contains technology node value (regex match)
4. `.sim-flow/critiques/DS0-critique.md` exists without blockers

---

### DS1: Study Setup

**Purpose:** Establish the problem definition, verification targets, workload
suite, and testbench requirements for the study.

**Prerequisites:** DS0 gate passed.

**Instruction files:** `instructions/ds1-study-setup.md`

**Work prompt:**

```text
You are executing step DS1 (Study Setup) of the Design Study Flow.

Read the step instructions provided. Read spec.md and:

1. Define the problem statement: what is the study trying to resolve?
   Write to study.md with problem definition, success criteria, and
   decision criteria (how candidates will be scored).

2. Establish target throughput, latency, area, power from the spec
   constraints. These become the pass/fail criteria for candidates.
   Write to targets.md.

3. Define the shared workload suite. All candidates will be tested
   against these same workloads. Propose workloads based on the
   operating environment described in the spec. Write workload
   definitions to workloads/.

4. Define UVM-lite testbench requirements for candidate evaluation.
   Write to testbench.md.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DS1 work
session. Read study.md, targets.md, workloads/, and testbench.md and
evaluate:
- Does the problem statement clearly define what the study resolves?
- Are decision criteria specific and measurable?
- Do targets trace back to spec.md constraints?
- Are workloads representative of the operating environment in the spec?
- Are there enough workloads to differentiate candidates (at least
  stress, contention, and realistic mix)?
- Are testbench requirements sufficient to evaluate all targets?

Write findings to .sim-flow/critiques/DS1-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. `study.md` exists with problem definition and decision criteria
2. `targets.md` exists with quantitative values
3. `workloads/` directory has at least one workload definition
4. `testbench.md` exists
5. `.sim-flow/critiques/DS1-critique.md` exists without blockers

---

### DS2: Functional Decomposition

**Purpose:** Decompose the design into functional units. This is the same
decomposition as DM2a but with a study-oriented framing: the decomposition
identifies where candidates can diverge.

**Prerequisites:** DS1 gate passed.

**Instruction files:** `instructions/ds2-decomposition.md`

**Work prompt:**

```text
You are executing step DS2 (Functional Decomposition) of the Design
Study Flow.

Read the step instructions provided. Read spec.md and:

1. Break the design into discrete functional units / operations.
2. Identify data dependencies between operations.
3. Characterize data movement between operations: data types, bit widths,
   rates, burst patterns, fanout.
4. Identify which operations have architectural freedom -- where
   different implementations could produce different candidates.
5. Write the decomposition to analysis/decomposition.md.
6. Write the data movement characterization to analysis/data-movement.md.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DS2 work
session. Read analysis/decomposition.md, analysis/data-movement.md, and
spec.md and evaluate:
- Are all functions in spec.md represented as operations?
- Are there operations not in the spec?
- Are data dependencies correct and complete?
- Is the data movement characterization complete?
- Are architectural freedom points identified -- where can candidates
  diverge?
- Is the decomposition at the right granularity?

Write findings to .sim-flow/critiques/DS2-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. `analysis/decomposition.md` exists with operation list
2. `analysis/data-movement.md` exists
3. `.sim-flow/critiques/DS2-critique.md` exists without blockers

---

### DS3: Pipeline Mapping

**Purpose:** Map operations to pipeline stages. This is where candidates
diverge: different trade-offs produce different architectures.

**Prerequisites:** DS2 gate passed.

**Instruction files:** `instructions/ds3-pipeline-mapping.md`

**Work prompt:**

```text
You are executing step DS3 (Pipeline Mapping) of the Design Study Flow.

Read the step instructions provided. Read spec.md,
analysis/decomposition.md, and study.md.

1. Using the target frequency and technology node, estimate the gate
   budget per cycle.
2. Propose multiple pipeline mapping candidates -- different trade-offs
   in parallelism, sharing, pipeline depth, and resource organization.
   Each mapping becomes a candidate architecture.
3. For each candidate, describe: pipeline stages, operations per stage,
   estimated area, estimated throughput, key trade-off.
4. Write all candidate mappings to analysis/pipeline-mapping.md.
5. Create a candidate directory for each: candidates/<name>/.

Aim for 3-5 candidates that span the design space meaningfully.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DS3 work
session. Read analysis/pipeline-mapping.md and the candidate directories
and evaluate:
- Do the candidates span the design space (not just minor variations)?
- Does each candidate fit within the gate budget per cycle?
- Are there combinational loops in any candidate?
- Are trade-offs clearly articulated for each candidate?
- Is any obvious architecture missing from the candidate set?
- Are all operations from the decomposition mapped in every candidate?

Write findings to .sim-flow/critiques/DS3-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. `analysis/pipeline-mapping.md` exists with multiple candidates
2. `candidates/` directory has at least 2 subdirectories
3. Each candidate directory exists
4. `.sim-flow/critiques/DS3-critique.md` exists without blockers

---

### DS4: Analytical Screening

**Purpose:** Use analytical models to estimate performance, area, and power
for each candidate. Eliminate infeasible candidates before investing in
simulation.

**Prerequisites:** DS3 gate passed.

**Instruction files:** `instructions/ds4-analytical-screening.md`

**Work prompt:**

```text
You are executing step DS4 (Analytical Screening) of the Design Study
Flow.

Read the step instructions provided. Read targets.md and
analysis/pipeline-mapping.md.

1. For each candidate, build analytical models: cycle estimators,
   queueing models, area estimates from gate counts.
2. Run analytical models against all workloads in workloads/.
3. Compare estimates against targets in targets.md.
4. Identify candidates that fail constraints analytically -- these are
   eliminated.
5. Write screening results to analysis/screening-results.md with a
   pass/fail matrix.
6. Write the screening decision (which candidates survive) to
   analysis/screening-decision.md.

Typically narrow to 2-4 candidates worth simulating.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DS4 work
session. Read analysis/screening-results.md and analysis/screening-decision.md
and evaluate:
- Are the analytical models reasonable for the architecture type?
- Are estimates compared against all targets?
- Are eliminations justified with evidence (not just intuition)?
- Is the surviving candidate set large enough to explore the space
  but small enough to simulate practically?
- Are there candidates that barely fail -- should they get a second
  look?

Write findings to .sim-flow/critiques/DS4-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. `analysis/screening-results.md` exists with per-candidate results
2. `analysis/screening-decision.md` exists with surviving candidates listed
3. At least one candidate survives screening
4. `.sim-flow/critiques/DS4-critique.md` exists without blockers

---

### DS5a: Candidate Prototyping (per candidate)

**Purpose:** Build a sim-foundation model for a specific candidate.

**Prerequisites:** DS4 gate passed.

**Instruction files:** `instructions/ds5a-candidate-prototyping.md`

**Work prompt (parameterized per candidate):**

```text
You are executing step DS5a (Candidate Prototyping) for candidate
"<candidate-name>" of the Design Study Flow.

Read the step instructions provided. Read the candidate's pipeline
mapping from analysis/pipeline-mapping.md and the data movement
from analysis/data-movement.md.

1. Create payload types from the data movement characterization.
2. Build a ConnectivityPlan for this candidate's pipeline mapping.
3. Implement modules using Foundation Module, HasLogic, and HasInstances.
4. Write smoke tests: elaboration, basic data flow.
5. Run cargo build and cargo test.

Work in the candidates/<candidate-name>/ directory.

Stop once the model builds and passes smoke tests. A separate critique
session will review your output.
```

**Orchestrator behavior:** The orchestrator runs DS5a once per surviving
candidate from DS4. It reads `analysis/screening-decision.md` to determine
the candidate list and spawns a session for each.

**Gate checks (per candidate):**

1. `candidates/<name>/src/model/` exists
2. `candidates/<name>/Cargo.toml` exists
3. `cargo build` succeeds in candidate directory
4. `cargo test` passes in candidate directory
5. Per-candidate critique file exists without blockers

**Aggregate gate:** All surviving candidates must pass their individual gates.

---

### DS5b: Candidate Smoke Validation (per candidate)

**Purpose:** Run the shared workloads against each candidate and record
baseline results.

**Prerequisites:** DS5a gate passed (all candidates).

**Instruction files:** `instructions/ds5b-candidate-validation.md`

**Work prompt (parameterized per candidate):**

```text
You are executing step DS5b (Candidate Smoke Validation) for candidate
"<candidate-name>" of the Design Study Flow.

Read the step instructions provided. Read workloads/ for the shared
workload suite.

1. Implement the UVM-lite testbench for this candidate per testbench.md.
2. Run all workloads against the candidate.
3. Record results via experiment tracking.
4. Write a results summary to
   candidates/<candidate-name>/analysis/workload-results.md.

A separate critique session will review your output.
```

**Gate checks (per candidate):**

1. At least one experiment run recorded for this candidate
2. Results summary file exists
3. Per-candidate critique file exists without blockers

---

### DS6: Comparison and Narrowing

**Purpose:** Compare all candidates across all workloads and narrow to the
top 1-2.

**Prerequisites:** DS5b gate passed (all candidates).

**Instruction files:** `instructions/ds6-comparison.md`

**Work prompt:**

```text
You are executing step DS6 (Comparison and Narrowing) of the Design
Study Flow.

Read the step instructions provided. Read targets.md and the results
from each candidate in candidates/*/analysis/workload-results.md.

1. Build a comparison matrix: all candidates x all workloads x all
   target metrics.
2. Evaluate each candidate against targets (pass/fail per metric).
3. Apply the decision criteria from study.md to rank candidates.
4. Narrow to the top 1-2 candidates with explicit rationale for each
   elimination.
5. Write the comparison to comparisons/round-1-comparison.md.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DS6 work
session. Read comparisons/round-1-comparison.md, targets.md, study.md,
and the per-candidate workload results and evaluate:
- Is the comparison matrix complete (all candidates x all metrics)?
- Are eliminations justified with evidence?
- Is any candidate eliminated prematurely (close to passing, might
  improve with tuning)?
- Are the surviving candidates genuinely the best, or just the most
  familiar?
- Does the ranking follow the decision criteria in study.md?

Write findings to .sim-flow/critiques/DS6-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. `comparisons/round-1-comparison.md` exists with comparison matrix
2. At least one candidate is identified as surviving
3. `.sim-flow/critiques/DS6-critique.md` exists without blockers

---

### DS7: Deep Analysis

**Purpose:** Deep performance analysis of surviving candidates. Understand
*why* they perform as they do.

**Prerequisites:** DS6 gate passed.

**Instruction files:** `instructions/ds7-deep-analysis.md`

**Work prompt:**

```text
You are executing step DS7 (Deep Analysis) of the Design Study Flow.

Read the step instructions provided. Read the DS6 comparison results
and the surviving candidate list.

For each surviving candidate:
1. Roofline analysis: compute-bound or memory-bound?
2. Bottleneck identification: which module limits throughput?
3. Latency breakdown: per-stage contribution to end-to-end latency.
4. Parameter sweeps targeting identified bottlenecks.
5. PPA refinement (Level 1 analytical estimation).

Write analysis reports to candidates/<name>/analysis/.
Write a cross-candidate analysis summary to
comparisons/deep-analysis-summary.md.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DS7 work
session. Read the per-candidate analysis reports and
comparisons/deep-analysis-summary.md and evaluate:
- Is the analysis thorough enough to explain performance differences
  between candidates?
- Are bottlenecks identified with evidence (not just speculation)?
- Do parameter sweeps make physical sense?
- Are PPA estimates reasonable?
- Is anything inconsistent between candidates that should be the same
  (e.g., same workload giving different results for unexplained reasons)?

Write findings to .sim-flow/critiques/DS7-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. Per-candidate analysis reports exist
2. `comparisons/deep-analysis-summary.md` exists
3. `.sim-flow/critiques/DS7-critique.md` exists without blockers

---

### DS8: Decision

**Purpose:** Select the winning candidate with full evidence.

**Prerequisites:** DS7 gate passed.

**Instruction files:** `instructions/ds8-decision.md`

**Work prompt:**

```text
You are executing step DS8 (Decision) of the Design Study Flow.

Read the step instructions provided. Read study.md for decision criteria,
targets.md for constraints, comparisons/round-1-comparison.md, and
comparisons/deep-analysis-summary.md.

1. Build the final decision matrix: metrics, constraint compliance,
   weighted scoring per study.md decision criteria.
2. Select the winning candidate with explicit rationale.
3. Document rejected candidates with reasons and what would change the
   decision.
4. If no candidate meets all constraints, document the gap and recommend:
   relax constraints, return to DS3 for new candidates, or abandon.
5. Write the decision to comparisons/final-decision.md.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DS8 work
session. Read comparisons/final-decision.md, study.md, and targets.md
and evaluate:
- Does the decision follow the criteria defined in study.md?
- Is the rationale supported by evidence from DS7 analysis?
- Is the winning candidate genuinely the best, or were alternatives
  insufficiently explored?
- If constraints were not met, is the recommended path forward
  reasonable?

Write findings to .sim-flow/critiques/DS8-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. `comparisons/final-decision.md` exists
2. A winning candidate is identified (or an explicit "no winner" with path
   forward)
3. `.sim-flow/critiques/DS8-critique.md` exists without blockers

---

### DS9: Formalize

**Purpose:** Promote the winning candidate into the `final-model/` slot of the
current study project and transition the orchestrator state from `design-study`
to `direct-modeling`. No new project is created -- the study and the final
model live in the same `cargo generate`d project.

**Prerequisites:** DS8 gate passed.

**Instruction files:** `instructions/ds9-formalize.md`

**Work prompt:**

```text
You are executing step DS9 (Formalize) of the Design Study Flow.

Read the step instructions provided. Read comparisons/final-decision.md
for the winning candidate.

1. Update spec.md with the winning candidate's architecture decisions.
   The study spec was intentionally open; the formalization spec must
   be detailed (as required by DM0).
2. Collect the candidate selection evidence: decision matrix, analysis
   results, comparison data. Write formalization-inputs.md summarizing
   what feeds into the DMF.
3. Copy the winning candidate's model code from candidates/<winner>/
   into final-model/ within the same project. Preserve the candidate
   directory for traceability.
4. Do not run `sim-flow init` and do not create a new project. The
   orchestrator will flip state.toml's `flow` field from
   `design-study` to `direct-modeling` and set `current_step` to DM0
   once the DS9 gate passes.

A separate critique session will review your output.
```

**Critique prompt (separate critique session):**

```text
You are a critique session reviewing the work produced by the DS9 work
session. Read the updated spec.md, formalization-inputs.md, and
final-model/ and evaluate:
- Does the updated spec.md contain all information required by DM0
  (frequency, node, detailed functional description, interfaces,
  pipelining, hierarchy)?
- Is the candidate selection evidence preserved and referenced?
- Can the Direct Modeling Flow proceed from DM0 with these inputs
  without losing context from the study?

Write findings to .sim-flow/critiques/DS9-critique.md. Prefix unresolved
issue lines with `UNRESOLVED:` and gate-blocking lines with `BLOCKER:`.
```

**Gate checks:**

1. Updated `spec.md` exists with detailed architecture
2. `formalization-inputs.md` exists
3. `final-model/` exists and contains the winning candidate's source tree
4. `cargo build` succeeds in `final-model/`
5. `.sim-flow/critiques/DS9-critique.md` exists without blockers

**Post-gate orchestrator action:** On gate pass, the orchestrator rewrites
`.sim-flow/state.toml` in place, setting `flow = "direct-modeling"` and
`current_step = "DM0"`. The DSF gate history is preserved under a
`[gates.ds]` subtable for audit.

---

## Per-Candidate Session Management

Steps DS5a and DS5b run once per surviving candidate. The orchestrator manages this by:

1. Reading `analysis/screening-decision.md` to get the candidate list.
2. Maintaining per-candidate gate status in state.toml using a nested subtable:

    ```toml
    [gates.DS5a]
    passed = false   # aggregate across candidates

    [gates.DS5a.candidates]
    "mesh-noc"     = { passed = true, timestamp = "..." }
    "ring-noc"     = { passed = true, timestamp = "..." }
    "crossbar-noc" = { passed = false }
    ```

    The aggregate `passed` flips true only when every entry under
    `[gates.DS5a.candidates]` has `passed = true`. The same schema applies
    to `[gates.DS5b]`.
3. Spawning sessions sequentially (one candidate at a time). Parallel
   per-candidate execution is out of scope for v1 -- candidates share the
   project's working tree, experiments DB, and `.sim-flow/` state, and
   serializing avoids concurrent-write hazards.

The user can target a specific candidate explicitly:

```text
sim-flow run DS5a --candidate mesh-noc
```

With no `--candidate` flag, the orchestrator iterates over the candidate list in order and runs each candidate's work+critique pair before moving on.

## Relationship to Direct Modeling Flow

The Design Study Flow and Direct Modeling Flow share:

- The same orchestrator crate (`sim-flow`)
- The same state management and gate validation infrastructure
- The same AI client abstraction
- The same work+critique session pair pattern and critique-file format

DS9 (Formalize) does **not** initialize a new Direct Modeling Flow instance.
Instead, the orchestrator flips the current project's state from
`design-study` to `direct-modeling` in place (see DS9 gate checks above).
The updated spec.md and the promoted `final-model/` source tree serve as
inputs to DM0 in the same project.

The instruction files are separate because the AI guidance differs:

- DM instructions assume a known design and prescribe specific actions
- DS instructions assume an open design space and guide exploration

## Workloads

Workloads for DSF are shared across all candidates so comparison results are
directly comparable. They are UVM-lite testbenches (Sequencer/Driver/Monitor/
Scoreboard) -- see [uvm-lite.md](../uvm-lite.md). The `workloads/` directory
at the study root is a Rust crate consumed by each candidate via a path
dependency, so every candidate exercises identical stimulus.

## Instruction File Location

Instruction files live in sim-foundation (see [02-direct-modeling-flow.md](02-direct-modeling-flow.md#instruction-file-location)):

```text
sim-foundation/instructions/
    ds0-specification.md
    ds1-study-setup.md
    ds2-decomposition.md
    ds3-pipeline-mapping.md
    ds4-analytical-screening.md
    ds5a-candidate-prototyping.md
    ds5b-candidate-validation.md
    ds6-comparison.md
    ds7-deep-analysis.md
    ds8-decision.md
    ds9-formalize.md
```
