# DM0 - Specification (critique session)

You are reviewing the DM0 work artifact. Treat it as work produced
by a third party even if you produced it yourself earlier in this
conversation -- the independent-review property depends on you
bracketing any prior reasoning rather than leaning on it. Do not
modify the spec; evaluate it and write the critique file.

## Inputs

The spec lands in one of two layouts; the gate accepts either, and
your review applies to whichever is on disk:

- **Single-file:** `docs/spec.md` at the project root. Read it
  directly.
- **Paginated:** numbered section files under `docs/spec/`
  (e.g. `docs/spec/01-overview.md`,
  `docs/spec/02-interfaces.md`, ...). The system stack's TOC
  block lists every section file with its size; use `read_file`
  per section, or `list_dir docs/spec/` if the TOC isn't already
  in scope. Treat the union of section files as "the spec" for
  the questions below; quote the section file path + line number
  when citing offending content.

Judge the spec on its own merits; any transcript or prior reasoning
you happen to have access to is not authoritative -- what's on
disk is.

## Evaluation

Judge the spec (whichever layout) by this standard:

- The spec does NOT need to spell out every minute detail.
- It DOES need to preserve explicit requirements and be clear enough that
  a competent modeling agent can infer the rest reasonably.
- A missing detail is a `BLOCKER:` only if it would likely cause two
  competent agents to build materially different models, or if it would
  force later steps to guess at core behavior, interface semantics,
  timing intent, or correctness expectations.
- A missing detail is `UNRESOLVED:` when it is real but safely inferable,
  deferrable, or unlikely to materially change the model.

For each question below, record a finding in the critique JSON.
Use `kind: "blocker"` for gate-blocking issues, `"unresolved"`
for open follow-ups that also block the gate until resolved, `"resolved"` for informational
acknowledgements (ignored by the gate). See "Output" below for
the schema.

1. Does the spec declare a clock frequency? (regex `\d+\s*(MHz|GHz)`
   in `docs/spec.md` OR any `docs/spec/*.md` section)
2. Does it declare a technology node? (regex `\d+\s*nm` in either
   layout's content files)
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
not obvious -- the finding's `body` field is the right place for the
"why" prose.

## Output

The canonical shape is a fenced artifact-write block whose
info-string is the destination path. Emit the JSON inline between
the open and close fence -- no leading prose, no `json` language
tag:

```docs/critiques/DM0-critique.json
{ ... critique JSON, see "JSON schema" below ... }
```

Bare-prose `{ ... }` JSON or a ` ```json ` fence is recoverable
(the orchestrator's `salvage_critique_json` path catches both) but
wastes a parser pass. Emit the canonical fenced form directly so
the file lands first-try.

Write the critique as JSON to `docs/critiques/DM0-critique.json`.
The orchestrator renders a human-readable
`docs/critiques/DM0-critique.md` from that JSON automatically; do
NOT write the markdown yourself.

### JSON schema

```json
{
  "step": "DM0",
  "summary": "1-paragraph summary of the critique outcome.",
  "findings": [
    {
      "kind": "blocker",
      "section": "free-form section name (e.g. \"External Interfaces\")",
      "title": "one-line summary of the finding",
      "body": "multi-line markdown explanation; quote offending lines, list remediation"
    }
  ],
  "notes": "optional free-form trailing prose"
}
```

`kind` values: `"blocker"` (gate-blocking), `"unresolved"`
(informational), `"resolved"` (historical / retry-mode). The
schema is strict (`deny_unknown_fields`); typos fail the parse
and the orchestrator surfaces "malformed critique JSON". Use the
exact field names listed.
