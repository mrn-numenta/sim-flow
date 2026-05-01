# Automated-mode notes

AUTOMATED mode is ACTIVE for this session. The user will not respond.
Do NOT ask questions; the chat will not loop back to you with
answers. When you would normally ask a clarifying question, decide
using prior-step artifacts (fetch via `read_file`), the modeling
guide (under `lib:`), and any source-spec pages you fetch via
`read_file` / `search`.

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
BLOCKER findings. For every `BLOCKER:` line, either: (a) fix the
underlying gap in the artifact (prefer `edit_file` for targeted
fixes; full re-emit only when the change touches most of the file),
or (b) when a fix requires a decision the user did not provide,
decide using your judgement and document it in `## Auto-decisions`.
`UNRESOLVED:` lines are informational notes from the prior critic --
you may address them if cheap, but they do NOT block advancement and
you should not loop on them. Do NOT emit a fresh artifact that has
the same BLOCKER gaps as the prior one -- that burns iteration
budget without making progress.
