# DM1 - Modeling Setup (work session)

You are executing step DM1 (Modeling Setup) of the Direct Modeling Flow.
Prerequisite: DM0 gate passed. Don't critique your own output here;
that's the critique pass.

## Goal

Derive quantitative verification targets and UVM-lite testbench
requirements from `docs/spec.md`. DM3a will implement the testbench; DM4 will
measure against the targets.

## Procedure

1. Read `docs/spec.md`.
2. Create `docs/targets.md` with a table of verification targets:
   - **throughput**: steady-state items per cycle (or per second given
     the clock frequency). Cite the spec section it derives from.
   - **latency**: end-to-end cycles (or ns) per transaction. Cite the
     spec section.
   - **area**: gate-count budget or sub-block breakdown. If the spec
     does not give a number, record the target as "unconstrained" and
     note the open question.
   - **power**: dynamic/static budget if the spec states one. Otherwise
     "unconstrained".
   - Add any additional design-specific targets the spec implies
     (injection rate, fairness, deadlock freedom, backpressure SLA).
3. Create `docs/testbench.md` listing UVM-lite components needed:
   - **Sequencer** per stimulus class (e.g. uniform, hotspot, burst).
   - **Driver** per external interface.
   - **Monitor** per observable signal / port.
   - **Scoreboard** policies (what is checked and how).
   - Map each component to the interfaces in `docs/spec.md`.
4. Do not duplicate content between files; `docs/targets.md` holds numbers,
   `docs/testbench.md` holds component structure.

## Output

- `docs/targets.md` at the project root.
- `docs/testbench.md` at the project root.

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM1-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
