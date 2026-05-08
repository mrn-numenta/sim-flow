# DM0 - Specification (work session)

You are executing step DM0 (Specification) of the Direct Modeling Flow.

## Goal

Produce or validate `docs/spec.md` at the project root. The specification is
an input for every later DMF step. The goal is not to exhaustively spell out
every possible detail; the goal is to capture the design intent clearly enough
that a competent modeling agent can decompose, pipeline, and implement the
model without guessing at core behavior.

## Two kinds of "spec"

This step distinguishes between two artifacts that both have "spec" in
the name; do not conflate them:

- **Source spec** -- the user-supplied document the orchestrator
  ingested before this session started. When present, it lives at:
  - `.sim-flow/source-spec.md` (or `.sim-flow/source-spec.<ext>` for
    PDF / TXT inputs the orchestrator paginated for you)
  - `.sim-flow/spec-pages/<NNN>.md` -- one file per source page,
    zero-padded; use these for `read_file` lookups
  - `.sim-flow/source-spec-toc.md` -- table of contents the
    orchestrator may have inlined into the system stack
  Treat the source spec as authoritative input. **Do not modify it**
  (`.sim-flow/` is the orchestrator's tree).
- **Sim-flow spec** -- the structured artifact you produce. Two
  acceptable layouts; downstream steps and the gate accept either:
  - **Single-file:** `docs/spec.md` at the project root. Use this
    when the spec is small enough to fit comfortably in one
    response (rough rule: under ~500 lines of markdown).
  - **Paginated:** a directory `docs/spec/` containing numbered
    section files (`docs/spec/01-overview.md`,
    `docs/spec/02-interfaces.md`, ...). Use this for large designs
    where a single response would exceed your output budget. The
    file numbers establish the canonical reading order; the
    section slug is for human readability. Each file holds one
    self-contained section.
    - The orchestrator inlines the section directory listing into
      every downstream step, so DM1 / DM2a / DM2b / etc. can
      `read_file` individual sections on demand without you
      having to maintain a hand-written TOC.
    - You MAY also write a brief `docs/spec.md` at the project
      root that points readers at the section directory; it is
      not required by the gate. If you write it, keep it short
      (intent + link to `docs/spec/`); the bulk content goes in
      the numbered files.
  Either layout is the input to every later DM step. **Pick one
  layout per project and stick with it** -- mixing a populated
  `docs/spec.md` with a populated `docs/spec/` is confusing for
  downstream readers.

## Procedure

1. Check whether `docs/spec.md` exists.
   - If yes, review it against `docs/spec.md.tmpl` and fill in any
     missing or incomplete sections.
   - If no, copy `docs/spec.md.tmpl` to `docs/spec.md` and use the
     template as the required structure for this step.
2. Check whether `.sim-flow/source-spec*` exists (for example via
   `read_file` on `.sim-flow/source-spec-toc.md` or `Glob`).
   - If yes, the user provided a source document. Read it selectively
     (use the TOC and per-page files for large specs; do not request
     everything at once) and fill in `docs/spec.md` from it.
   - Treat the source spec as authoritative. Do not silently invent
     requirements that are not supported by the source.
   - When the source is ambiguous, incomplete, or contradictory, record
     that in `## Open Questions` and cite source-spec page numbers.
3. If no source spec was provided, or if the source spec does not answer
   everything needed to complete `docs/spec.md`, fill in the template
   from the listed predecessor inputs and target artifacts as far as
   you can. For the rest:
   - In automated mode (the automated-mode notes appear earlier in
     the system context above), make your best educated guess and
     record each non-trivial assumption in `## Auto-decisions`.
   - In manual mode (the manual-mode notes appear earlier in the
     system context above), pick the single most important missing
     field and ask the user one concrete question about it. Update
     `docs/spec.md` once they answer, then ask about the next most
     important missing field on the next turn. Do not bulk-guess in
     manual mode.
4. Use the template headings as the required document structure, but use
   engineering judgement about depth. The template is scaffolding for a
   clear, consistent spec; it is not a demand that every field be
   exhaustively detailed. Focus on capturing enough normative information
   for downstream modeling.
5. Prefer explicit requirements over inferred detail.
   - If the source material or user gives a detail explicitly, preserve it.
   - If two explicit requirements conflict, call that out in
     `## Open Questions`; do not silently pick one.
   - If a secondary detail is omitted but can be reasonably inferred
     without changing architectural behavior, interface semantics, timing
     intent, or correctness expectations, you may infer it.
   - If a missing detail would likely cause two competent agents to build
     materially different models, do not guess silently. Ask the user in
     manual mode or record an auto-decision in automated mode.
6. At minimum, `docs/spec.md` must include enough information for later
   steps to infer the rest reasonably. That usually means:
   - **Technology node** matching regex `\d+\s*nm`
   - **Clock frequency** matching regex `\d+\s*(MHz|GHz)`
   - **Gate budget per cycle** as a single concrete line in the spec.
     If the source material gives an explicit value, copy it verbatim.
     Otherwise, derive one from the frequency + technology node and
     write it explicitly using a "Derived gate budget per cycle" line,
     e.g.:
     `Derived gate budget per cycle: ~50–100 (1 GHz at 7 nm, FO4 ~10 ps).`
     Either form satisfies the requirement; downstream steps and the
     critique gate look for the number, not the wording around it.
   - **External interfaces** with names, widths, protocols, direction,
     and semantics
   - **Functional behavior** detailed enough to derive named operations
     and data movement
   - **Timing, latency, throughput, and flow-control behavior**
   - **Pipeline and hierarchy intent**
   - **Reset / initialization / flush behavior**
   - **Parameters and valid ranges**, when applicable
   - **Representative examples or scenarios** detailed enough to trace
     expected behavior when the design would otherwise be ambiguous
   - **Open Questions** for unresolved ambiguity and **Auto-decisions**
     for non-trivial assumptions in automated mode
7. Remove placeholder text as you replace it with real content. If a
   section truly does not apply, say so explicitly rather than leaving
   the placeholder in place.
8. The gate-budget requirement is hard because DM2 needs it to reason
   about functional decomposition and pipeline staging. ALWAYS land a
   concrete number in `docs/spec.md` — either copied from the source
   material or computed from the frequency + technology target via the
   "Derived gate budget per cycle: ..." line shown above. Do not leave
   the budget implicit and rely on a downstream step to do the
   derivation; weaker critique models read the absence of a literal
   number as a blocker even when the surrounding context allows
   derivation.
9. Do not stop after creating a partially filled scaffold. The goal of
   this step is a model-ready `docs/spec.md` that preserves explicit
   requirements and makes downstream inference safe and bounded.

## Output

EITHER:

- `docs/spec.md` at the project root, updated or newly created
  (single-file layout for small specs).

OR:

- `docs/spec/<NN>-<slug>.md` files for each section, numbered to
  establish reading order (e.g.
  `docs/spec/01-overview.md`,
  `docs/spec/02-interfaces.md`,
  `docs/spec/03-functional-behavior.md`,
  `docs/spec/04-timing-throughput.md`,
  `docs/spec/05-reset-and-corner-cases.md`,
  `docs/spec/06-examples.md`,
  `docs/spec/07-open-questions.md`).
  The numbered prefix is REQUIRED for canonical ordering; the
  slug after the number is free-form (lower-case, hyphenated).
  Cover the same content as the single-file layout -- the
  template's section structure maps onto one or more numbered
  files per section group.

When the artifacts above are complete, stop. Do not write
`docs/critiques/DM0-critique.json` (the critique is a distinct
task) and do not write a hand-rolled `docs/spec.md` index when
using the paginated layout (the orchestrator surfaces the
section listing automatically). Do not `/exit` on your own --
the user and the orchestrator control session boundaries.
