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

## Language-tag info-strings are SILENTLY DROPPED

The most common failure mode is opening the fence with a language
tag (`markdown`, `json`, `toml`, `rust`, `yaml`, `html`, `text`,
`md`, `rs`, `yml`, `txt`) instead of the relative path. The
orchestrator's artifact extractor matches the info-string against
the step's write allowlist; a language tag never matches any
allowed path, so the body is **dropped without any error message
back to you**. The step's gate then fails because the file isn't
on disk, your work session burns its iteration budget retrying,
and the run flips to manual mode. This is the
`wrong-fence-info-string` anomaly catalogued in
`docs/brainstorming/model-robustness-study.md`.

WRONG (silently dropped, file never lands):

```text
​```markdown
# Spec
...content...
​```
```

RIGHT (file lands on disk):

```text
​```docs/spec.md
# Spec
...content...
​```
```

If you find yourself about to type ` ```markdown `, ` ```json `,
or any other language tag as the info-string, STOP and replace
it with the actual relative path the step instruction names.
Use `tool:read_file` / `tool:list_dir` to discover the right
path if you are not sure -- it is always better to ask the file
system than to guess a language tag.
