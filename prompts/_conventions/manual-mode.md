# Manual-mode notes

MANUAL mode is ACTIVE for this session. The user IS available and
will respond to your questions.

When the step's instructions or a gate check would normally have you
guess at missing information, do NOT guess. Ask the user ONE
specific, concrete question per turn and wait for the answer before
continuing. The chat will loop back to you with their reply.
Auto-decisions are NOT appropriate in this mode -- they're reserved
for unattended runs.

Stay strictly within the step's scope. The "Step inputs and target
artifacts" message above lists every file this step is allowed to
read or write. Treat that list as authoritative:

- Do NOT run `cargo build` / `cargo test` / `cargo check` /
  `cargo clippy` unless the step's `phases` includes `build` /
  `test`. Speculative builds during a `chat`-only step waste budget
  and don't surface anything the step actually needs.
- Do NOT `search` or `list_dir` across the framework, library, or
  project tree looking for unrelated context. The artifacts listed
  in the system context above are the only inputs you should
  consider; if you need more, ask the user.
- Do NOT read configuration / tooling files (`.github/`,
  `.claude/`, `Cargo.toml`, etc.) the step doesn't list. They are
  not part of this step.

After your first artifact-write turn the orchestrator will evaluate
the structural gate (file-exists / file-matches checks; the
critique-clean check is intentionally excluded because critique is a
distinct task with its own prompt and is not your job in this work
pass). If the gate fails it will feed the failure list back to you
as the next user message; respond by re-emitting the affected
artifact(s) with the issues fixed -- or, when the change is small (a
renamed header, a typo, a single value), use `edit_file` instead of
re-emitting the whole artifact.

If a `<step>-critique.md` file is inlined below (a previous critique
pass found issues), your job on THIS iteration is to address the
BLOCKER findings. For every `BLOCKER:` line, either: (a) fix the
underlying gap in the artifact (prefer `edit_file` for targeted
fixes; full re-emit only when the change touches most of the file),
or (b) when a fix needs a decision the user hasn't given you, ask
the user a concrete question about that decision. `UNRESOLVED:`
lines are informational; you may surface them in your question but
they do NOT block advancement.
