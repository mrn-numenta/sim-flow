# Incorporating LanceDB + Rig into sim-flow — Where they help, what not to do

**Status:** brainstorm. Explores needs and possibilities; no
architecture or implementation plan in this doc — those land
separately once we settle on a direction.
**Date:** 2026-05-17 (revised after four-spec review and image
extraction assessment).
**Reads from:** [dmf-llm-critique-rig-lancedb-2026-05-17.md](dmf-llm-critique-rig-lancedb-2026-05-17.md),
[rust-analyzer-lsp-discovery.md](rust-analyzer-lsp-discovery.md),
[model-robustness-study.md](model-robustness-study.md).
**Sibling brainstorms:** [spec-ingest-figure-extraction.md](spec-ingest-figure-extraction.md)
(figure-content recovery from PDF specs),
[spec-md-restructure.md](spec-md-restructure.md) (spec.md as
structured-artifact-bearing IR + L2/L7 retrieval).

## 1. What this doc is and isn't

The earlier "Rig + LanceDB as foundation?" doc already settled the
big question: **no, not as a foundation; yes, LanceDB on its own
for RAG; Rig optionally as a transport.** This doc is the
follow-up — given everything that's landed since (live LSP tools,
Phase 0d fence-fix, native tool calls, the Phase 0 anomaly
catalog), what's the concrete first slice, and what stays off the
table?

## 2. What's changed since the prior brainstorm

Things that are already shipped now that the prior doc proposed
or assumed away:

- **Live framework discovery via LSP.** [`api_search` / `api_hover` /
  `api_impls` / `api_references` /
  `api_expand_macro`](../../src/__internal/session/tools/mod.rs) are
  wired in the universal tool catalog. The agent no longer
  navigates the 864-page rustdoc snapshot by intuition — it
  queries rust-analyzer directly. **The "force RAG over `fw:api`"
  proposal needs re-scoping: it now competes with / complements
  live LSP, not the static markdown.**
- **Native tool calls landed.** Tool dispatch is structurally
  typed; the fenced-block path is the fallback. The Phase 0d
  fence-fix dropped wrong-fence-info-string from ~92% of
  affected trials to ~33%.
- **Two-cap critique policy + work-side cap bump landed.** The
  dominant terminator shifted from `critique-iter-cap` to
  `work-no-artifact` (the empty-turn stall) and then partly back
  out after the fence fix.
- **None of this addresses the original DM2d invented-API
  failure** (rgb_toy: model fabricated `take_input` because it
  had a plausible prior and no retrieval check forced it to look
  things up). LSP tools help only when the model knows the right
  symbol name to query. When it doesn't — when it's reaching for
  "the function that consumes one element from an input port and
  returns `Option<T>`" — there's still no recovery path.

### 2.1 Evidence from four real specs

Reviewed against four representative spec PDFs (Apical NoC 17pp,
Numenta SoC 38pp, Spatial Pooler 2.1 10pp, RV12 RISC-V CPU Core
95pp). Findings that change the plan:

- **Page-fragmentation is empirically severe, not theoretical.**
  Apical NoC has parameter tables sliced across p3→p4. Numenta SoC
  has the error-handling table across p28→p29. **RV12 splits every
  pipeline-stage signal table across page boundaries (IF p13→p14,
  PD p15→p16, ID p17→p18).** Page-based chunking is a structural
  cliff for hardware specs.
- **Stub sections are real and unhandled.** Numenta SoC has 8
  heading-only placeholders (HTM, CPU, Memory System, NoC, Memory
  Map, Boot/Reset, Clock, Debug, Register Definition, SW Flow).
  Today's pipeline ingests these as 0-byte pages; the agent has no
  signal that "this section needs to be filled in."
- **Cross-spec inheritance is a real pattern.** Spatial Pooler 2.1
  explicitly says "Hardware elements... are inherited directly
  from the TM spec." Sim-flow today assumes one source spec; SP
  without TM is unintelligible.
- **Figures carry information the prose does not.** RV12's IF
  block diagram (p13) shows a 4:1 next-PC mux selecting `pc+2`/
  `pc+4`/`ex_nxt_pc`/`st_nxt_pc` — the prose only says "the PC is
  restarted." The EX diagram (p20) is the entire operand-bypass
  network (`wb_r`/`mem_r`/`ex_r` forwarding into ALU/Mul/Div/LSU/
  Branch); none of the bypass topology appears in prose. **A
  text-only ingest pipeline cannot author a faithful model from
  these specs.**
- **HTML-origin PDFs carry per-page chrome.** RV12 prints from
  `roalogic.github.io/RV12/DATASHEET.html`; every page has a
  repeated header banner, URL footer, and `Page N of 95` footer.
  Today's pdfium extraction includes all of it as page text.
- **Signal-tables are the highest-value structured artifact for
  hardware specs.** Every RV12 pipeline stage follows the same
  template: prose → block diagram → `Signal / Direction / To-From
  / Description` table. The table IS the inter-stage interface
  contract — much more concrete than the prior "structured
  artifacts" recommendation.
- **Repeated headers under different parents.** RV12 has
  "Instruction Fetch (IF)" under both `Introduction to the RV12 >
  Execution Pipeline` (overview) AND `RV12 Execution Pipeline`
  (deep dive). Leaf-only chunk labels conflate them; breadcrumb
  paths are essential.

## 3. What lancedb and rig actually are (load-bearing facts)

### LanceDB

- Apache-2.0, embedded library — no server, opens a directory.
- Lance columnar (Arrow-based) format with versioned manifests,
  IVF_PQ / IVF_FLAT / HNSW vector indexes plus BTree/scalar.
  Hybrid (vector + filter) queries supported.
- **Bring-your-own-vectors in Rust.** The Python SDK has an
  embedder registry; the Rust crate does not. The caller computes
  the vectors and inserts rows.
- No built-in chunker / text splitter / embedder. No auth, no
  server-side query planner beyond filter + ANN.
- Indexing 100k chunks of ~500 tokens is well below scale where
  docs publish numbers — local-SSD ANN queries at this size are
  routinely sub-10ms but not officially quoted.

### Rig

- MIT, currently 0.37; README warns "future updates will contain
  breaking changes."
- Two layers: low-level provider `Client` / `CompletionModel` /
  `EmbeddingModel`, **plus** an `Agent` builder with preamble +
  tools + RAG. **You can use the provider layer without the
  Agent.**
- ~24 provider modules including Anthropic, OpenAI, Cohere,
  Voyage, Together, Ollama, llamafile. No dedicated `vllm` or
  `openai_compat` module — vLLM works by pointing the `openai`
  client at a custom base URL.
- Has streaming, tool calls, structured-output extractor.
- Vector-store integrations include a companion `rig-lancedb`
  crate.
- **Async (tokio).** sim-flow today is sync (`ureq`-based HTTP);
  adopting rig anywhere in the agent dispatcher pulls tokio into
  that path.
- **Prompt caching and extended thinking are not documented in
  the rig public surface.** We use both today (Anthropic prompt
  caching, `enable_thinking=false` for vLLM). Any rig-as-transport
  decision has to re-verify these features before swapping.
- Cannot drive subprocess CLI agents (Claude Code, codex,
  copilot). HTTP-only.

## 4. Candidate uses, scored

Each candidate scored on **leverage** (does it move the needle on
an observed failure mode?) and **cost** (effort, risk,
dependency footprint). High-leverage / low-cost goes first.

### 4.0 Spec-ingest pre-step (no lance, no rig)

These are spec-side fixes that need no embeddings or vector store
— they are textual / structural transformations on
`spec_ingest.rs`'s output. They land **before** any lance work
because they raise the value of every downstream RAG candidate
and they fix problems that exist even without RAG. Sized to be
small, mostly-pure Rust changes.

| # | Pre-step                                                                  | Leverage | Cost | Take                                                                                                                                                                            |
| - | ------------------------------------------------------------------------- | -------- | ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| L0a | **Section-based re-chunking.** Honor markdown headings rather than PDF pages. Reassemble tables split by page boundaries. Emit `{section_path, breadcrumb, kind: prose / table / stub, body}`. | High | Low | Mandatory for any spec > ~20 pages. Without it, RV12-class specs lose every signal table. |
| L0b | **Page-chrome stripping.** Pre-pass that detects repeated header / footer / "Page N of M" lines across pages and removes them before chunking. | Medium | Low | RV12-class HTML-printed PDFs leak chrome into every chunk otherwise. Trivial heuristic (line appears on >50% of pages). |
| L0c | **Hierarchical breadcrumb paths.** Each chunk carries its full ancestor chain (`Introduction > Execution Pipeline > Instruction Fetch (IF)`), not just the leaf header. | High | Low | Disambiguates repeated section names; gives the agent context without re-reading parents. |
| L0d | **Stub-section flagging.** A `# Section Title` followed by no body (or only "TBD") emits `{kind: stub, breadcrumb: ...}`. Surfaces "this section needs to be filled in" as a structured signal. | Medium | Low | Numenta-SoC-class outline-specs become navigable. The agent (or a Q&A turn) can enumerate stubs and ask the user. |
| L0e | **TBD detection.** Scan all chunks for the literal `TBD` token (with breadcrumb context) and emit a structured list. | Medium | Low | Bypasses LLM cost on the discovery half: a deterministic pre-pass produces the list of open questions. DM0 turns this into either user questions (manual) or auto-decisions (auto). |
| L0f | **Signal-table extraction.** Recognize the `Signal / Direction / To-From / Description` table pattern, reassemble across pages, emit as `{kind: signal_table, stage: "<breadcrumb>", rows: [...]}`. | High | Medium | Highest-value structured artifact for hardware specs. Per-stage I/O contracts that DM1 / DM2 can consume directly without LLM re-parsing. RV12 has six of these (one per pipeline stage). |
| L0g | **Cross-spec reference parsing.** Parse explicit "see X spec, section Y" patterns into a structured reference list. Initial form: a `## References` section + linked-doc registration. | Medium | Medium | Enables SP→TM-style multi-document specs. Modest scope — just registering peer specs so downstream tools (and L2 RAG) can include them in search. |

### 4.1 LanceDB candidates

| # | Use case                                              | Leverage | Cost   | Take                                                                                                                                                                                                |
| - | ----------------------------------------------------- | -------- | ------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| L1 | **Semantic search over `fw:api` + `fw:src`** as a NEW tool alongside existing LSP `api_*` tools. | High   | Medium | Direct fix for the rgb_toy DM2d invented-API failure. Complements LSP — LSP answers "I know the name, tell me the truth"; lance answers "I don't know the name, find me candidates." See §5. |
| L2 | **Semantic search over source spec pages** (post-L0 section chunks) per step. | **High** | Low | Re-weighted from Medium → High after the four-spec review. RV12 (95pp) and Numenta SoC (38pp) make "agent reads TOC, guesses at relevant pages" untenable. With L0a/c chunks (sections + breadcrumbs), this is a small step on top of the L1 pipeline. The bigger win is for large specs where prose, signal tables, and figures cluster around specific concerns — `api_spec_search("next-PC mux inputs")` should hit `RV12 > Execution Pipeline > Instruction Fetch (IF)`. |
| L3 | **"Previously rejected" pile per step** as a lance table.                          | Medium | Medium | Worth doing eventually, but volume is small (tens-to-hundreds of entries per project). For that N, flat JSONL + substring/regex is sufficient. Lance only justified once we want semantic similarity ("a prior session was rejected for a similar architectural shape"). Defer. |
| L4 | **Replay-corpus indexing** for the robustness study.                                | Low      | Medium | Nice for cross-run "find similar failures" research tooling. Not a production path. Out of scope here.                                                                                              |
| L5 | **Project-source index** so `edit_file` doesn't drift.                              | Low      | High   | The mental-model-drift problem is solved by reading current bytes, not by semantic similarity. `read_file` already returns truth. Adding a Lance layer over project source is overkill and stale-prone. **Don't.** |
| L6 | **Cross-spec / referenced-spec ingestion + retrieval.** Index registered peer specs alongside the primary, and let L2 queries span both. | Medium-High | Medium | Direct fix for SP→TM-style inheritance. Requires L0g (reference parsing). Search scope is `{primary_spec, ...peer_specs}` with per-doc filters. Without it, multi-doc specs are unhandled. |
| L7 | **Signal-table index as structured rows, not vector.** Index L0f's extracted signal-table rows in a non-vector Lance table with `{stage, signal_name, direction, peer, description}` columns. Hybrid query: exact match on `signal_name`, scalar filter on `stage`. | Medium | Low | Once L0f exists, this is almost free. Lets the agent ask "show me every signal driven from PD" or "where is `ex_nxt_pc` consumed?" without LLM-parsing tables. Distinct from vector RAG — this is structured retrieval over typed rows. |

### 4.2 Rig candidates

| # | Use case                                                                | Leverage | Cost   | Take                                                                                                                                                                                       |
| - | ----------------------------------------------------------------------- | -------- | ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| R1 | **Use rig as an embedding client only** to feed the L1 / L2 lance pipeline. | High | Low | Rig's `EmbeddingModel` trait covers ~24 providers including Voyage, OpenAI, Cohere, and local (Ollama). Single async POST per chunk; we already need an embedder. This is the natural seam. **Yes.** |
| R2 | **Use rig's `openai` provider for vLLM** (replace our OpenAI-compat client). | Low | High | rig's "openai with custom base URL" works, but our current client carries vLLM-specific quirks (`chat_template_kwargs.enable_thinking`, `seed`, `prefers_bare_json_critique` family handling). Rewriting on rig means re-implementing each adaptation hook. Async-conversion cost is real. Marginal benefit. **Skip.** |
| R3 | **Use rig's anthropic provider** (replace `AnthropicAgent`).            | Low      | High   | Rig doesn't document prompt caching or extended thinking — both features we use. Swap-out risk too high for what's effectively a tidy-up. **Skip.** |
| R4 | **Adopt rig's `Agent`** as a higher-level orchestrator.                   | Negative | High   | Already analyzed in the prior doc. Conflicts with state machine, gates, per-step write scoping, protocol-event wire format, subprocess CLI clients. Adopting `Agent` is rebuilding sim-flow on a thinner base. **Don't.** |
| R5 | **Use rig's `extractor` for structured output** (critique JSON parsing). | Low      | Low    | Plausible cleanup, but our current salvage path + the planned `try_repair_json` covers this. Pulling in rig only for `extractor` isn't worth the dep. **Skip for now.** |
| R6 | **Opportunistic: when adding a brand-new HTTP backend** (Gemini, etc.), reach for the matching `rig::providers::*` module rather than writing one from scratch. | Variable | Low | Only relevant the day we want a new backend. Don't preemptively pull in rig; do reach for it when the time comes. |

## 5. Recommended first slice — end-to-end RAG, one tool

Build the smallest piece that exercises every layer of the stack
and addresses one observed failure mode (invented-API in DM2d):

### Scope

1. **Schema.** Single Lance table `framework_chunks`:
   ```
   id            string  (path::symbol or path::chunk-N)
   source_path   string  ("fw:api/pages/.../*.md" or "fw:src/.../*.rs")
   kind          string  ("api-page" | "src-fn" | "src-impl" | "src-trait" | "src-other")
   name          string  (symbol name when known)
   text          string  (the chunked body)
   vector        FixedSizeList<Float32, DIM>
   ```
2. **Chunking.** Two passes:
   - `fw:api/pages/*.md` → one chunk per page (already curated, already short).
   - `fw:src/**/*.rs` → one chunk per top-level item (function, impl
     block, trait def, module doc-comment). Use `syn` for parsing.
3. **Embedding.** Use `rig::providers::voyage_ai` or
   `providers::openai` (`text-embedding-3-small`) as the embedder.
   Choose at index-build time; record the model in a manifest so
   queries use the same one. Stay behind rig's `EmbeddingModel`
   trait so swapping is one-line.
4. **Index build CLI.** `sim-flow build-api-index --out
   <foundation-root>/.sim-flow/api-index.lance`. Build once per
   framework version (gated on `Cargo.lock` hash + crate version
   stamps); reusable across projects.
5. **Tool.** New `api_semantic_search(query: str, k: int = 8) ->
   [{path, name, snippet, score}]` wired into the universal tool
   catalog alongside the existing LSP tools.
6. **Prompt nudge.** DM2d / DM3b / DM3c work-prompts gain one
   short paragraph: "When you don't already know the framework
   symbol name, call `api_semantic_search` first. THEN call
   `api_hover` on each candidate to verify the live signature
   before writing. Do NOT write against a signature you haven't
   `api_hover`ed."

### Why this is the right first slice

- **Hits one observed failure mode head-on** (invented-API in DM2d).
- **Doesn't fight the existing tool catalog** — adds one tool
  next to five working ones.
- **Touches every layer exactly once**: chunker, embedder
  (via rig), lance store, query tool, prompt integration.
  Anything we get wrong shows up immediately.
- **Replay-testable.** Re-run the rgb_toy DM2d capture in
  `MockAgent::from_corpus` mode with `api_semantic_search`
  available; check whether the agent would have caught its own
  fabrication.
- **No subprocess CLI impact** — the existing Claude Code /
  codex / copilot paths still work unchanged.
- **Async leakage is bounded.** Only the index-build CLI and the
  `api_semantic_search` tool body touch tokio. The orchestrator's
  main loop stays sync; the tool's `dispatch` returns a result
  synchronously from the caller's perspective.

### What to measure

- **Wrong-API-invention rate.** Before/after on rgb_toy DM2d
  replay. The capture format from the robustness study already
  records every tool call and assistant turn; we read it.
- **Query latency.** End-to-end embed + ANN per call. Target
  <200ms at p95 against a local lance directory of ~5k chunks.
- **Index build time + size.** One-time cost per framework
  version. Should be minutes, not hours; tens of MB on disk.
- **`api_semantic_search` → `api_hover` follow-through rate.**
  How often does the agent verify a candidate before writing?
  If this is low, the prompt nudge needs tightening; if it's
  zero, the tool isn't earning its keep.

## 6. Explicit DON'Ts

These would each be plausible at first glance; flagged so we don't
slide into them.

1. **Don't replace the HTTP transport (ureq → rig) for Anthropic
   or vLLM.** Rig doesn't document prompt caching or extended
   thinking. Our adaptation layer carries vLLM-specific quirks
   that would need re-implementing. Async-conversion cost is
   real. The marginal benefit (less hand-rolled HTTP code) is
   not worth the risk.
2. **Don't adopt rig's `Agent`.** Already analyzed; collides with
   state machine, gates, per-step scoping, protocol-event wire
   format, subprocess CLI clients.
3. **Don't replace `fw:api` static pages with lance.** The
   curated TOC is editorial scaffolding that semantic search
   can't recreate, and LSP `api_hover` is the precise-answer
   path. Lance is the third path — "candidate discovery" —
   not a replacement.
4. **Don't build the critique-history pile in lance yet.**
   Volume is small. Flat JSONL with substring search lands the
   feature; switch to lance only if/when we want semantic
   similarity over rejected approaches.
5. **Don't index every project's `spec-pages` until we measure
   value from L1.** The chunking / embedding pipeline is shared,
   so it's cheap to add later; don't speculatively index now.
6. **Don't ship lance / rig in the bundled extension.** Both
   live in `sim-flow`-the-orchestrator-crate. The VS Code
   extension stays a thin renderer.
7. **Don't run the index-build inside the orchestrator session.**
   Build once via CLI, reuse across sessions. A multi-minute
   embedding pass inside an interactive flow is a UX cliff.
8. **Don't tie the index to a specific embedding provider in
   code.** Store the model name + dimension in the manifest;
   require queries to use the same model. Lets us swap embedders
   without re-coding callers.
9. **Don't preemptively migrate to rig for "future backend
   abstraction."** The day we need a new backend, reach for
   `rig::providers::<thing>` for that backend only. Until then,
   our adaptation layer is richer than rig's provider abstraction
   in the ways we care about (per-family normalizers, thinking
   controls, prompt cache awareness).
10. **Don't conflate "figure content" with "figure RAG."** The
    figure-content problem splits into two halves: (a) extracting
    a faithful figure from the source PDF, and (b) turning that
    figure into structured text the agent can use. Hand-review of
    the current `spec_ingest.rs` output on RV12 shows that **half
    (a) is currently broken on the most important class of
    figures** (composite raster-sheet + vector text overlay — every
    per-stage RV12 diagram). That's a `spec_ingest.rs` correctness
    fix (page-region rasterization vs embedded-image extraction),
    not a lance/rig concern. Once half (a) is fixed, half (b)
    (captions) becomes a real option to evaluate with vision models
    or author-supplied captions — and the resulting caption text
    CAN be embedded into L2. **Track the extraction half in
    [spec-ingest-figure-extraction.md](spec-ingest-figure-extraction.md);
    track the captioning half once that lands. Don't paper over
    extraction failures with text RAG.**
11. **Don't run L0 transformations inside the orchestrator
    session.** Section chunking, page-chrome stripping, signal-table
    extraction belong in `spec_ingest.rs` (or a successor) at PDF
    ingest time. The agent should see already-chunked, already-
    cleaned input — not raw PDF text and not re-do the chunking
    every session.

## 7. Open questions to resolve before coding

1. **Embedder choice.** Voyage 3 (purpose-built for code), OpenAI
   `text-embedding-3-small` (cheap, broad), or local (Ollama with
   `nomic-embed-text`)? Local is friction-free and offline; hosted
   gives better recall on code-specific queries. Pick one to start,
   keep the manifest open.
2. **Index freshness.** Re-build on every `Cargo.lock` change in
   `crates/framework`? On every release-tagged version? Or on
   demand via a `sim-flow rebuild-api-index` operator command?
3. **Per-project vs shared index.** The framework is the same
   across projects; one shared `~/.sim-flow/api-index/` is the
   obvious choice. Confirm no per-project re-indexing happens
   accidentally.
4. **Hybrid query.** Lance supports `vector + filter` queries.
   Should `api_semantic_search` accept a `kind` filter
   ("only `src-fn`, not `api-page`")? Probably yes, but the
   default should be unfiltered — let the agent narrow.
5. **Snippet size returned to the agent.** Returning full chunks
   blows tokens. Returning only the first N chars loses context.
   Try first paragraph + signature line; iterate.
6. **Interaction with `read_file`.** When the agent gets a hit,
   should it `read_file fw:src/...` to see the full body, or
   trust the snippet? Probably "read full body before writing
   against the signature" — the prompt should say so.
7. **`api_hover` integration.** Should `api_semantic_search`
   results include a hint like "next: call api_hover with
   `<symbol>` to verify"? Or rely on the prompt to enforce
   that step? Lean toward including the hint in the tool's
   structured response so the model doesn't have to remember.
8. **Failure mode of the LSP path.** rust-analyzer cold-start
   is 2–3 min on the framework workspace. While LSP is cold,
   `api_semantic_search` is the only working discovery path —
   document that, and make sure `api_hover` waits for LSP
   ready rather than racing.
9. **L0 ordering vs. L1 first slice.** L0a (section chunking) +
   L0c (breadcrumbs) are arguably prerequisites for L2 (spec RAG)
   but not for L1 (framework RAG, which has its own chunker over
   `fw:api` + `fw:src`). Do we land L0 before L1 to validate the
   chunker once, or land L1 first to validate the lance pipeline
   and back-fill L0 for L2? Lean: L0a+c+b first (no lance, fast
   feedback), then L1, then L0d–g, then L2/L6/L7.
10. **Signal-table schema.** L0f / L7 imply a stable schema for
    extracted signal-tables. Initial sketch: `{stage_breadcrumb,
    signal_name, direction: "in"|"out", peer: "<stage> | <bus>",
    description, source_chunk_id}`. Confirm against more
    pipeline-stage tables (Apical NoC engine interface, RV12
    MEM/WB if their tables match the IF/PD/ID/EX pattern).
11. **Adversarial / partial specs.** Numenta SoC has stub
    sections that are intentionally TODO. When the agent
    encounters a stub via L0d, what does DM0 do — ask the user
    one question per stub (manual mode), record auto-decisions
    (automated mode), or refuse to advance? Default behavior
    needs writing down.
12. **Page-chrome heuristic tuning.** L0b — "line appears on
    >50% of pages" misses chrome that only appears on the first /
    last page, and might over-strip if a recurring block-quote
    legitimately repeats. Measure on the four sample specs;
    likely needs a "page header / footer region" detector that
    looks at vertical position too, not just text repetition.

## 8. Bottom line

The prior brainstorm's verdict stands: **LanceDB is the
high-leverage piece; Rig is at most a transport.** After the
four-spec review and the image-extraction assessment:

1. **L0 (spec-side ingest pre-step) is mandatory and lance-free.**
   Section chunking + page-chrome stripping + breadcrumb paths +
   stub flagging + TBD detection + signal-table extraction are
   textual / structural fixes that should land in `spec_ingest.rs`
   before any RAG work. They unblock RV12-class large specs and
   Numenta-SoC-class stub-heavy specs regardless of whether we
   adopt lance.
2. **L1 (framework RAG) remains the first lance slice** — it
   addresses a measured, observed failure mode (invented-API
   in DM2d) independent of spec quality.
3. **L2 / L6 / L7 (spec RAG + cross-spec + signal-table index)
   are the high-value extensions** once L0 lands. The four-spec
   review re-weighted L2 from Medium to High leverage; signal-table
   extraction (L7) is the new entry that wasn't in any prior
   brainstorm.
4. **Figure handling splits in two.** The extraction half is a
   `spec_ingest.rs` correctness problem (the current pipeline
   silently drops labels and wires on a known class of composite
   figures). The captioning half is a separate problem with
   multiple plausible paths. Both are tracked in
   [spec-ingest-figure-extraction.md](spec-ingest-figure-extraction.md),
   independent of the lance/rig direction.
5. **spec.md becomes structured-artifact-bearing.** The chosen
   direction is B + L2 + L7: spec.md carries per-block signal
   tables, FSM tables, parameter tables, encoding tables, and
   source-spec anchors that DM1+ can use as typed contracts. L2
   serves the source-spec long tail; L7 indexes the structured
   tables for direct query. Explored in
   [spec-md-restructure.md](spec-md-restructure.md).

What this brainstorm collection establishes:

- The **needs** sim-flow has across spec ingestion, framework
  discovery, retrieval, and structured artifacts.
- The **possibilities** for each — what lance gives us, what rig
  gives us, what stays sim-flow's, what doesn't fit either.
- The **failure modes** observed (invented-API in DM2d, lost
  figure content, page-fragmented tables, prose duplication of
  source specs) and which candidates address which.

What this brainstorm collection deliberately does NOT do:

- Pick concrete schemas (signal-table fields, caption shape,
  spec.md section IDs).
- Specify the sequencing of work in calendar terms.
- Define gate-check or critique changes.
- Choose specific embedders, DPI settings, captioning paths.

Those are architecture-doc and implementation-plan questions.
This collection feeds those documents but should not pre-empt
them.
