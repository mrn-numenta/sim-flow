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

Judge `docs/spec.md` by this standard:

- The spec does NOT need to spell out every minute detail.
- It DOES need to preserve explicit requirements and be clear enough that
  a competent modeling agent can infer the rest reasonably.
- A missing detail is a `BLOCKER:` only if it would likely cause two
  competent agents to build materially different models, or if it would
  force later steps to guess at core behavior, interface semantics,
  timing intent, or correctness expectations.
- A missing detail is `UNRESOLVED:` when it is real but safely inferable,
  deferrable, or unlikely to materially change the model.

For each question below, write a one-line answer in the critique file.
Prefix gate-blocking issues with `BLOCKER:` and non-blocking issues with
`UNRESOLVED:`. `RESOLVED:` lines are informational acknowledgements and
are ignored by the gate.

**Finding-marker grammar.** The gate parses lines starting with
`BLOCKER:` / `RESOLVED:` / `UNRESOLVED:` (case-insensitive,
plural OK) optionally preceded by list markers (`-`, `*`, `+`,
`>`), heading markers (`#`+), bold/underline (`**` / `__`), and
one decoration glyph (e.g. `❌` `✅`). Headings DO match
(`### BLOCKER: ...`); section titles describing a blocker
without a colon-after-keyword (e.g. `### BLOCKER 1 - title`)
do NOT match -- they're prose. Mid-sentence mentions do NOT
match. ONLY the keyword-colon shape is a finding; pick the form
deliberately.

1. Does `docs/spec.md` declare a clock frequency? (regex `\d+\s*(MHz|GHz)`)
2. Does it declare a technology node? (regex `\d+\s*nm`)
3. Does it either declare an explicit gate budget per cycle or provide
   enough information for DM1 to derive a reasonable gate-budget-per-cycle
   estimate, usually via frequency plus technology target?
4. Is the design intent clear enough that DM2a can derive major named
   operations without guessing at the core architecture?
5. Are the external interfaces described clearly enough to model I/O
   behavior correctly, including names, widths, direction, protocol, and
   essential semantics?
6. Is the internal dataflow clear enough to infer the main payloads,
   transfers, and connectivity needed for decomposition and
   implementation?
7. Are timing, throughput, flow-control, pipelining, and hierarchy
   specified clearly enough that DM2b can make reasonable staging and
   latency decisions without inventing architectural intent?
8. Are reset, initialization, flush, drain, state, storage,
   arbitration, or exceptional behaviors specified well enough to avoid
   incorrect modeling assumptions where they materially matter?
9. If the design is parameterizable, are the important parameters and
   valid ranges listed clearly enough for the model to be configured
   correctly?
10. Where the spec omits detail, are those omissions safely inferable by
   a competent modeling agent, or are any of them likely to produce
   materially different implementations?
11. Does the spec contain any explicit contradictions, ambiguities, or
    unresolved conflicts that should be called out with specific lines or
    sections?
12. Does the spec include at least one representative scenario or enough
    concrete behavioral detail to anchor later decomposition and
    implementation?
13. Does the document still contain template placeholder text or empty
    sections that hide missing information rather than stating "not
    applicable" or an explicit open question?

When you raise a finding, say why it matters to later steps when that is
not obvious (for example: "BLOCKER: external request ordering semantics
are missing; DM2a/DM2c could build materially different queueing
behavior").

## Output

Write `docs/critiques/DM0-critique.md`. The body format is
free-form markdown; only line-prefix tokens matter to the gate.
