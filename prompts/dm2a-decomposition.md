# DM2a - Functional Decomposition (work session)

You are executing step DM2a (Functional Decomposition) of the Direct
Modeling Flow. Prerequisite: DM1 gate passed. Don't critique your own
output here; that's the critique pass.

## Goal

Break the design into discrete operations and characterize the data
moving between them. DM2b will map operations to pipeline stages; DM2d
will implement them as Foundation modules. Data movement becomes the
payload types and port widths in DM2d. Use the gate-budget-per-cycle
target or estimate from DM1 to choose a decomposition granularity that
is realistic for later pipeline mapping.

## Procedure

1. Read `docs/spec.md` and `docs/targets.md`.
2. Identify the gate-budget-per-cycle target or estimate in
   `docs/targets.md`.
   - Treat it as a hard input to decomposition granularity.
   - If `docs/targets.md` does not provide a usable gate budget, treat
     that as an upstream blocker rather than silently guessing.
3. Check whether `docs/analysis/decomposition.md` and
   `docs/analysis/data-movement.md` exist.
   - If yes, review them against `docs/analysis/decomposition.md.tmpl`
     and `docs/analysis/data-movement.md.tmpl` and fill in any missing
     or incomplete sections.
   - If no, copy `docs/analysis/decomposition.md.tmpl` to
     `docs/analysis/decomposition.md` and
     `docs/analysis/data-movement.md.tmpl` to
     `docs/analysis/data-movement.md`, then use those templates as the
     required structure for this step.
4. Break the design into operations that are meaningful for both
   architectural understanding and later stage mapping.
   - Respect explicit structure from `docs/spec.md`.
   - Use engineering judgement where the spec leaves secondary details
     implicit.
   - Do not invent core behavior or major architectural blocks that are
     not supported by the spec.
   - If more than one decomposition is plausible and the choice would
     materially affect later pipeline mapping or implementation, call it
     out explicitly in the decomposition rather than silently picking one.
5. Enumerate operations. For each operation, record:
   - A short stable name (identifier-safe, snake_case preferred) that
     DM2b, DM2c, and DM2d will reference.
   - Purpose (one sentence).
   - Data inputs (source operation or external port).
   - Data outputs (destination operation or external port).
   - Dominant cost (compute, storage, arbitration, etc.).
   - Whether the operation is primarily combinational, stateful,
     buffering, arbitration/scheduling, or CDC-related.
   - Whether it is likely timing-critical or likely to need splitting
     across multiple stages under the current gate budget.
   - Natural boundaries that matter later: buffering points, arbitration
     points, queueing points, storage boundaries, or clock-domain
     crossings.
6. Write the decomposition to `docs/analysis/decomposition.md`. Use a
   consistent section heading per operation, for example:

   ```
   ## Operation: fetch
   ```

   DM2b and reviewers can scan these headings to cross-reference.
7. In `docs/analysis/decomposition.md`, include a short summary of the
   decomposition strategy:
   - the gate-budget-per-cycle target or estimate you used
   - why the chosen operation granularity is appropriate
   - any ambiguous boundaries or alternative decompositions that were
     considered but not chosen
8. Characterize data movement in `docs/analysis/data-movement.md`:
   - For each edge between operations, record:
     - producer
     - consumer
     - data type / payload meaning
     - bit width
     - rate (items per cycle or per transaction)
     - burst pattern
     - fanout
   - Also record, when relevant:
     - whether the edge is payload, control, credit, response, or mixed
     - ordering semantics
     - flow-control / backpressure notes
     - CDC / clock-domain notes
   - Treat external ports the same way.
9. Use the template headings as the required document structure, but use
   engineering judgement about depth. Remove placeholder text as you
   replace it with real content. If a section truly does not apply, say
   so explicitly rather than leaving placeholder text in place.
10. Granularity target: coarse enough that each operation maps to at
   most a handful of pipeline stages, fine enough that data movement
   between operations is visible and stage boundaries can be reasoned
   about using the gate budget. Avoid "one gate per operation" and
   "the entire pipeline as one operation".

## Output

**Use the path as the fence info-string, verbatim.** Opening
the fence with a language tag (`markdown`, `json`, `toml`, `rust`,
`yaml`, `text`, `md`, `rs`, `yml`, `txt`) means the body is
**silently dropped** -- the file never lands on disk, the gate
fails, and the work session burns its retry budget. See
`_conventions/fenced-blocks.md` ("Language-tag info-strings are
SILENTLY DROPPED") for the failure mode in detail. If you don't
remember the exact path, run `tool:read_file` / `tool:list_dir`
to discover it -- never guess `\`\`\`markdown` as a fallback.

- `docs/analysis/decomposition.md`
- `docs/analysis/data-movement.md`

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM2a-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
