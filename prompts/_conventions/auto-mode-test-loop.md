# Automated-mode notes (test-loop addendum)

The following sections cover the cargo-test investigation /
fix-attempt accounting and the per-project bug log. They only
load for steps with a `cargo build` / `cargo test` loop (DM2d,
DM3b, DM3c, and any other step whose `work_phases` includes
`"build"` or `"test"`). Chat-only auto-mode sessions skip this
section.

## Investigation vs fix attempts (`declare_fix`)

The auto driver classifies every `run_cargo test` turn as either an
**investigation** (you're measuring / probing) or a **fix attempt**
(you committed to a specific change). Investigation turns are
cheap; fix attempts are bounded. Both have separate caps:

- **Investigation budget** (default 10 turns since the last fix
  attempt): you may run `cargo test`, read framework docs, write
  new diagnostic test files (e.g. `tests/<area>/diag_*.rs`) freely.
  This phase is for understanding the failure.
- **Fix-attempt budget**: every time the target failing-test set
  doesn't strictly shrink AND you (a) edited a pre-existing
  step-owned artifact, OR (b) called `declare_fix`, the auto loop
  counts it. Capped by `max_auto_iters` for heuristic touches and
  by a separate declared-fix cap (default 8) for `declare_fix`
  calls.

**When to call `declare_fix`**: right before you run `cargo test`
that you EXPECT to pass. Pass a one-line `rationale` summarising the
change you just made. The orchestrator scores the next test run as
a fix attempt regardless of whether the file-op heuristic saw it.

```text
declare_fix({"rationale": "raised injector rate to 1/cycle to match Foundation's tick contract"})
run_cargo({"command": "test"})
```

**Why bother**:

- If your fix lives in a NEW file (e.g. you refactored a helper
  under `tests/testbench/<new_file>.rs`), the file-op heuristic
  will not classify it as a fix attempt -- it'll look like
  investigation and the test turn won't get credit. `declare_fix`
  fixes that.
- If you've finished a clear investigation phase and want to
  commit, `declare_fix` resets the investigation counter so you
  earn another full budget for the NEXT investigation phase if
  the fix doesn't pan out.

**Do NOT** call `declare_fix` for:

- Pure measurement / probing (just running `cargo test` to see
  state) -- that's investigation.
- Adding a diagnostic test file -- that's investigation.
- Stylistic / typo fixes unrelated to the failing tests.

If you call `declare_fix` 8 times without progress, the auto driver
bails so the operator can decide whether to raise the budget,
inject more framework context, or commit a fix manually. Use the
budget thoughtfully.

**Test-expectation nudge**. After your 4th declared fix without
progress, the auto driver emits a one-time Diagnostic asking you
to consider whether the TEST EXPECTATION is wrong rather than the
implementation. If you see that nudge, pause and check:

- Does the failing assertion match what the spec actually says?
- Is the expected cycle count / output value / port shape derived
  from the spec, or copy-pasted from an earlier draft?
- Is the test asserting against a stale value the implementation
  has since correctly updated past?

If the test expectation is the problem, fix the TEST instead of
declaring another fix on the implementation. The nudge fires at
most once per session and is advisory -- you can ignore it if the
implementation really is wrong, but ignoring it without
considering the alternative is how impl-chasing loops happen.

## Bug log

Every project carries a persistent bug log at
`<project>/.sim-flow/bug-log.jsonl`. Use it to record distinct
failure modes you encounter so the operator can mine the history
later. Applies to ANY step with a failing-test or critique loop:
DM2d unit-test failures, DM3c stress / edge / coverage failures,
DM4b perf-target misses, SV3 verilator failures, plus critique
BLOCKERs that re-occur.

**When to open a bug** (`log_bug({"issue": ..., "category": ...})`):

- The same test(s) fail across two or more critique cycles.
- A critique flagged a structural BLOCKER you can't resolve in one
  pass.
- Cargo / framework / external tooling refuses to run as expected.
- You spot a Foundation behavior the docs don't predict.

ONE bug per distinct issue (not one per turn). Categories (closed
taxonomy -- anything else is a tool error):

- `compile_error` -- `cargo build` / `cargo check` failed.
- `test_failure` -- `cargo test` failed (correctness symptom).
- `missing_test_target` -- `cargo test --test <name>` couldn't find
  the target file.
- `gate_violation` -- a gate check rejected output (write-path,
  milestone deferral, schema).
- `tool_misuse` -- you invoked a specific tool (`log_bug`,
  `run_cargo`, `write_file`, ...) with wrong args / path / shape.
- `framework_misuse` -- you misunderstood a Foundation API in
  `sim-foundation/crates/*` (e.g. used `HasInstances` where
  `HasLogic` was needed).
- `flow_misuse` -- you misunderstood a sim-flow concept in
  `sim-foundation/tools/sim-flow` -- step / milestone semantics,
  the work/critique cycle, write-path allowlists, no-progress
  classifier rules. NOT a specific tool call (that's `tool_misuse`)
  and NOT a bug in sim-flow itself (that's `flow_logic`).
- `prompt_ambiguity` -- the instruction was unclear; you took a
  defensible-but-wrong interpretation. Use this to flag prompts that
  need editing.
- `missing_dependency` -- a required crate / binary / file wasn't on
  disk or on PATH.
- `network` -- LLM dispatch failure, server unreachable, timeout.
- `correctness` -- model logic was wrong (the diagnosis, vs the
  `test_failure` symptom).
- `performance` -- correct output but missed a perf target.
- `documentation` -- markdown / schema / formatting issue, not behavior.
- `flow_logic` -- sim-flow code itself misbehaved (not your fault).
  For misunderstanding sim-flow rather than sim-flow misbehaving,
  use `flow_misuse`.
- `other` -- escape hatch, use sparingly; critique flags `other`-
  heavy logs.

The orchestrator auto-fills the step id and current milestone path.

**While investigating**: call
`declare_hypothesis({"rationale": "<one-line guess>"})` whenever
you form a new theory about the root cause. These are pure
logging -- no effect on the no-progress classifier. They build the
trail of what you considered.

**When committing to a fix**: call
`declare_fix({"rationale": "<one-line summary of the change>"})`
right before the `cargo test` you expect to pass. This signals the
classifier (counts as a fix attempt, resets investigation
budget) AND appends a `fix_attempt` event to the current open bug.

**When fixed**: after the failing tests pass, call
`resolve_bug({"resolution": "<1-3 sentences: root cause + what you
changed>"})`. Marks the bug `status: resolved`; the entry stays
in the log as a permanent record.

**Implicit targeting**: `declare_hypothesis` / `declare_fix` /
`resolve_bug` all target the most-recently-opened OPEN bug (LIFO).
If you have multiple bugs open and need to log against an older
one, resolve / re-open the stack to surface the right one.

If you never call `log_bug`, the tools still work in their
non-bug-log modes (e.g. `declare_fix` still signals the
classifier; it just doesn't append to a bug entry). But the
operator strongly prefers a bug entry for every distinct failure
you investigate -- the value is in the historical record.
