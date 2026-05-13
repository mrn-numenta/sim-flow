# Automated-mode notes

AUTOMATED mode is ACTIVE for this session. The user will not respond.
Do NOT ask questions; the chat will not loop back to you with
answers. When you would normally ask a clarifying question, decide
using:

- prior-step artifacts under `docs/` (fetch via `read_file`),
- the modeling guide (under `lib:`),
- the **user-supplied source spec** when ingestion produced one --
  available at `.sim-flow/source-spec.md` (or `.sim-flow/source-spec.<ext>`
  for paginated PDF / TXT inputs) and per-page at
  `.sim-flow/spec-pages/<NNN>.md`. The orchestrator may have inlined a
  TOC into this system stack; if it didn't, read
  `.sim-flow/source-spec-toc.md` first and fetch only the pages you
  need (don't request the whole spec at once). For DM0 specifically,
  the source spec is the authoritative input you derive `docs/spec.md`
  from; for later steps it is reference material when `docs/spec.md`
  is ambiguous.

Document each non-trivial decision in an `## Auto-decisions`
subsection of the artifact you are producing. One bullet per
decision, of the form `- decided <X>; rationale: <one sentence>`.

After your first artifact-write turn the orchestrator will evaluate
the structural gate (file-exists / file-matches checks; the
critique-clean check is intentionally excluded because critique is a
distinct task with its own prompt and is not your job in this work
pass). If the gate fails it will feed the failure
list back to you as the next user message; respond by re-emitting
the affected artifact(s) with the issues fixed -- or, when the
change is small (a renamed header, a typo, a single value), use
`edit_file` instead of re-emitting the whole artifact. When the
structural gate passes, the session ends automatically -- you do not
need to say goodbye.

If a `<step>-critique.md` file is inlined below (a previous critique
pass found issues), your job on THIS iteration is to address the
**both** `BLOCKER:` and `UNRESOLVED:` findings. Both block step
advancement -- the gate refuses to clear, and the auto loop's
no-progress detector fires when the count of (Blocker +
Unresolved) findings doesn't strictly decrease across retries.
`UNRESOLVED:` means "previously flagged and STILL outstanding"; it
is not informational, it is a carry-over finding the prior critique
expects you to clear.

For every `BLOCKER:` or `UNRESOLVED:` line, either: (a) fix the
underlying gap in the artifact (prefer `edit_file` for targeted
fixes; full re-emit only when the change touches most of the file),
or (b) when a fix requires a decision the user did not provide,
decide using your judgement and document it in `## Auto-decisions`,
or (c) when the finding cannot be addressed in this artifact (e.g.
it's a fundamental spec conflict with `targets.md`), update the
upstream artifact so the conflict goes away -- and surface the
update in `## Auto-decisions`. Do NOT emit a fresh artifact that
leaves the same Blocker / Unresolved findings unaddressed -- that's
what trips `max_critique_no_progress_iters`.

`RESOLVED:` lines are confirmations from the prior critic that
earlier flagged findings have been fixed; no action required on your
side.

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
