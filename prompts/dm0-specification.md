# DM0 - Specification (work session)

You are executing step DM0 (Specification) of the Direct Modeling Flow.
The artifact you produce is `docs/spec.md`: the structured, anchor-bearing
normalization of the source spec (or, when there is no source spec, of the
user's design intent). Every later DM step (DM1, DM2*, DM3*, the critiques,
and the gate engine) reads `docs/spec.md` as authoritative truth.

## Two kinds of "spec"

- **Source spec** -- the user-supplied document the orchestrator ingested
  before this session started. When present, the ingest pipeline has
  chunked, classified, and indexed it under
  `.sim-flow/spec-ingest/`. Use the `spec_semantic_search` tool to find
  relevant chunks and `read_file` on the returned `chunk_path` to read the
  full body. **Do not modify** anything under `.sim-flow/` -- it is the
  orchestrator's tree.
- **Sim-flow spec** -- `docs/spec.md`: the structured artifact you produce.
  Single file at the project root. Use the schema described below.

### Source-spec access discipline (read this first)

The source spec is accessed via retrieval tools, not by browsing the
filesystem:

- **All source-spec lookups go through `spec_semantic_search` or
  `signal_table_query`.** These tools own retrieval over the ingest
  corpus under `.sim-flow/spec-ingest/`. They return relevant
  `chunk_path` values with `breadcrumb`, page range, and a snippet.
  When the snippet is insufficient, `read_file` the returned
  `chunk_path` for the full body -- that is the ONLY context in which
  you should `read_file` anything inside `.sim-flow/`.
- **Do NOT enumerate, glob, or guess paths under `.sim-flow/`.** The
  orchestrator's tree is not browsable by the agent; the search tools
  are the contract.
- The auto-populate step has ALREADY filled the deterministic parts of
  `docs/spec.md` from the ingest corpus before this session started
  (see "What you arrive to" below). Your first move should be to read
  `docs/spec.md` -- not to crawl the corpus.

## The structured spec.md schema

`docs/spec.md` follows a fixed top-level section order. Required sections
MUST be present; optional sections are present only when the design has
that feature. The orchestrator parses your output with a markdown parser
that keys on H2 headings and column-header conventions, so heading text
must match exactly.

### Enforced on write (NEW)

`docs/spec.md` is special: every `write_file` and `edit_file` call
targeting it runs the structured-schema validator BEFORE the write
lands on disk. If the proposed content fails to parse, is missing a
required H2 section, or fails cross-reference / anchor / quantitative-
row checks, the write is REFUSED with a structured error listing
every violation. The file on disk is unchanged. You must fix every
listed issue and retry. The orchestrator will not "round down" or
silently accept a malformed spec.md — there is no path where DM0
advances on an invalid file.

Practical consequences:

- Prefer `edit_file` over `write_file docs/spec.md`. The
  auto-populate step on session start seeds the canonical schema;
  `edit_file` targeted edits preserve it. A full `write_file
  docs/spec.md` rewrite is allowed but you must match the schema
  exactly or the write will be rejected.
- Heading strings are case-sensitive and verbatim. `## Purpose And
  Scope` is NOT the same as `## Purpose` + `## Scope` + `## Non-goals`
  — the validator names each missing section explicitly.
- Table column headers must match the documented set exactly.
- Block parents must reference a declared block by name, or be empty,
  or be the literal `(none -- top-level)`. The validator rejects any
  other value.
- Anchors must be `<source>:p<N>`, `<source>:p<N>-<M>`, or
  `<source>:chunk-<NNN>`. Stray text, missing colon, or unknown
  form → rejected.

### Required sections (in this order)

1. `# <Project Name> Design Specification` -- H1 title.
2. `## Metadata` -- definition-list block: `Design name`, `Version`,
   `Status` (`draft | reviewed | approved`), `Authors`, `Source
   documents` (primary + peers, each with role + path), `Last updated`.
3. `## Purpose` -- one to three short prose paragraphs.
4. `## Scope` -- one to three short prose paragraphs.
5. `## Non-goals` -- one to three short prose paragraphs.
6. `## Assumptions and Constraints` -- a `### Quantitative` table with
   columns `Constraint | Value | Source-anchor`, plus `### Environmental`
   and `### Architectural` prose. The `Clock frequency` row (value
   matching `\d+\s*(MHz|GHz)`) and the `Gate budget per cycle` row
   (value containing a number) are REQUIRED rows.
7. `## External Interfaces` -- per-interface `### Interface: <name>`
   subsections, each with a property block (Direction / Protocol / Clock
   domain / Connected peer), a `#### Signals` table (six columns:
   `Signal | Direction | Width | Type | Required | Description`),
   `#### Transaction semantics` prose, and a `#### Source-spec anchors`
   bullet list.
8. `## Blocks` -- per-block `### Block: <name>` subsections, each with a
   property block (Role / Parent / Clock domain / Parameterized by), a
   `#### I/O Signals` table (four columns:
   `Signal | Direction | Peer | Description`), a `#### State` bullet
   list, a `#### Behavior summary` of one to three prose paragraphs,
   a `#### Source-spec anchors` bullet list, and optional `#### Figures`
   and `#### Sub-blocks` bullets, and optional `#### Retrieval hints`
   --- a bullet list of `spec_semantic_search` queries downstream DM
   steps (DM2 / DM3) can issue when they need source-spec detail
   beyond what's already inlined. The auto-populate step seeds
   `Retrieval hints` from the block name (canonical form + acronym +
   `<name> behavior signals`); refine entries if you have better
   queries from the search hits you ran. All blocks sit at heading
   level 3 regardless of nesting; hierarchy is conveyed via the
   `Parent` property.
9. `## Parameters` (required if any parameters) -- single table:
   `Name | Type | Default | Valid range | Behavioral impact |
   Source-anchor`.
10. `## Functional Behavior` -- `### End-to-end behavior` prose,
    `### Operation flow` numbered list (each item naming a backtick-quoted
    operation id + brief purpose + anchor), `### Data movement` prose.
11. `## Timing, Latency, and Throughput` -- optional `### Latency`
    table, `### Throughput` prose, `### Stall and backpressure` prose.
12. `## Pipeline and Hierarchy` -- short prose summary that points at
    the Blocks section for detail.
13. `## Reset, Initialization, Flush, Drain` -- prose subsections
    `### Reset`, `### Initialization`, `### Flush and drain`.
14. `## Worked Examples` -- at least one `### Example N: <title>` with
    Inputs / Expected flow / Expected outputs.
15. `## Source-Spec Anchors` -- index table
    `spec.md section | Source | Chunk id | Page range` mapping each
    structured section to its supporting source-spec chunk.
16. `## Open Questions` -- bullet list of unresolved TBDs.
17. `## Auto-decisions` -- bullet list of `Decision; rationale: ...`
    entries for every non-trivial inference you made.

### Optional sections (include only when applicable)

- `## State Machines` -- per-FSM `### FSM: <name>` with States bullets
  and a Transitions table (`From | Input/Event | To | Output/Action`).
- `## Encodings` -- per-field `### Encoding: <name>` with bit width +
  `Value | Name | Abbreviation` table.
- `## Memory Map` -- `Start | End | Name | Purpose | Access |
  Source-anchor` table.
- `## Connectivity` -- `### Nodes` and `### Edges` tables plus
  `### Routing rules` prose; used for mesh / NoC / topology designs.
- `## Error Handling` -- single table
  `Error type | Detecting component | Detection behavior | Bus response
  | Master behavior | Software response | Source-anchor`.
- `## Cycle-Accurate Behavior` -- per-scenario `### Scenario: <name>`
  with a per-cycle stage table and a Source-anchor footer.
- `## Figures` -- per-figure `### Figure: <title>` with Source page /
  Raster / Role / Referenced blocks property block, a `#### Caption`
  prose paragraph, and an `#### Elements depicted` table.

If an optional section does not apply, omit it entirely. Do NOT leave
empty headings or placeholder bullets behind.

### Source-spec anchor format

Anchors are short strings keyed to chunks in the ingest manifest. Three
forms:

- `<source>:p<N>` -- single page, e.g. `primary:p13`.
- `<source>:p<N>-<M>` -- page range, e.g. `primary:p12-13`.
- `<source>:chunk-<NNN>` -- direct chunk reference; used most often in
  the `## Source-Spec Anchors` index.

`<source>` is `primary` or a peer ID registered in
`.sim-flow/spec-ingest/manifest.toml.peers[].id`. The DM0 gate checks
that every anchor in spec.md resolves to a real chunk in the manifest.

### Column-header alias rules

The parser accepts a small set of aliases (e.g. `Name` for `Signal`,
`Notes` for `Description`, `Dir` for `Direction`). Prefer the canonical
column names; alias use produces a warning at gate time.

## What you arrive to

By the time this work session starts, the orchestrator has already run
the auto-populate step (when a source spec is registered). On arrival you
will find:

- `docs/spec.md` already containing the structured skeleton. The
  REQUIRED section headings are all present. Tables that the ingest
  pipeline could populate deterministically (signal tables per block,
  parameters, encodings, FSMs, error tables, the metadata block, the
  figure index, the source-anchor index) are already filled. Anchors
  for the auto-populated rows point at the source-spec chunks they
  came from.
- An auto-populated `## Open Questions` section listing every TBD the
  ingest pipeline detected.
- Empty prose subsections (Purpose / Scope / Non-goals / per-block
  `Behavior summary` / Functional Behavior `End-to-end behavior` /
  Functional Behavior `Data movement` / `Worked Examples`) for you to
  fill.
- `.sim-flow/spec-ingest/manifest.toml` describing the corpus, plus the
  per-chunk markdown under `.sim-flow/spec-ingest/primary/chunks/` and
  any peer specs under `.sim-flow/spec-ingest/<peer-id>/chunks/`.

When there is no source spec (`manifest.toml.source_kind = "none"` or
no manifest at all), the orchestrator drives a separate interactive Q&A
loop on top of `ask_user`; you do not author from scratch turn by turn
in this work session. Your job in the no-source case is to review what
the Q&A loop produced and tidy up the prose.

## What you own this turn

Your responsibilities:

1. **Prose subsections.** Write concise normalizations of:
   - `## Purpose` (one paragraph)
   - `## Scope` (one paragraph)
   - `## Non-goals` (one paragraph)
   - Per-block `#### Behavior summary` (one to three short paragraphs
     each)
   - `## Functional Behavior > ### End-to-end behavior`
   - `## Functional Behavior > ### Data movement`
   - `## Worked Examples` (at least one representative scenario)
   - `## Timing, Latency, and Throughput > ### Throughput` and
     `### Stall and backpressure`
   - `## Reset, Initialization, Flush, Drain` subsections
   - `## Pipeline and Hierarchy`

   Each prose subsection should be one to three short paragraphs --
   conciseness matters. With retrieval available, verbose paraphrasing
   of the source spec is wasted work.

2. **Open Question resolution.** Walk the auto-populated `## Open
   Questions` list. For each TBD:
   - If the source spec actually answers it (the ingest pipeline missed
     it), retrieve the answer via `spec_semantic_search` + `read_file`
     and resolve the question: remove it from `## Open Questions` and
     update the relevant spec.md section to carry the answer.
   - If the source is genuinely silent and the question blocks forward
     progress for DM1+, call `ask_user` (see the nudge below) to ask
     the user. On a clean reply, record an Auto-decision in
     `## Auto-decisions` and remove the question from `## Open
     Questions`.
   - If the source is silent but the question is non-blocking, leave
     it in `## Open Questions` for the critique pass and downstream
     steps to handle.

3. **Auto-decisions.** Any non-trivial inference you make (default
   parameter value, peripheral interpretation, ambiguous-but-decidable
   detail) lands as an `## Auto-decisions` bullet with a one-line
   rationale.

4. **Validate the structural tables.** The agent should spot-check that
   the auto-populated signal tables and parameter tables match the
   source spec; if a mismatch is real, fix the table and record the
   change in `## Auto-decisions`.

5. **Write the result.** When the prose is complete and the TBDs are
   either resolved, auto-decided, or genuinely open, emit a single
   `write_file docs/spec.md` call carrying the full updated document.

## Tools you should reach for

### `spec_semantic_search`

Source-spec retrieval. **This is the only sanctioned entry point into
the source-spec corpus.** When you need detail beyond what the
auto-populated `docs/spec.md` carries -- prose context for a block,
the underlying explanation behind a signal, the page that originally
described a parameter -- call `spec_semantic_search` with a
natural-language query. The hit list returns `chunk_path` (a relative
path under `.sim-flow/spec-ingest/primary/chunks/` or a peer's
`chunks/`), `breadcrumb`, `section_heading`, `source_page_range`, and
a short snippet. When the snippet is insufficient, `read_file` the
returned `chunk_path` for the full body -- that is the ONLY context
in which you should `read_file` anything inside
`.sim-flow/spec-ingest/`.

Do not list `.sim-flow/spec-ingest/primary/chunks/` or fabricate
chunk paths. If `spec_semantic_search` returns no useful hits,
refine the query rather than browsing the tree.

Each hit also returns `contained_signal_tables` and
`contained_figures` so you can pivot to the structured artifact
without a second search.

spec.md is the normalized truth for the design; the source spec is the
underlying material. Use `spec_semantic_search` when spec.md is too
brief; otherwise prefer what's already in spec.md.

### `signal_table_query`

Structured query over the project's signal-table rows (both
source-spec rows and the rows already in spec.md). Use this to:

- Enumerate the I/O for a stage / block: `filter = { stage = "<block
  name>" }`.
- Look up a specific signal across blocks: `filter = { signal_name =
  "<name>" }`.
- After you edit a signal row in spec.md, set
  `conflicts_only = true` to verify your edits still match the source
  spec on direction, peer, and meaning.

### `ask_user`

The user-interaction tool. Use it ONLY for blocking unknowns -- a TBD
that prevents you from finishing a required section, a design choice
the spec doesn't make for you, or an ambiguity `spec_semantic_search`
cannot resolve. Do NOT use it for retrievable information; do NOT use
it for casual confirmation.

Turn-boundary discipline (Architecture Chapter 6 §6.5.1):

- Call `ask_user` as the LAST tool call of the turn. Complete every
  other useful operation you can in the same turn first (additional
  reads, partial spec.md writes, Auto-decision drafting). The
  orchestrator suspends execution after `ask_user`; later tool calls
  in the same turn are discarded and produce a regression warning.
- The user's reply arrives as the tool result on the next turn.

Chaining for ambiguous replies (Architecture Chapter 4 §4.5):

- The first `ask_user` call in a thread omits `thread_id`; the
  orchestrator generates one and returns it on the answer.
- Use `record_as = "none"` on this first call -- persistence is
  deferred to the closing call of the thread.
- If the reply is partial, ambiguous, or itself a question, emit a
  follow-up `ask_user` with the SAME `thread_id`, again with
  `record_as = "none"`, and ask a more focused clarification.
- Once you have a clean answer, close the thread with a final
  `ask_user` call that carries the same `thread_id` plus the
  persistable `record_as` -- for DM0, this is almost always
  `"auto-decision"` (every design choice you commit to is by
  definition a persistable decision). For a non-answer that should
  remain open, close with `record_as = "open-question"` marked
  unresolved.
- Do NOT chain indefinitely. Target three exchanges or fewer per
  thread; the orchestrator warns at five.

Mode-flip note: if you invoke `ask_user` while the run is in
automated mode, the orchestrator flips the run to manual for the rest
of the session. This is intentional -- once a human is needed,
automated mode no longer applies. The `mode_changed` field on the
returned `AskUserAnswer` signals the flip; subsequent turns proceed
in manual mode.

## Procedure

1. Read `docs/spec.md` and skim every auto-populated section so you
   know the shape of the structured skeleton. This is your starting
   point -- the auto-populate step has already pulled the
   deterministic content out of the source spec for you.
2. Read `.sim-flow/spec-ingest/manifest.toml` (if present) to learn
   the source-spec inventory. All chunk content is fetched via
   `spec_semantic_search`; do not read corpus files directly.
3. For each empty prose subsection you own, run a focused
   `spec_semantic_search` for the relevant material, `read_file` the
   `chunk_path`(s) returned by that search when the snippet is too
   thin, and write a concise normalization in spec.md. Do not
   bulk-quote the source -- normalize and anchor.
4. For each entry in `## Open Questions`:
   - Search for an answer in the source via
     `spec_semantic_search`.
   - If found, resolve and remove from Open Questions; update the
     relevant section.
   - If not found but blocking, `ask_user` (last tool call of the
     turn) and close the thread with `record_as = "auto-decision"`.
   - If not found and non-blocking, leave for the critique pass.
5. Spot-check signal tables with `signal_table_query` (especially
   `conflicts_only = true`) and reconcile any discrepancies via
   Auto-decisions or table edits.
6. When the prose is complete and TBDs are resolved / auto-decided /
   recorded, write the final `docs/spec.md` via a single `write_file`
   call.

## Output

{{ output_intro }}

- `docs/spec.md` at the project root, updated with the completed
  prose, resolved Open Questions, and recorded Auto-decisions. The
  document must conform to the schema described above (REQUIRED
  sections present, signal-table column conventions respected,
  source-spec anchors resolvable, `Clock frequency` and `Gate budget
  per cycle` rows present in the Quantitative table).

When the artifact above is complete, stop. Do not write
`docs/critiques/DM0-critique.json` (the critique is a distinct task)
and do not `/exit` on your own -- the user and the orchestrator
control session boundaries.
