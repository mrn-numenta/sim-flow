### JSON schema

```json
{
  "step": "{{ step_id }}",
  "summary": "1-paragraph summary of the critique outcome.",
  "findings": [
    {
      "kind": "blocker",
      "section": "free-form section name (e.g. \"External Interfaces\")",
      "title": "one-line summary of the finding",
      "body": "multi-line markdown explanation; quote offending lines, list remediation"
    }
  ],
  "notes": "optional free-form trailing prose"
}
```

`kind` values: `"blocker"` (gate-blocking), `"unresolved"`
(also gate-blocking until resolved), `"resolved"`
(informational; ignored by the gate). The schema is strict
(`deny_unknown_fields`); typos fail the parse and the
orchestrator surfaces "malformed critique JSON". Use the exact
field names listed.
