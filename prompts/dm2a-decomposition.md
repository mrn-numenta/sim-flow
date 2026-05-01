# DM2a - Functional Decomposition (work session)

You are executing step DM2a (Functional Decomposition) of the Direct
Modeling Flow. Prerequisite: DM1 gate passed. Don't critique your own
output here; that's the critique pass.

## Goal

Break the design into discrete operations and characterize the data
moving between them. DM2b will map operations to pipeline stages; DM2c
will implement them as Foundation modules. Data movement becomes the
payload types and port widths in DM2c.

## Procedure

1. Read `docs/spec.md` and `docs/targets.md`.
2. Enumerate operations. For each operation, record:
   - A short stable name (identifier-safe, snake_case preferred) that
     DM2b and DM2c will reference.
   - Purpose (one sentence).
   - Data inputs (source operation or external port).
   - Data outputs (destination operation or external port).
   - Dominant cost (compute, storage, arbitration, etc.).
3. Write the decomposition to `docs/analysis/decomposition.md`. Use a
   consistent section heading per operation, for example:

   ```
   ## Operation: fetch
   ```

   DM2b and reviewers can scan these headings to cross-reference.
4. Characterize data movement in `docs/analysis/data-movement.md`:
   - For each edge between operations, record data type, bit width,
     rate (items per cycle or per transaction), burst pattern, and
     fanout.
   - Treat external ports the same way.
5. Granularity target: coarse enough that each operation maps to at
   most a handful of pipeline stages, fine enough that data movement
   between operations is visible. Avoid "one gate per operation" and
   "the entire pipeline as one operation".

## Output

- `docs/analysis/decomposition.md`
- `docs/analysis/data-movement.md`

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM2a-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
