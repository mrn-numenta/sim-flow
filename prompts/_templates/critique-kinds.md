Record findings in the critique JSON (see "Output" below for the
schema). Each finding's `kind` MUST be one of three exact
lowercase values:

- `"blocker"` -- gate-blocking; the downstream step cannot
  proceed until this is fixed.
- `"unresolved"` -- open follow-up; also blocks the gate until
  resolved. Use this for real gaps that are safely deferrable
  but not yet addressed.
- `"resolved"` -- informational acknowledgement or retry-mode
  trace; ignored by the gate.

The orchestrator fails the gate on EVERY `"blocker"` AND
`"unresolved"` finding; only `"resolved"` lets the gate pass.

The schema is strict (`deny_unknown_fields` with a lowercase
enum). Synonyms or near-misses like `"warning"`, `"issue"`,
`"open"`, `"info"`, `"note"`, `"concern"`, `"todo"`, `"blocked"`,
or capitalised forms (`"Blocker"`, `"BLOCKER"`) all fail the
parse and the run reports "malformed critique JSON". Map every
finding to one of the three values above; do not invent new
ones.
