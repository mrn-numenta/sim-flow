# DM2a - Functional Decomposition (critique session)

You are reviewing the DM2a work artifacts. {{ third_party_reviewer_note }} Do not modify the artifacts; evaluate them and write the
critique file.

## Inputs

- `docs/spec.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/data-movement.md`

## Evaluation

{{ critique_kinds }}

1. Is every function described in `docs/spec.md` represented as an operation
   in `docs/analysis/decomposition.md`?
2. Are there operations in the decomposition that are not implied by
   `docs/spec.md`? (invented functionality)
3. Does `docs/analysis/decomposition.md` clearly state the
   gate-budget-per-cycle target or estimate it used from `docs/targets.md`?
4. Is the decomposition granularity consistent with that gate budget and
   likely useful for later pipeline mapping?
5. Are data dependencies between operations correct and complete?
6. Is the data movement characterization complete? Every edge must have
   producer, consumer, data type / meaning, bit width, rate, burst
   pattern, and fanout, plus ordering / flow-control / CDC notes where
   those are relevant.
7. Does the decomposition capture architecturally important boundaries
   such as stateful elements, buffering points, arbitration points,
   queueing points, storage boundaries, or CDC boundaries when they
   materially matter?
8. Is the decomposition at the right granularity -- not one gate per
   operation, not the whole pipeline as one operation?
9. Are operation names identifier-safe (DM2d will use them as Rust
   module and struct names)?
10. Where the spec leaves secondary details implicit, are the inferred
    decomposition boundaries reasonable and clearly documented, rather
    than silently inventing core behavior?

## Output

{{ output_intro }}

{{ critique_output_block }}