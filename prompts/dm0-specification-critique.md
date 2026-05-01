# DM0 - Specification (critique session)

You are reviewing the DM0 work artifact (`docs/spec.md`). Treat it
as work produced by a third party even if you produced it yourself
earlier in this conversation -- the independent-review property
depends on you bracketing any prior reasoning rather than leaning
on it. Do not modify `docs/spec.md`; evaluate it and write the
critique file.

## Inputs

- `docs/spec.md` at the project root. Judge it on its own merits;
  any transcript or prior reasoning you happen to have access to is
  not authoritative -- the spec is.

## Evaluation

For each question below, write a one-line answer in the critique file.
Prefix every gate-blocking issue with `BLOCKER:` (DM1 cannot proceed
until it is fixed). Prefix informational notes -- nits, follow-up
questions, things you can work around -- with `UNRESOLVED:`. The
orchestrator fails the DM0 gate on `BLOCKER:` lines only; `UNRESOLVED:`
lines do not block advancement.

1. Does `docs/spec.md` declare a clock frequency? (regex `\d+\s*(MHz|GHz)`)
2. Does it declare a technology node? (regex `\d+\s*nm`)
3. Is the functional description detailed enough to decompose into
   discrete operations in DM2a?
4. Are all interfaces (internal and external) described with names,
   widths, and protocols?
5. Are pipelining and hierarchy specified?
6. If the design is parameterizable, are the parameters and ranges
   listed?
7. Is anything ambiguous or contradictory? Call out specific lines.
8. Is anything missing that would block DM1 target derivation?

Flag each question whose answer is insufficient with `UNRESOLVED:` (the
AI can keep working around it) or `BLOCKER:` (DM1 cannot proceed).
RESOLVED: lines are informational acknowledgements, ignored by the gate.

## Output

Write `docs/critiques/DM0-critique.md`. The body format is
free-form markdown; only line-prefix tokens matter to the gate.
