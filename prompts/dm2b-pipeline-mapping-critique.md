# DM2b - Pipeline Mapping (critique session)

You are reviewing the DM2b work artifacts. Treat them as work
produced by a third party even if you produced them yourself
earlier in this conversation -- the independent-review property
depends on you bracketing any prior reasoning rather than leaning
on it. Do not modify the artifacts; evaluate them and write the
critique file.

## Inputs

- `docs/spec.md`
- `docs/targets.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/data-movement.md`
- `docs/analysis/pipeline-mapping.md`

## Evaluation

Record findings in the critique JSON (see "Output" below for the
schema). `kind: "blocker"` and `kind: "unresolved"` both block the gate; `"resolved"` is
historical / retry-mode.

1. Does the mapping use the canonical gate-budget-per-cycle target or
   estimate from `docs/targets.md`, and is that usage clearly stated?
2. Does the mapping respect the target frequency and technology node
   from `docs/spec.md`?
3. Does each stage fit within the estimated gate budget per cycle?
4. Are there any combinational loops (feedback without a flop
   crossing)?
5. Does the mapping honor `docs/spec.md`'s pipelining and hierarchy
   constraints?
6. Is every operation from `docs/analysis/decomposition.md` mapped to a
   stage? List any missing operation names explicitly.
7. Are important boundaries from DM2a -- such as buffering, arbitration,
   queueing, storage, feedback, or CDC boundaries -- preserved where they
   materially matter?
8. Is the stage rationale well explained, or are important stage-boundary
   decisions asserted without justification?
9. Is anything split across stages that should not be, or combined
   across stages that should be split?
10. Does `docs/analysis/pipeline-mapping.md` provide enough per-stage
    detail for DM2d to implement the stage structure without having to
    rediscover the intended boundaries?

## Output

Write the critique as JSON to
`docs/critiques/DM2b-critique.json`. The orchestrator renders a
human-readable `docs/critiques/DM2b-critique.md` from that JSON
automatically; do NOT write the markdown yourself.

### JSON schema

```json
{
  "step": "DM2b",
  "summary": "1-paragraph summary of the critique outcome.",
  "findings": [
    {
      "kind": "blocker",
      "section": "free-form section name",
      "title": "one-line summary of the finding",
      "body": "multi-line markdown explanation"
    }
  ],
  "notes": "optional free-form trailing prose"
}
```

`kind` values: `"blocker"`, `"unresolved"`, `"resolved"`. Schema
is strict (`deny_unknown_fields`); typos fail the parse.
