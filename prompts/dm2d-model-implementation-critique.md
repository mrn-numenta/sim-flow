# DM2d - Model Implementation (critique session)

You are reviewing the DM2d work artifacts (the model
implementation under `src/`). Treat them as work produced by a
third party even if you produced them yourself earlier in this
conversation -- the independent-review property depends on you
bracketing any prior reasoning rather than leaning on it. Do not
modify the implementation; evaluate it and write the critique file.

## Inputs

- `docs/plan/plan.md`
- `docs/plan/milestone-*.md`
- `docs/analysis/pipeline-mapping.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/data-movement.md`
- `src/model/` source tree
- `tests/` source tree

## Evaluation

Prefix gate-blocking issues with `BLOCKER:` (DM3 cannot proceed
until fixed). Prefix informational notes with `UNRESOLVED:`. The
orchestrator fails the DM2d gate on `BLOCKER:` lines only.

1. Does the `ConnectivityPlan` topology match
   `docs/analysis/pipeline-mapping.md`?
2. Does each module's `evaluate()` implement the operation(s)
   assigned to that stage in `decomposition.md`?
3. Are payload types consistent with the data widths, types, and
   fanouts in `data-movement.md`?
4. Are there any custom implementations that deviate from
   Foundation patterns (bypassing the port system, manual
   scheduling, violating the evaluate / settle / update phase
   order)?
5. Do all smoke tests pass? Are they meaningful (elaboration, data
   flow, backpressure, idle) or trivial?
6. Is the code organized per Foundation conventions (model / sim /
   test split)?
7. Are operation names from the decomposition reflected in module
   or type names?
8. Does the implementation match the plan? Every milestone in
   `docs/plan/` should have its tasks all `[x]`. Flag tasks still
   `[ ]` (incomplete) and code that doesn't trace back to a plan
   task (out-of-scope drift).

## Output

Write `docs/critiques/DM2d-critique.md`. Free-form markdown body;
only line-prefix tokens (`BLOCKER:`, `UNRESOLVED:`, `RESOLVED:`)
are inspected by the gate.
