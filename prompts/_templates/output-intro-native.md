Call `write_file({"path": "<one-of-the-paths-below>", "content":
"<full file content>"})` for each artifact. For targeted edits to
an existing file, call `edit_file({"path": "...", "old_string":
"...", "new_string": "..."})` instead. The orchestrator exposes
both as native function tools; calls dispatch directly to disk.
**Do NOT** emit fenced markdown code blocks with the path as the
info-string -- fenced bodies are NOT extracted in
native-tool-calls mode, the file never lands, and the gate
fails. If you don't remember the exact path, call
`read_file({"path": "..."})` or `list_dir({"path": "..."})` to
discover it before writing. See
`_conventions/orchestrator-native-tools.md` for the full rules.
