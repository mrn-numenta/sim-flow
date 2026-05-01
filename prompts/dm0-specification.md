# DM0 - Specification (work session)

You are executing step DM0 (Specification) of the Direct Modeling Flow.
Don't critique your own output here; that's the critique pass.

## Goal

Produce or validate `docs/spec.md` at the project root. The specification is
an input for every later DMF step (DM1 targets, DM2a decomposition,
DM3 testbench, DM4 analysis). Gaps here ripple forward.

## Procedure

1. Check whether `docs/spec.md` exists.
   - If yes, review it against the required content below and fill in
     any gaps.
   - If no, draft a skeleton and walk the user through filling it in.
2. Ensure `docs/spec.md` contains:
   - **Clock frequency** (e.g., "2 GHz", "1500 MHz") -- must match the
     regex `\d+\s*(MHz|GHz)`.
   - **Technology node** (e.g., "7 nm", "5nm") -- must match the regex
     `\d+\s*nm`.
   - **Detailed functional description** -- inputs, outputs, the
     transformation between them. Structured headings preferred.
   - **Internal and external interfaces** -- port names, widths,
     protocols, direction, and where they connect.
   - **Pipelining and hierarchy** -- intended pipeline depth, stage
     boundaries, and sub-modules.
   - **Parameterization** -- if the design is parameterizable, list the
     parameters and their valid ranges.
3. Do not invent requirements the user has not stated. Flag ambiguities
   by adding a brief "Open Questions" subsection; do not silently
   resolve them.

## Output

- `docs/spec.md` at the project root, updated or newly created.

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM0-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
