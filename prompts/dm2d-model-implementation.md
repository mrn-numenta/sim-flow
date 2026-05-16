# DM2d - Model Implementation (work session)

You are executing step DM2d (Model Implementation) of the Direct
Modeling Flow. Prerequisite: DM2c gate passed.

## Goal

Execute the implementation plan written in DM2c to produce a
cycle-accurate sim-foundation model that elaborates and passes smoke
tests. Exhaustive verification is DM3, not here.

## Inputs

Read these before starting:

- `docs/impl-plan/plan.md` -- the milestone index. Read this first; it
  tells you which milestone files to read in what order.
- `docs/impl-plan/milestone-*.md` -- per-milestone task lists. The plan
  is your source of truth for what to build and in what order;
  follow it task by task.
- `docs/spec.md`
- `docs/targets.md`
- `docs/testbench.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/pipeline-mapping.md`
- `docs/analysis/data-movement.md`

Reference material (read on demand; do NOT bulk-read upfront):

- **PRIMARY -- sim-models** via the `lib:` prefix: modeling guide,
  worked examples, library models, prior user projects. These are
  the curated, opinionated answers to "how do I express this in the
  framework?". Always check here first.
- **SECONDARY -- foundation framework public API**. The framework
  is large; consult it on demand, NOT upfront, and only when `lib:`
  doesn't answer your question. Two routes, in order:
    1. **`api_*` tools (preferred)** -- talk to a live
       `rust-analyzer` rooted at the foundation workspace, so the
       content always matches the current code. Read `fw:api/toc.md`
       once for the tool palette and the curated starting-point
       symbols, then use:
         - `api_search(query)` to find symbols by name,
         - `api_hover(query)` for a symbol's signature + rustdoc
           (the live replacement for `fw:api/pages/.../*.md`),
         - `api_impls(query)` to enumerate every `impl` of a trait,
         - `api_references(query)` to see how a type is consumed,
         - `api_expand_macro(path, line)` to see what a derive
           (`HasLogic`, `ConfigModel`, ...) actually generates.
       First call per session spawns rust-analyzer and waits for
       initial indexing (~2 min on a cold workspace); subsequent
       calls are fast.
    2. **`fw:api/pages/.../*.md` snapshot (fallback)** -- a static
       rustdoc mirror, still under `fw:api/pages/...`. Use it when
       the live tools don't cover something or rust-analyzer is
       unavailable. Use `fw:src/prelude.rs` or other `fw:src/...`
       paths only when you need an exact signature missing from
       both routes. Do not browse internal helpers; treat anything
       outside the curated public API surface as implementation
       detail.

Each top-level `lib:` directory has a `README.md` that indexes its
contents -- start there before diving into individual files. Consult
`lib:` material in this order of authority:

- **Modeling guide** (canonical): start with
  `lib:docs/modeling-guide/README.md`, then read the numbered
  chapters it points to (01-quickstart, 02-rust-quickstart,
  03-building-models, 04-testing-models,
  05-observability-and-analysis, 06-design-patterns).
- **Worked examples** (canonical, minimal): start with
  `lib:examples/README.md` for the index, then read the README
  inside each example directory before its source. Pick a few
  examples whose topology matches your design.
- **Library models** (canonical, production-grade reusable building
  blocks): start with `lib:library/README.md` if present, and the
  per-module READMEs (e.g. `lib:library/micron-lpddr5x/README.md`).
  Reference for non-trivial structures (memory subsystems, NoC
  fabrics, SoC composition).
- **User projects** (NON-canonical, illustrative only):
  `lib:users/<name>/` -- prior-art models from other users in this
  org. Read any `README.md` first. Useful for ideas but they may
  use older idioms or non-standard patterns; prefer the modeling
  guide / examples / library when they conflict.

You MAY consult the foundation framework's public API via `fw:`
(start with `fw:api/toc.md`); do NOT browse other sim-foundation
crates or the framework crate's internal helper modules. Use `fw:` and
`lib:` to answer "how do I express this in the framework?", not
"what should I build?" The DM2c plan remains the source of truth for
scope and structure.

## Procedure

1. Read `docs/impl-plan/plan.md` to orient. The orchestrator scopes
   each work session to ONE `docs/impl-plan/milestone-NN-*.md` file
   at a time -- only the current milestone appears in your inputs.
   Do that milestone and stop (see step 3 below); the auto-driver
   re-launches you for the next milestone after the paired critique.
2. Read `docs/targets.md` and `docs/testbench.md` before starting
   implementation.
   - Use `docs/targets.md` to preserve target-sensitive structural
     choices such as gate-budget-driven stage boundaries, buffering, or
     other performance-sensitive implementation decisions already baked
     into the plan and mapping.
   - Use `docs/testbench.md` to notice observability or smoke-test needs
     that must be supported structurally during implementation.
   - Do not turn DM2d into a full verification-implementation step;
     DM3 still owns the full testbench and verification suite.
3. For each milestone:
   - Read its `milestone-NN-<name>.md` file.
   - Work through tasks in the order they're listed. See "Order,
     jumping, and deferring" below before deviating.
   - As you complete each task, edit the milestone file to flip
     its `- [ ]` to `- [x]`.
   - Run `run_cargo` as appropriate (see step 6 below) to confirm
     compilation / tests as you land artifacts.
   - When the LAST `- [ ]` in the milestone is resolved (`- [x]`
     done OR `- [-]` deferred), **STOP**. Surface a clear notice
     -- `Milestone NN: <name> complete; ready for critique.`
     followed by a one-line summary of what landed (and any
     deferred items, with a single sentence each) -- and wait for
     the paired critique session before starting the next
     milestone. Do NOT chain milestones. The critique is the
     primary correctness check for the milestone; user review may
     happen around it, but you should assume advancement depends on
     the critique passing, not just on the milestone file being
     checked off.

### Order, jumping, and deferring

`docs/impl-plan/plan-management.md` is the source of truth for how to
walk a plan: task states (`- [ ]` pending, `- [x]` done, `- [-]`
deferred with `defer reason:` sub-bullet), how to handle
out-of-order work (`order swap:` sub-bullet documenting why),
and how to add work the plan missed (`added:` sub-bullet on a
new `- [ ]` / `- [x]` row). Read it before starting; follow it
strictly.
4. **Payload types**: create Rust structs in `src/model/` derived
   from `data-movement.md`. Payload types live alongside the
   modules that produce and consume them.
5. **Connectivity**: wire every pipeline stage by implementing
   `HasInstances` on the parent module — register children via
   `InstanceBuilder::instance(name, child)` inside `instances()`,
   and bind their ports inside `connect()` using `NetBuilder`
   (`bind_input_named` / `bind_output_named` / `connect_named`).
   Name modules after operations from `decomposition.md` so future
   readers can trace spec -> decomposition -> code. Do **not** use
   the `ConnectivityPlanBuilder` recipe path; the inline
   `HasInstances` + `connect()` style is what the gate expects.
6. **Modules**: implement each module using the Foundation
   `Module`, `HasLogic`, and `HasInstances` traits. Every module
   must respect framework invariants and the structure established by
   the plan and pipeline mapping. For simple modules, implementing logic
   directly in `evaluate()` may be fine; for complex modules, factor
   helpers when that improves clarity. Do not treat those style notes as
   permission to ignore the plan's intended architecture.

   **Observability discipline**: meaningful state (counters, flags,
   queue depths, last-cause discriminators) lives in `&self` fields
   covered by `#[derive(SignalTraceState)]`, not as locals inside
   `evaluate()`. External probes attach to the model by hierarchical
   path at perf-analysis time; they observe ports + struct fields, so
   anything you'd later want to measure must survive evaluate as
   addressable state. Do NOT embed `LatencyProbe` / `StallProbe` /
   `ThroughputProbe` / `OccupancyProbe` etc. by default -- DM4 owns
   the probe-instantiation policy via `docs/perf-plan/probes.toml`,
   and embedded probes pay their hot-path cost on every run. Embed
   one only when an evaluate-local computation legitimately cannot
   be exposed as a field; flag the exception so the critique can
   weigh it.
7. **Cargo verification**: after each module lands, invoke the
   `run_cargo` tool to verify it compiles / passes:
   - `run_cargo({"command": "check"})` -- cheap type-only pass
     while iterating.
   - `run_cargo({"command": "build"})` -- once you think a
     module is done.
   - `run_cargo({"command": "test"})` -- once smoke / unit
     tests are in place, the elaboration smoke test must pass.
   `cargo fmt --check` AND `cargo clippy --all-targets -- -D
   warnings` are run AUTOMATICALLY by the orchestrator AFTER you
   stop and surfaced to the next critique. Do NOT invoke them
   yourself; their results are authoritative when the critique
   sees them. Any FAIL is flagged as a BLOCKER and you'll
   re-enter the milestone with diagnostics inlined.
   Read the returned stdout / stderr; if there are real errors,
   fix them and re-run. Do NOT guess at build errors from
   source -- always confirm with `run_cargo` output.
8. **Tests**: write **only** unit tests and smoke tests at this
   step. Exhaustive verification (directed sequences, coverage
   targets, randomized stimulus, scoreboards) belongs to **DM3** --
   do not pre-empt that scope here.
   The smoke tests **must live in `tests/elaboration.rs`** —
   that's the file the gate runs via `cargo test --test elaboration`.
   Other test files (e.g. `tests/units.rs`) are allowed alongside
   it for per-module unit tests, but the elaboration smoke set
   has to be in `tests/elaboration.rs` specifically; renaming it
   breaks the gate. Cover:
   - **Smoke** (in `tests/elaboration.rs`): elaboration (topology
     builds without error), basic data flow through the pipeline,
     backpressure propagation, idle cycles produce no spurious
     outputs.
   - **Unit** (small, focused): per-module correctness of the
     `evaluate()` core for a couple of representative inputs --
     enough to catch obvious wiring / payload-type mistakes while
     iterating, not enough to substitute for DV.
   If you find yourself writing scoreboards, sequencers, or a
   directed-test suite, stop -- defer that work to DM3.
9. **Tick off completed plan tasks**. As you finish each task in
   `docs/impl-plan/milestone-*.md`, use `edit_file` to flip the leading
   `- [ ]` to `- [x]`. ONLY change the checkbox; do NOT modify the
   task text, reorder tasks, add new ones, or restructure
   milestones — the plan is DM2c's contract and the critique flags
   any drift. If a task turns out to be wrong or impossible, leave
   the box unchecked and document the discrepancy in your final
   summary instead. Closed-out checklists are how the critique
   confirms milestone-by-milestone progress on incremental DM2d
   reviews; missing them shows up as `UNRESOLVED:` items at minimum
   and can hide regressions during multi-milestone retries.

## Coding Requirements

All Rust code authored in this step MUST follow these rules. The
critique flags violations as `BLOCKER:` because downstream steps
depend on the codebase staying readable, idiomatic, and
modification-friendly across iterations.

- **Idiomatic Rust**. Prefer the standard idioms (`?` for error
  propagation, `Result` / `Option` over panics for recoverable
  conditions, iterators over manual loops, pattern matching over
  nested `if let`). Boring code beats clever code.
- **Data-oriented + memory-friendly**. Prefer concrete types over
  trait objects, owned data over indirection, contiguous storage
  (`Vec`, fixed-size arrays) over heap-of-heaps, struct-of-arrays
  when iteration patterns favor it. Avoid premature
  `Arc<Mutex<_>>` and similar shared-mutable indirection unless
  the framework forces it.
- **Functional where appropriate**. Small pure helpers, immutable
  bindings by default, `iter().map().filter().collect()` over
  mutable accumulators, exhaustive `match` for state machines.
- **No magic numbers or strings**. Any literal with meaning
  beyond "this exact value" must be a named `const` (or named
  enum variant, or named struct field). Port names, payload
  widths, threshold values, run-id schemes -- all named, not
  inlined.
- **No emojis**. Comments, error messages, doc strings, log
  output, and string literals stay ASCII. Emojis muddle
  terminals, diffs, and grep.
- **File size cap: under 400 lines**. Split files along clear
  axes (one component / module per file) rather than letting any
  single file grow without bound. The critique flags any source
  file at or above 400 lines as `BLOCKER:`.

## Constraints

- **Block-diagram contract**. The orchestrator auto-renders the
  block diagram on the DM2d -> DM3a advance via
  `crate::dump_topology(&args)` defined in `src/lib.rs`.
  The contract DM2d MUST keep intact:
  - `pub struct Top` (literal name `Top`) stays in
    `src/model/top.rs` and has `Default + Module +
    HasInstances + HasLogic` impls.
  - `dump_topology` in `src/lib.rs` stays callable with
    `&TopologyDumpArgs`. Don't rename, delete, or change its
    signature.
  - `src/main.rs`'s top-of-`fn main` dispatch (which calls
    `dump_topology` when any `--dump-*` flag is passed) stays
    in place.

  **DO NOT** redirect `Top` through a re-export
  (`pub use crate::pipeline::Foo as Top;`) -- the gate's
  structural grep checks `src/model/top.rs` for the literal
  text `pub struct Top` AND `impl Module for Top`. A type
  alias or re-export does not contain those tokens; the gate
  fails even though the code compiles. If you want a
  descriptive struct name internally, define it as the inner
  field of `Top`, not as `Top` itself.

  Canonical shape the gate expects (replace stub bodies, keep
  the names + traits + module location):

  ```rust
  // src/model/top.rs
  use foundation_framework::prelude::*;

  #[derive(Clone, Debug, Default, HasInstances, HasLogic, SignalTraceState)]
  pub struct Top {
      // ... your stage instances + ports here ...
  }

  impl Module for Top {
      fn evaluate(&mut self, ctx: &mut EvaluateContext) { /* ... */ }
      fn settle(&mut self, ctx: &mut SettleContext)     { /* ... */ }
      fn update(&mut self, ctx: &mut UpdateContext)     { /* ... */ }
  }
  ```

  The pipeline structure / stages live as child modules
  (`src/model/<stage>.rs`) referenced from `Top`'s fields, not
  from a sibling `src/pipeline.rs`. Anything DM2d writes that
  the milestone task list names under `src/model/` MUST land
  under `src/model/`; writes to `src/<file>.rs` for code that
  the plan placed under `src/model/` are auto-redirected by
  the orchestrator (you'll see a notice in the tool result)
  -- but the right move is to write the canonical path the
  first time.

  Replace the stub `Top` body with the real model -- add
  fields, child modules, port wiring, evaluate/settle/update
  bodies -- but keep the type name + `Default::default()`
  constructibility so the orchestrator's auto-render keeps
  working through the entire flow.
- Do not bypass the Foundation port system.
- Do not implement custom scheduling.
- Do not alter the `Module` phase order (evaluate -> settle -> update).
- Outside the framework's public API surface (reachable via `fw:`),
  do not read or cite sim-foundation source files; use the docs and
  examples in `sim-models/` (via `lib:`) as the source of truth.
- Stay inside the plan. If a task in the plan turns out to be wrong
  or impossible, flag the issue (in auto mode, document the
  decision in the milestone file's `## Auto-decisions` section)
  rather than silently deviating.
- References do not override the plan. If examples or public framework
  APIs suggest a different structure than the plan, prefer the plan and
  document the tension rather than drifting silently.

## Re-entry

If DM2d runs across multiple work + critique sessions (a milestone
gets re-prompted because the critique flagged something, or the
session was killed mid-milestone), restart by walking
`docs/impl-plan/milestone-NN-*.md` files in numeric order. The
first one with at least one `- [ ]` row OR any task whose code
hasn't actually landed in `src/` is your current milestone, and
you start at the first such row in that file. Do NOT skip a
milestone just because its rows are all `[x]` -- if `cargo build`
or the elaboration smoke test fails on first run, the prior
milestone's claim of completeness was wrong; back up and reopen
the failing tasks before moving forward.

## Output

{{ output_intro }}

- `src/model/` populated with modules and payload types.
- Smoke tests + minimal unit tests under `tests/` passing. Full
  verification suite is DM3, not here.
- Every milestone file's tasks marked `[x]`.
- `cargo build` and `cargo test` both succeed.

Milestone completion and step completion are different:

- After each milestone is complete, stop and wait for the paired
  milestone critique before starting the next milestone.
- After the final milestone is complete and the build/tests are green,
  stop for the final DM2d critique. That final critique is the
  end-to-end integration/regression pass across the full
  implementation, not the first time the work is being reviewed.

Do not write
`docs/critiques/DM2d-critique.md`; the critique is a distinct task.
Do not `/exit` on your own -- the user and the orchestrator control
session boundaries.
