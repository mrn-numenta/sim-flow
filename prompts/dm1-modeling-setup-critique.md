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

Record findings in the critique JSON. Use `kind: "blocker"` for
gate-blocking issues, `"unresolved"` for non-blocking notes,
`"resolved"` for informational acknowledgements (ignored by the
gate). See "Output" below for the schema.

1. Does every target in `docs/targets.md` trace back to a specific line or
   section of `docs/spec.md`?
2. Does `docs/targets.md` include a gate-budget-per-cycle target or
   estimate, and is its basis clear?
3. If the gate-budget-per-cycle value is not explicit in `docs/spec.md`,
   is the derivation from frequency and technology target reasonable and
   clearly explained?
4. Is anything in `docs/spec.md` that should have a target missing from
   `docs/targets.md`?
5. Do the targets use appropriate status/provenance (`explicit`,
   `derived`, `inferred`, `unconstrained`, `deferred`) rather than
   presenting guesses as hard requirements?
6. Do the testbench components in `docs/testbench.md` cover every interface
   described in `docs/spec.md`?
7. Does `docs/testbench.md` describe a verification strategy for the
   behaviors and targets that matter, not just a shallow component list?
8. Are Sequencer, Driver, Monitor, and Scoreboard responsibilities
   distinct (UVM-lite invariants), or has the plan collapsed them in a
   way that will bite later?
9. Are Scoreboard checks meaningful (value, ordering, invariants) or
   trivial (non-crash)?
10. Does `docs/testbench.md` name a concrete `lib:examples/<NN-name>/test/`
    implementation baseline, with a rationale that actually matches the
    spec's port shape / stage count / flow-control surface? An absent
    baseline is a `BLOCKER:`; a named baseline whose topology
    demonstrably mismatches the design (e.g.  `04-combinatorial-logic`
    cited for a multi-stage pipeline) is also a `BLOCKER:` -- DM3b
    inherits the choice without re-evaluating it.

## Output

Write the critique as JSON to `docs/critiques/DM1-critique.json`.
The orchestrator renders a human-readable
`docs/critiques/DM1-critique.md` from that JSON automatically; do
NOT write the markdown yourself.

### JSON schema

```json
{
  "step": "DM1",
  "summary": "1-paragraph summary of the critique outcome.",
  "findings": [
    {
      "kind": "blocker",
      "section": "free-form section name",
      "title": "one-line summary of the finding",
      "body": "multi-line markdown explanation; quote offending lines, list remediation"
    }
  ],
  "notes": "optional free-form trailing prose"
}
```

`kind` values: `"blocker"` (gate-blocking), `"unresolved"`
(informational), `"resolved"` (historical / retry-mode). The
schema is strict (`deny_unknown_fields`); typos fail the parse.
