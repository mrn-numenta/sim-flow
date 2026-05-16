# rust-analyzer LSP as a code-discovery backend (scoping doc)

**Status:** draft / scoping. No code changes yet.
**Created:** 2026-05-15
**Owner:** mneilly@numenta.com
**Motivation:** today sim-flow points the agent at a static
`foundation-docs/api/` corpus -- 864 markdown pages generated
from rustdoc plus a curated `toc.md` -- so it can explore the
sim-foundation API while writing code. The corpus works, but
it goes stale on every framework change, can't answer
"who calls this?" or "what implements this trait?", and
cannot show what derive macros actually expand to. This doc
scopes whether (and how) to replace or augment the corpus
with live queries against `rust-analyzer` over LSP.

This doc does **not** propose code changes -- the goal is to
align on which discovery questions we want answered, which
LSP capabilities map onto them, and the integration shape
before any wiring lands.

---

## 1. Where we are today

- `tools/sim-flow/extensions/sim-flow-vscode/foundation-docs/api/toc.md`
  is the agent's entry point: 34 lines of curated "start here"
  pointers into `fw:api/pages/.../*.md`.
- 864 normalized markdown pages live under
  `foundation-docs/api/pages/`, generated from rustdoc and
  reshaped for LLM consumption.
- The DM2d prompt (`prompts/dm2d-model-implementation.md:34-45`)
  tells the agent: read `fw:api/toc.md` first, fetch only the
  one or two `fw:api/pages/...` files it needs, fall back to
  `fw:src/...` for exact signatures, and stay out of internal
  helper modules.
- No LSP client lives in `src/` -- this would be greenfield
  inside sim-flow.

The corpus's job is to answer "how do I express this in the
framework?" in narrow, paged chunks -- it is not the agent's
plan-of-record.

---

## 2. What the static corpus does well

Worth naming up front, because any LSP-based replacement
that drops these regresses the workflow:

- **Curated narrative.** `toc.md` lists "recommended starting
  points" -- prelude, model/dataflow, hierarchy, runtime,
  uvm, topology, observability. That ordering is editorial,
  not derivable from the code.
- **Position-free reads.** The agent says "show me docs for
  `HasLogic`" without owning a buffer open on the symbol.
- **Build-free, deterministic snapshots.** No cargo, no
  indexing wait, identical bytes across sessions. Good for
  prompt caching and reproducibility.
- **Macro derives are listed** as first-class items
  (`derive.HasLogic.md`, etc.).

---

## 3. What the static corpus cannot do

| Discovery need | Today | Why rustdoc can't fix it |
|---|---|---|
| Current signature after a refactor | Stale until regen | Snapshot, not live |
| "What types implement trait `T`?" | Not in the docs | Rustdoc doesn't index impls cross-crate this way |
| "Who calls this function?" | Not in the docs | Not a rustdoc concept |
| `#[derive(HasLogic)]` expansion | Not knowable from docs | Macros are expanded by the compiler, not rustdoc |
| Methods available on a value of type `T` | Read trait + impl pages and join in your head | No type-at-cursor concept |
| Why a trait bound fails here | Not knowable | Needs the type checker |

The first two rows are the ones that bite us most in DM2d --
the agent writes against a signature that changed, or asks
"what implements `HasLogic`?" and has to grep for it.

---

## 4. What rust-analyzer adds, mapped to discovery needs

Reviewed against
`https://rust-analyzer.github.io/book/contributing/lsp-extensions.html`.

### High value, standard LSP

- `workspace/symbol` with rust-analyzer's
  `workspaceSymbolScopeKindFiltering` (scope = workspace,
  kind = types) -- the direct replacement for "search the
  TOC."
- `textDocument/hover` -- signature plus rustdoc comment in
  one response. This is the direct analogue of reading an
  `api/pages/*.md` file, and it is always live.
- `textDocument/definition`, `textDocument/typeDefinition`,
  `textDocument/implementation` -- jump to the source or to
  every impl of a trait.
- `textDocument/references` -- find usages. Lets the agent
  see how `LaneCtx` is actually consumed by library models
  before writing its own.
- `textDocument/completion` on a positioned cursor --
  enumerates methods/fields on a value without page-flipping.
- `textDocument/signatureHelp` -- argument hints while the
  agent is drafting a call.
- `callHierarchy/incomingCalls` / `outgoingCalls`,
  `typeHierarchy` -- structural navigation.

### High value, rust-analyzer-specific

- `rust-analyzer/expandMacro` -- biggest single win for
  sim-foundation. `HasLogic`, `HasInstances`, `ConfigModel`,
  `CheckpointModel`, `SignalTracePayload`,
  `SignalTraceState` are all derive macros, and the markdown
  cannot show what they generate.
- `experimental/parentModule`, `experimental/runnables` --
  module navigation and test/binary discovery.
- `rust-analyzer/getFailedObligations` -- trait-bound
  diagnostics at a position. Useful right after the agent
  writes a `impl HasLogic for ...`.

### Lower value (debugging, not discovery)

- `rust-analyzer/viewHir`, `viewMir`,
  `viewRecursiveMemoryLayout`, `viewSyntaxTree` -- likely
  overkill for the agent's task.
- `experimental/ssr` -- interesting but niche.
- `rust-analyzer/viewCrateGraph`, `fetchDependencyList` --
  maybe useful once for orientation, not per-task.

---

## 5. Practical trade-offs to plan for

- **Cold index.** rust-analyzer needs an initial workspace
  index. Cold start is seconds to minutes on a large
  workspace; subsequent queries are fast. We pay that cost
  per session, not per query.
- **Workspace must check.** Names resolve through the type
  checker, so a broken workspace degrades answer quality.
  The agent is often editing code mid-flight -- we need to
  decide whether to re-index aggressively or let stale
  answers stand briefly.
- **Position-anchored API.** Most LSP requests want
  `(file, position)`. The agent's mental model is "symbol
  by name." Bridge with `workspace/symbol` -> first hit ->
  `hover`/`definition`, or fabricate a scratch buffer for
  hover-on-name.
- **No editorial layer.** rust-analyzer answers questions
  precisely but won't tell the agent "start at prelude."
  Whatever replaces the corpus has to keep that scaffolding.
- **Determinism.** Live answers change as the workspace
  changes -- harder to cache prompts, harder to reproduce a
  failed run. The static corpus's determinism is a feature
  for the model-robustness work, not just an accident.

---

## 6. Integration shapes

### Option A -- MCP tool wrapper over a rust-analyzer subprocess

sim-flow spawns one `rust-analyzer` per session, exposes a
small set of MCP tools the agent already understands:

- `api_search(name) -> [{symbol, kind, path}]`
- `api_hover(symbol) -> {signature, docs}`
- `api_impls(trait) -> [type]`
- `api_references(symbol) -> [location]`
- `api_expand_macro(path) -> string`
- `api_failed_obligations(file, position) -> [obligation]`

Each tool wraps one or two LSP calls under the hood
(typically `workspace/symbol` -> `hover`/`definition`).
Lowest prompt churn -- the agent learns a flat tool surface,
not LSP's positional model. Highest implementation cost.

### Option B -- Raw LSP tools, no wrapper

Expose `lsp_hover`, `lsp_definition`, etc. directly. More
flexible, more tokens spent on position juggling, and the
agent has to learn LSP's positional model. Probably worse
for smaller open models given the robustness study.

### Option C -- Hybrid (keep TOC, replace deep reads)

Keep `toc.md` and the "recommended starting points" as
narrative scaffolding. Retire the 864 `pages/*.md` files in
favor of `api_hover`/`api_impls` queries against the symbols
those pages describe. The TOC stops listing concrete files
and starts listing concrete symbols to query.

This is the most balanced option: it keeps the editorial
layer that LSP cannot recreate, drops the snapshot that goes
stale, and trades 864 disk files for live queries.

### Option D -- Augment-only

Leave the markdown corpus exactly as-is. Add two new tools:
`api_expand_macro` and `api_impls`. These are the things
rustdoc genuinely can't answer; everything else stays on the
existing path. Smallest change, biggest macro-related
payoff, easiest to revert if it doesn't help. Good first
step before committing to Option C.

---

## 7. Recommendation and open questions

**Recommendation:** start at Option D, plan for Option C.

- Option D is one or two tools' worth of work and validates
  whether the agent actually uses LSP answers when offered
  them. If `expand_macro` and `impls` don't move outcomes,
  the full migration to Option C is premature.
- If Option D pays off, move to Option C: keep the TOC as
  editorial scaffolding, retire the per-page markdown, route
  everything else through `hover`/`definition`/`completion`.
- Skip Option B -- the robustness work suggests narrow,
  named tools beat exposing LSP shape directly.

**Open questions to resolve before any wiring:**

1. Session lifecycle. One rust-analyzer per sim-flow run, or
   per task? What invalidates the index?
2. Failure mode when the workspace doesn't typecheck. Do we
   degrade gracefully to the static corpus, or surface the
   error?
3. Caching layer. Are `hover` results deterministic enough
   to memoize per workspace SHA? Helps prompt caching.
4. Scope. We currently steer the agent away from internal
   helper modules. `workspace/symbol` returns everything --
   do we filter to the public surface, or trust the agent?
5. Cost. The DM2d prompt budget already pulls in a lot of
   context. How many LSP round-trips per task before this is
   a net loss vs reading one curated markdown page?
6. Macro UX. `expand_macro` outputs are large. Truncate?
   Summarize? Return a delta?
