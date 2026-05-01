# DM2a - Functional Decomposition (critique session)

You are reviewing the DM2a work artifacts. Treat them as work
produced by a third party even if you produced them yourself
earlier in this conversation -- the independent-review property
depends on you bracketing any prior reasoning rather than leaning
on it. Do not modify the artifacts; evaluate them and write the
critique file.

## Inputs

- `docs/spec.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/data-movement.md`

## Evaluation

Prefix unresolved issues with `UNRESOLVED:` and gate-blocking issues with
`BLOCKER:`.

1. Is every function described in `docs/spec.md` represented as an operation
   in `docs/analysis/decomposition.md`?
2. Are there operations in the decomposition that are not implied by
   `docs/spec.md`? (invented functionality)
3. Are data dependencies between operations correct and complete?
4. Is the data movement characterization complete? Every edge must have
   data type, bit width, rate, burst pattern, and fanout.
5. Is the decomposition at the right granularity -- not one gate per
   operation, not the whole pipeline as one operation?
6. Are operation names identifier-safe (DM2d will use them as Rust
   module and struct names)?

## Output

Write `docs/critiques/DM2a-critique.md`.
