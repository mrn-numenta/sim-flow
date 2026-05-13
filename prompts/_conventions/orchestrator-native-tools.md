# Artifact persistence (MANDATORY)

You are running through the sim-flow orchestrator with native
function-calling enabled. The orchestrator advertises a tool
catalog (`write_file`, `edit_file`, `delete_file`, `read_file`,
`list_dir`, `search`, `run_cargo`) as native function tools. When
the step's instructions tell you to write or edit a specific file,
**CALL the matching tool** -- do NOT emit a fenced markdown code
block whose info-string is the file path.

The orchestrator does NOT extract files from your response text in
this mode. ONLY your native tool calls reach disk. A fenced block
that looks like a write produces no file. This is by far the most
common cause of "the spec was generated but never written"
round-trips. If you find yourself about to write triple-backticks
followed by a relative path, STOP and call `write_file` instead.

## Hard rules

- Use `write_file({"path": "...", "content": "..."})` to create or
  fully replace a file.
- Use `edit_file({"path": "...", "old_string": "...", "new_string":
  "..."})` for targeted updates. `old_string` must appear EXACTLY
  ONCE in the current file -- include enough surrounding context
  to make the substring unique.
- Use `delete_file({"path": "..."})` to remove an orphan file --
  for example, when a prior milestone renamed a source module and
  left a 0-byte stub at the old location. Scope: only paths that
  fall inside this step's write allowlist (the same paths
  `write_file` and `edit_file` accept). Directories are NOT
  removable; this tool only deletes regular files.
  - When the user asks (in their prompt) for a delete that falls
    OUTSIDE the allowlist, call `delete_file` with the path anyway.
    The orchestrator will pause and ask the user to approve a
    one-shot scope override. On `yes`, the next `delete_file` for
    that path succeeds; on `no`, you'll see the user's response
    in the next turn and should course-correct (acknowledge the
    refusal, propose an alternative, or move on). Do NOT pre-emptively
    refuse the user's request -- let the orchestrator's prompt
    surface the scope question.
- Use `read_file({"path": "..."})` to inspect a file before
  editing.
- Use `list_dir({"path": "..."})` and `search({"pattern": "...",
  "path": "..."})` to explore.
- Use `run_cargo({"subcommand": "...", "args": [...]})` for build /
  test / check steps where that tool is in scope.
- The `path` argument must be EXACTLY what the step instruction
  specifies. Project-relative paths only. No traversal (`../`).
- Do NOT describe a file in prose instead of writing it via a tool
  call.
- Do NOT paste the file contents inside a ` ``` ` fence as a
  substitute for calling `write_file` -- it will not be persisted.
- NEVER write, edit, delete, or move anything under `.sim-flow/`.
  That directory is the orchestrator's private state tree
  (`state.toml`, `config.toml`, prompt overrides, control sockets,
  experiments DB, debug logs). Touching it -- including "fixing"
  `state.toml` to mark a step passed -- corrupts the flow. You may
  READ from `.sim-flow/spec-pages/` and `.sim-flow/source-spec.md`
  when the orchestrator inlined pointers to them; everything else
  under `.sim-flow/` is off-limits. Generated documents go under
  `docs/` (e.g. critiques live at
  `docs/critiques/<step>-critique.json` -- emit them by calling
  `write_file` with that path); project source under `src/`;
  analysis artifacts under `analysis/`.

## Overriding step-prompt phrasing

Older step instructions in this session may describe an "artifact-
write convention" that asks you to emit fenced code blocks whose
info-string is the relative path. **THIS CONVENTION DOES NOT APPLY
IN NATIVE-TOOL-CALLS MODE.** Treat every instance of "emit a
fenced block with the path as the info-string" as "call
`write_file` with that path and the same content." If a step's
`## Output` section gives you a relative path, that's the `path`
argument to `write_file` -- nothing more.

This includes critique outputs. The DM*-critique step prompts
describe a fenced JSON block at `docs/critiques/<step>-critique.json`;
in this mode you call `write_file({"path": "docs/critiques/<step>-
critique.json", "content": "<json>"})` instead.

## Why this matters

Native tool calls are a structurally typed channel: the function
name and argument names are part of the API schema and cannot be
substituted (you cannot accidentally write to the language-tag
slot, because there is no language-tag slot). Fenced blocks rely
on the model emitting a relative path in exactly the right place
in a markdown info-string; one substitution (e.g. ` ```markdown `
instead of ` ```docs/spec.md `) drops the entire body silently.
The model-robustness study quantifies this at 33-62% trials
affected on weaker open models. Using `write_file` eliminates the
class entirely.
