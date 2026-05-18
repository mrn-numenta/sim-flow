# DM0 - Specification (critique session)

You are reviewing the DM0 work artifact at `docs/spec.md`.
{{ third_party_reviewer_note }} Do not modify the spec; evaluate it and
write the critique file.

## Input

- `docs/spec.md` at the project root: the structured Chapter 2 schema
  spec that DM0's work session produced.
- `.sim-flow/spec-ingest/manifest.toml` and the per-chunk corpus under
  `.sim-flow/spec-ingest/` -- consult via `spec_semantic_search` and
  `read_file` when you need source-spec context the spec.md has
  normalized away.

Judge the spec on its own merits and against the source-spec corpus.
Any transcript or prior reasoning you might have access to is not
authoritative -- what's on disk is.

## The schema you are checking against

`docs/spec.md` follows a fixed top-level section order. The
high-level structure is:

| H2 heading | Required? | Shape |
| --- | --- | --- |
| `Metadata` | required | definition list |
| `Purpose` / `Scope` / `Non-goals` | required | prose |
| `Assumptions and Constraints` | required | `### Quantitative` table + prose subsections |
| `External Interfaces` | required if any | per-interface H3 subsections with a signal table |
| `Blocks` | required | per-block H3 subsections with I/O table + behavior summary |
| `Parameters` | required if any | single table |
| `State Machines` / `Encodings` / `Memory Map` / `Connectivity` / `Error Handling` / `Cycle-Accurate Behavior` / `Figures` | optional | typed tables / per-entry subsections |
| `Functional Behavior` | required | prose + operation list |
| `Timing, Latency, and Throughput` | required | prose + optional latency table |
| `Pipeline and Hierarchy` | required | short prose pointing at Blocks |
| `Reset, Initialization, Flush, Drain` | required | prose |
| `Worked Examples` | required | at least one scenario |
| `Source-Spec Anchors` | required | index table |
| `Open Questions` | required | bullet list |
| `Auto-decisions` | required | bullet list |

Signal tables use canonical column sets:

- External Interfaces signal table:
  `Signal | Direction | Width | Type | Required | Description`.
- Blocks I/O signal table:
  `Signal | Direction | Peer | Description`.

Parameter table: `Name | Type | Default | Valid range | Behavioral
impact | Source-anchor`.

Source-spec anchors are one of:
`<source>:p<N>` / `<source>:p<N>-<M>` / `<source>:chunk-<NNN>`.

The DM0 gate engine separately enforces:
- The required `Clock frequency` row (value matching
  `\d+\s*(MHz|GHz)`) in the Quantitative table.
- The required `Gate budget per cycle` row (value containing a
  number) in the Quantitative table.
- Every source-spec anchor resolves to a real chunk in
  `manifest.toml`.
- `Auto-decisions` is non-empty when running in automated mode.

Findings you raise here are semantic checks ABOVE that
structural / regex gate.

## Walk

Per Architecture Chapter 6 §6.3 Step C, walk the structured spec
checking semantic consistency:

1. **Metadata sanity.** Are `Design name`, `Version`, `Status`,
   `Authors`, and `Source documents` filled? Does `Source documents`
   include every peer registered in `manifest.toml.peers[].id`?
2. **Purpose / Scope / Non-goals prose.** Are they short, focused, and
   non-redundant? Does the prose describe the design's intent rather
   than restating the section heading?
3. **Quantitative table.** Beyond the regex-gated rows, are the values
   plausible given the source spec? Does any quantitative row carry an
   obviously wrong unit (e.g. `1 ns` for a clock frequency)?
4. **External Interfaces.** Does every declared interface have a
   non-empty signal table? Do the signal `Width` / `Type` values look
   plausible for the protocol? Do the Source-spec anchors actually
   resolve to chunks describing this interface? Spot-check at least one
   interface via `spec_semantic_search` against the source.
5. **Blocks.** Every Block must carry:
   - A non-empty `Behavior summary` (warning if under ~50 chars).
   - A `Parent` that names another declared block or
     `(none -- top-level)`.
   - An `#### I/O Signals` table (warning if empty -- the auto-populate
     pass usually fills these).
   - At least one Source-spec anchor.
   Walk the source-anchor list per block: each anchor's
   `<source>:p<N>` form must map to a chunk in the manifest.
6. **Signal-table consistency.** Use `signal_table_query` with
   `conflicts_only = true` to surface any (stage, signal_name) pair
   where the row in spec.md disagrees with the source-spec row on
   direction, peer, width, or description. Each conflict is a finding;
   record an `unresolved` for benign rewording, a `blocker` for a
   direction / width disagreement.
7. **Parameters.** Are `Type` / `Default` / `Valid range` filled? Does
   `Behavioral impact` carry useful prose (not a tautology like
   `"Sets X"`)?
8. **Functional Behavior.** Does `End-to-end behavior` describe what
   the design DOES, in plain language, before diving into
   `Operation flow`? Is each entry in `Operation flow` a single
   well-named operation with a backtick-quoted id + a one-line
   purpose? Does `Data movement` describe payload flow rather than
   restating the operation list?
9. **Timing, Pipeline, Reset.** Are stalls, backpressure, flush, and
   reset behavior specified well enough that DM2b can make staging
   decisions without inventing intent?
10. **Worked Examples.** Is there at least one concrete scenario with
    explicit Inputs / Expected flow / Expected outputs? A worked
    example is the only safety net against ambiguous prose; absence
    is a blocker.
11. **Source-Spec Anchors index.** Does the index cover the spec.md
    sections that carry anchors elsewhere in the document? Are the
    `Chunk id` values present (not `"TBD"`)?
12. **Open Questions vs Auto-decisions.** Are entries in `Open
    Questions` genuinely open (i.e. nothing in the source spec
    answers them)? Use `spec_semantic_search` to spot-check; an
    answered TBD that's still in Open Questions is a finding (move
    to Auto-decisions or resolve in-section). Conversely, are
    Auto-decisions backed by evidence or do any of them look like
    LLM guesses for which an `ask_user` was warranted?
13. **Spec.md vs source spec coverage.** Pick a handful of important
    source-spec sections (the design's headline blocks, the
    parameters table, the interfaces) and use `spec_semantic_search`
    to verify they show up in spec.md with anchors back to those
    chunks. A source-spec section that the spec.md never references
    is a finding (`unresolved`) unless the section was intentionally
    out of scope.

For each question above, when you raise a finding, the `body` field
should explain WHY the issue matters to later steps -- spec.md is the
input to DM1 / DM2 / DM3, and the cost of an ambiguity here propagates
through every downstream step.

## What counts as blocker vs unresolved vs resolved

- `BLOCKER` -- the issue would cause two competent agents to build
  materially different models, OR force a later step to guess at core
  behavior, interface semantics, timing intent, or correctness
  expectations.
- `UNRESOLVED` -- the issue is real but safely inferable, deferrable,
  or unlikely to materially change the model.
- `RESOLVED` -- informational; ignored by the gate. Use this to record
  positive findings (e.g. "the Worked Examples section is unusually
  clear") that help downstream agents calibrate trust in the spec.

A missing detail that the source spec also did not specify is
typically `UNRESOLVED`, not `BLOCKER`, unless the missing detail
blocks DM2 (e.g. a missing top-level interface signal).

{{ critique_kinds }}

## Output

{{ output_intro }}

{{ critique_output_block }}
