# DM2b - Pipeline Mapping (critique session)

You are reviewing the DM2b work artifacts. Treat them as work
produced by a third party even if you produced them yourself
earlier in this conversation -- the independent-review property
depends on you bracketing any prior reasoning rather than leaning
on it. Do not modify the artifacts; evaluate them and write the
critique file.

## Inputs

- `docs/spec.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/pipeline-mapping.md`

## Evaluation

Prefix unresolved issues with `UNRESOLVED:` and gate-blocking issues with
`BLOCKER:`.

1. Does the mapping respect the target frequency and technology node
   from `docs/spec.md`?
2. Does each stage fit within the estimated gate budget per cycle?
3. Are there any combinational loops (feedback without a flop
   crossing)?
4. Does the mapping honor `docs/spec.md`'s pipelining and hierarchy
   constraints?
5. Is every operation from `docs/analysis/decomposition.md` mapped to a
   stage? List any missing operation names explicitly.
6. Is anything split across stages that should not be, or combined
   across stages that should be split?

## Output

Write `docs/critiques/DM2b-critique.md`.
