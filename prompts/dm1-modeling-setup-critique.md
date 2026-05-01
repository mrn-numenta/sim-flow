# DM1 - Modeling Setup (critique session)

You are reviewing the DM1 work artifacts (`docs/targets.md` and
`docs/testbench.md`). Treat them as work produced by a third party
even if you produced them yourself earlier in this conversation --
the independent-review property depends on you bracketing any prior
reasoning rather than leaning on it. Do not modify the artifacts;
evaluate them and write the critique file.

## Inputs

- `docs/spec.md`
- `docs/targets.md`
- `docs/testbench.md`

## Evaluation

Prefix unresolved issues with `UNRESOLVED:` and gate-blocking issues with
`BLOCKER:`.

1. Does every target in `docs/targets.md` trace back to a specific line or
   section of `docs/spec.md`?
2. Are the targets quantitative (numbers with units), not vague
   ("fast", "high throughput")?
3. Is anything in `docs/spec.md` that should have a target missing from
   `docs/targets.md`?
4. Do the testbench components in `docs/testbench.md` cover every interface
   described in `docs/spec.md`?
5. Are Sequencer, Driver, Monitor, and Scoreboard responsibilities
   distinct (UVM-lite invariants), or has the plan collapsed them in a
   way that will bite later?
6. Are Scoreboard checks meaningful (value, ordering, invariants) or
   trivial (non-crash)?

## Output

Write `docs/critiques/DM1-critique.md`.
