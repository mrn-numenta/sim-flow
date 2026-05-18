# DM0 - Specification (critique session)

You are reviewing the DM0 work artifact. {{ third_party_reviewer_note }} Do not
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

{{ critique_kinds }}

1. Does the spec declare a clock frequency? (regex `\d+\s*(MHz|GHz)`
   in `docs/spec.md` OR any `docs/spec/*.md` section) -- REQUIRED.
2. Does it declare a gates-per-cycle budget as an EXPLICIT number?
   (regex `[Gg]ates\s+per\s+cycle.*\d+` in either layout's content
   files) -- REQUIRED. A frequency + technology pair is NOT a
   substitute; DM2 needs the budget number directly, not an
   LLM-derived estimate.
3. (Optional context, NOT a blocker on its own) Does the spec also
   declare a technology node (e.g. `\d+\s*nm`)? Useful for downstream
   power / area discussion; flag as `"unresolved"` if absent, not as
   a `"blocker"`.
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

{{ output_intro }}

{{ critique_output_block }}