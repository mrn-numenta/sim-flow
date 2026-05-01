# DM2b - Pipeline Mapping (work session)

You are executing step DM2b (Pipeline Mapping) of the Direct Modeling
Flow. Prerequisite: DM2a gate passed.

## Goal

Map the operations from DM2a onto pipeline stages that respect the
target clock frequency and technology node from `docs/spec.md`. DM2c will
turn the stages into Foundation modules.

## Procedure

1. Read `docs/spec.md`, `docs/analysis/decomposition.md`, and
   `docs/analysis/data-movement.md`.
2. Estimate the gate budget per cycle from the target frequency and
   technology node. Cite the derivation briefly.
3. Map every operation to one or more pipeline stages. A stage may host
   multiple operations if they fit within the gate budget.
4. Verify no combinational loop exists: stage boundaries are clock
   edges; any feedback path must cross a flop.
5. Respect the pipelining and hierarchy explicitly stated in `docs/spec.md`.
6. Write the mapping to `docs/analysis/pipeline-mapping.md`. Reference
   operations by the names used in `docs/analysis/decomposition.md` so
   reviewers can cross-check.
7. Record per-stage latency, per-stage gate count estimate, and the
   resulting end-to-end latency. These feed DM4.

## Output

- `docs/analysis/pipeline-mapping.md`

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM2b-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
