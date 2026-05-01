# Artifact-write convention (MANDATORY)

Your job is to PRODUCE FILES, not to describe them. When the step's
instructions tell you to write a specific file, you MUST emit the
ENTIRE updated file content as a fenced markdown code block whose
opening fence's info-string is the exact relative path the
instruction names. Use this form, verbatim:

```<relative-path>
<file content>
```

## Hard rules

- The path must be EXACTLY what the step instruction specifies.
- Always include the complete file content, not a diff.
- Do not describe a file in prose instead of emitting its block.
- Prose / explanation can appear outside the block; anything not
  inside a fenced block with a path info-string is treated as
  commentary only and will NOT be written to disk.
- NEVER write under `.sim-flow/`. That directory is the
  orchestrator's private state tree (`state.toml`, `config.toml`,
  prompt overrides, control sockets, debug logs). Writes there are
  rejected even if you emit a fenced block. Generated documents go
  under `docs/` (e.g. critiques live at
  `docs/critiques/<step>-critique.md`); project source under `src/`;
  analysis artifacts under `analysis/`.
