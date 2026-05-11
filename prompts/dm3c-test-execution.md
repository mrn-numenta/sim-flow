# DM3c - Test Execution and Coverage (work session)

You are executing step DM3c (Test Execution and Coverage) of the
Direct Modeling Flow. Prerequisite: DM3b gate passed.

## Goal

Implement and run every test enumerated across DM3a's
`docs/test-plan/test-milestone-NN-<name>.md` files using the
UVM-lite testbench DM3b landed. Then measure coverage with
`cargo-tarpaulin` and meet the threshold `coverage.md`
specified.

DM3c is milestone-driven: walk each
`test-milestone-NN-<name>.md` file in order, complete one
milestone at a time, stop for the paired critique after each.
Do NOT chain milestones.

## Inputs

- `docs/impl-plan/plan-management.md` -- plan-file conventions
  (task states, ordering, the 10-task cap).
- `docs/test-plan/test-plan.md` -- index. Read first to orient
  on testbench architecture and the TOC of milestone files.
- `docs/test-plan/test-milestone-NN-<name>.md` -- per-milestone
  task lists. The first file with at least one `- [ ]` row is
  your current milestone.
- `docs/test-plan/coverage.md` -- coverage strategy (threshold,
  exclusions, run command, report path). Consumed by the
  coverage milestone.
- `docs/testbench.md` -- testbench architecture; useful when a
  test failure raises questions about what a Monitor should
  observe.
- `tests/` -- the testbench scaffolding DM3b produced. You add
  test bodies but do NOT modify the scaffolding (Sequencers,
  Drivers, Monitors, Scoreboards, `SimEnvBuilder` helper). If
  the scaffolding is wrong, flag it and stop -- DM3b's gate
  failed.
- `src/` -- the model under test, for understanding behavior
  when a test fails.

## Procedure

1. **Pick the current milestone**. Walk
   `docs/test-plan/test-milestone-NN-<name>.md` files in
   lexicographic filename order, which IS the canonical category
   order: **smoke (01) -> edge (02) -> stress (03) -> random
   (04) -> coverage (05)**. The first file with at least one
   open `- [ ]` row is your current milestone. Do NOT skip ahead
   to a later category because the current one looks done at a
   glance -- a missed `- [ ]` in smoke / edge gets caught at the
   per-milestone critique, and the order matters because smoke
   is a precondition for edge, stress is a precondition for
   random, etc. Read ONLY that milestone file and any supporting
   context its tasks reference -- do NOT bulk-read later
   milestones; that defeats the small-context-per-review point.

2. **Walk the tasks in this milestone IN ORDER**. For each
   `- [ ]` row:
   - Write the test body using the testbench helpers DM3b
     defined; do not invent new components.
   - Run `run_cargo({"command": "test", "args": ["<test_name>"]})`
     (or the equivalent shell invocation if `run_cargo` is
     unavailable).
   - If the test fails because of a design bug in `src/`, fix
     the model and re-run; record the fix in the test row's
     `- fix:` sub-bullet.
   - If the test fails because of a test bug, fix the test.
   - Once passing, flip the row's `- [ ]` to `- [x]` in the
     milestone file. ONLY change the checkbox; do NOT modify
     the task text or reorder rows.

3. **Random-test reproducibility**. Random milestones must pin
   a seed in each test name (`<test>_seed_<N>`) per the plan.
   A failure of `foo_seed_42` should be reproducible from the
   seed alone; never use uncontrolled randomness.

4. **Coverage milestone**. The final test-execution milestone
   (typically `test-milestone-05-coverage.md`) walks
   `coverage.md`'s run command:
   - Install if missing: `cargo install cargo-tarpaulin`
     (one-time per environment).
   - Run the command from `coverage.md` (typically
     `cargo tarpaulin --out Html --out Lcov --output-dir
     target/coverage` plus the `--exclude-files` flags).
   - Read the line-coverage percentage from the output.
   - If at or above `coverage.md`'s declared threshold, record
     the measured percentage and report path in
     `docs/test-plan/test-plan.md`'s `## Coverage` section
     (e.g. `coverage report: target/coverage/lcov.info
     (line: 92.4%)`). Strategy stays in `coverage.md`; the
     measured result goes in the index.
   - If below threshold, identify uncovered lines (open the
     HTML report) and either:
     - add tests to cover them (preferred -- adds new `- [x]`
       rows to the appropriate test-milestone file), or
     - extend `coverage.md`'s `## Exclusions` list with the
       specific file / module + a one-line reason. The reason
       must be specific (e.g. "platform-gated Windows path
       under `cfg(windows)`"), not vague ("unimportant").
       Adding exclusions in DM3c is allowed only when the
       coverage gap is genuinely test-resistant.

5. **Verify and stop**. When every `- [ ]` row in the current
   milestone is resolved (`- [x]` done OR `- [-]` deferred with
   a `- defer reason:` sub-bullet):
   - `run_cargo({"command": "test"})` -- the full suite still
     passes (no regression).
   - `cargo fmt --check` AND `cargo clippy --all-targets -- -D
     warnings` are run AUTOMATICALLY by the orchestrator after
     you stop and surfaced to the next critique. Do NOT invoke
     them yourself; their results are authoritative when the
     critique sees them. A FAIL on either gets flagged as a
     BLOCKER and you'll re-enter the milestone with diagnostics
     inlined.

   Then **STOP**. Surface a clear notice:

   > `test-milestone NN: <name> complete; ready for critique.`
   > `<one-line summary: count of tests added, design fixes
   > that landed, deferred items>`

   Do NOT roll into the next milestone. The paired critique is
   the gate; a clean critique advances DM3c, a critique with
   `BLOCKER:` items sends you back into the same milestone with
   focused feedback.

### Order, jumping, and deferring

`docs/impl-plan/plan-management.md` is the source of truth: task
states (`- [ ]` / `- [x]` / `- [-]` with `defer reason:`),
out-of-order work (`order swap:` sub-bullet), and additions
(`added:` sub-bullet). Read it before starting.

DM3c-specific note: deferred (`- [-]`) rows count as resolved
for milestone-completion, but they do NOT contribute to the
coverage threshold's pass criterion. A milestone where every row
was deferred is a `BLOCKER:` for the critique to flag because
the flow has no meaningful execution signal for that class of
behavior, even though the file itself is "complete" by the
row-count rule. If you find yourself deferring more than ~25% of
a milestone's rows, stop and surface the trend rather than
coasting -- the critique flags a "mostly-deferred milestone" as
a `BLOCKER:` even when individual defer reasons read fine.

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
  axes (one test per file -- see "File Layout" below) rather
  than letting any single file grow without bound. The critique
  flags any source file at or above 400 lines as `BLOCKER:`.

## File Layout

DM3c writes EACH test to its own file under a per-category
subdirectory:

- `tests/smoke/<test_name>.rs` -- one file per smoke test.
- `tests/edge/<test_name>.rs` -- one file per edge test.
- `tests/stress/<test_name>.rs` -- one file per stress test.
- `tests/random/<test_name>.rs` -- one file per random test.
  Random test names pin a seed (`<test>_seed_<N>.rs`).

DM3b's testbench scaffolding lives under `tests/testbench/`
(separate, not modified by DM3c). The basic data-flow smoke test
DM3b authored lives at `tests/smoke/basic_data_flow.rs` -- DM3c
does NOT overwrite it.

`<test_name>.rs` matches the test's `#[test]` function name
exactly (so `cargo test <test_name>` lines up with the file).
File-per-test gives the critique a clean unit of review and
keeps each file well under the 400-line cap.

## Re-entry

If DM3c runs across multiple work + critique sessions, restart
by walking the `test-milestone-NN-*.md` files in numeric order.
The first one with at least one `- [ ]` row -- or any test
whose name has no matching `#[test]` function in `tests/` -- is
your current milestone, and you start at the first such row in
that file.

## Output

**Use the path as the fence info-string, verbatim.** Opening
the fence with a language tag (`markdown`, `json`, `toml`, `rust`,
`yaml`, `text`, `md`, `rs`, `yml`, `txt`) means the body is
**silently dropped** -- the file never lands on disk, the gate
fails, and the work session burns its retry budget. See
`_conventions/fenced-blocks.md` ("Language-tag info-strings are
SILENTLY DROPPED") for the failure mode in detail. If you don't
remember the exact path, run `tool:read_file` / `tool:list_dir`
to discover it -- never guess `\`\`\`markdown` as a fallback.

When the artifacts in the current milestone are complete and
verified, stop with the milestone-complete notice. Do not write
`docs/critiques/DM3c-critique.md`; the critique is a distinct
task. Do not `/exit` on your own -- the user and the orchestrator
control session boundaries.

Final output, after all milestones are complete and the final
critique has passed:

- `cargo test` passes (every implemented test).
- `cargo tarpaulin` reports line coverage at or above
  `coverage.md`'s threshold.
- Every row in every `test-milestone-NN-*.md` is `- [x]` or
  `- [-]` with a `defer reason:`.
- The coverage report path + measured percentage is recorded in
  `docs/test-plan/test-plan.md`'s `## Coverage` section.
- Any uncovered lines that are intentionally left below direct
  coverage are recorded in `coverage.md`'s `## Exclusions` list
  with a concrete reason.
