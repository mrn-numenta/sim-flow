# DM2b - Pipeline Mapping (work session)

You are executing step DM2b (Pipeline Mapping) of the Direct Modeling
Flow. Prerequisite: DM2a gate passed.

## Goal

Map the operations from DM2a onto pipeline stages that respect the
target clock frequency, technology node, and gate-budget-per-cycle target
or estimate already established by DM1. DM2d will turn the stages into
Foundation modules.

## Procedure

1. Read `docs/spec.md`, `docs/targets.md`,
   `docs/analysis/decomposition.md`, and
   `docs/analysis/data-movement.md`.
2. Identify the gate-budget-per-cycle target or estimate in
   `docs/targets.md`.
   - Treat it as the canonical budget for this step.
   - Do not recompute a different budget from scratch unless
     `docs/targets.md` explicitly records an alternative with rationale.
   - If `docs/targets.md` does not provide a usable gate budget, treat
     that as an upstream blocker rather than silently guessing.
3. Check whether `docs/analysis/pipeline-mapping.md` exists.
   - If yes, review it against `docs/analysis/pipeline-mapping.md.tmpl`
     and fill in any missing or incomplete sections.
   - If no, copy `docs/analysis/pipeline-mapping.md.tmpl` to
     `docs/analysis/pipeline-mapping.md`, then use that template as the
     required structure for this step.
4. Map every operation to one or more pipeline stages.
   - A stage may host multiple operations if they fit within the gate
     budget and form a sensible implementation boundary.
   - Respect explicit pipelining and hierarchy from `docs/spec.md`.
   - Preserve important boundaries surfaced in DM2a such as buffering,
     arbitration, queueing, storage, feedback, and CDC boundaries when
     they materially matter.
5. For each stage, record:
   - stage name
   - purpose
   - operations assigned
   - gate-count estimate
   - latency contribution
   - register / buffering assumptions
   - important boundaries and notes
6. If more than one stage split is plausible:
   - choose the most defensible mapping under the current gate budget
   - document the rationale
   - record meaningful alternatives or unresolved questions rather than
     silently choosing when the choice materially matters
7. Verify no combinational loop exists: stage boundaries are clock
   edges; any feedback path must cross a flop.
8. Write the mapping to `docs/analysis/pipeline-mapping.md`. Reference
   operations by the names used in `docs/analysis/decomposition.md` so
   reviewers can cross-check.
9. Record per-stage latency, per-stage gate count estimate, and the
   resulting end-to-end latency. These feed DM4.
10. Use the template headings as the required document structure, but use
    engineering judgement about depth. Remove placeholder text as you
    replace it with real content. If a section truly does not apply, say
    so explicitly rather than leaving placeholder text in place.

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

- `docs/analysis/pipeline-mapping.md`

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM2b-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
