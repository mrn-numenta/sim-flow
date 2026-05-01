# Artifact persistence (MANDATORY)

You have native filesystem tools available (Write, Edit, Read,
Glob). When the step's instructions tell you to write a specific
file, use the Write tool to create it (or Edit for targeted
updates). Do NOT emit a fenced markdown code block whose info-string
is the file path -- this orchestrator does NOT extract files from
your response text in this mode; ONLY your tool calls reach disk. A
fenced block looks like a write but produces no file, which is the
most common cause of "the spec was generated but never written"
round-trips.

## Hard rules

- Use Write to create new files; Edit for targeted updates; paths
  are relative to the project root.
- The path must be EXACTLY what the step instruction specifies.
- Do not describe a file in prose instead of writing it via a tool
  call.
- Do not paste the file contents inside a ` ``` ` fence as a
  substitute for calling Write -- it will not be persisted.
- NEVER write, edit, delete, or move anything under `.sim-flow/`.
  That directory is the orchestrator's private state tree
  (`state.toml`, `config.toml`, prompt overrides, control sockets,
  experiments DB, debug logs). Touching it -- including "fixing"
  `state.toml` to mark a step passed -- corrupts the flow. You may
  READ from `.sim-flow/spec-pages/` and `.sim-flow/source-spec.md`
  when the orchestrator inlined pointers to them; everything else
  under `.sim-flow/` is off-limits. Generated documents go under
  `docs/` (e.g. critiques live at
  `docs/critiques/<step>-critique.md`); project source under `src/`;
  analysis artifacts under `analysis/`.
