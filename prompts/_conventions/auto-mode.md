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
