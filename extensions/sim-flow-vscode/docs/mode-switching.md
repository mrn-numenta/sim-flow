# sim-flow VS Code Mode Switching Decisions

## Overview

This document records the current product decisions for project,
session, and backend mode switching in the sim-flow VS Code extension.
It exists so the team can:

- keep the implemented behavior aligned with explicit decisions
- test against a stable contract
- revisit decisions later without losing the original rationale

These decisions reflect the current intended behavior as of
2026-05-01.

## Decision Summary

### Active Project Ownership

Decision:

- The dashboard and chat panel must always stay in sync on the active
  project.
- Activating a new project switches both surfaces immediately.

Implications:

- The chat panel is never allowed to keep showing a stale project after
  the dashboard has switched.
- The dashboard is never allowed to remain on a stale project after the
  chat panel has switched.

### Project-Scoped Chat Context

Decision:

- Chat context is project-specific.
- Messages, token totals, phase/tool/artifact header state, and prompt
  context from project A must never appear in or contribute to project
  B.

Implications:

- Conversation persistence must remain keyed by project.
- Prompt construction must only use the active project's transcript.

### Project Switch During Active Work

Decision:

- Switching projects implicitly stops the old session.
- The newly activated project becomes the only visible and active
  context in the chat panel.

Implications:

- We do not keep a hidden active session running for the previous
  project after a project switch.
- Stop behavior on switch must be clean and predictable for both direct
  panel replies and orchestrated auto sessions.

### LLM Source Switching

Decision:

- Switching `sim-flow.llm.source` should be as seamless as possible.
- If the source changes during an active session, stop the old session
  and transparently relaunch on the new source.

Implications:

- API -> API switching should move the user onto the new backend with
  minimal friction.
- API -> CLI and CLI -> API switching must preserve correct routing and
  keep the legacy CLI flow working.
- Source changes must not leave the panel in an ambiguous mixed-backend
  state.

### Reload And Restore

Decision:

- The ideal behavior is to start exactly where the user left off after
  reload.

Implications:

- Persisted transcript restore is necessary but not sufficient.
- The longer-term target is true session reattachment for active work,
  not merely restoring static history.

### Duplicate Play

Decision:

- If Play is pressed and the extension is already in the same effective
  session that Play would launch, ignore the request.

Implications:

- Duplicate Play must not spawn duplicate sessions.
- Duplicate Play must not reset transcript state or header state.

### Awaiting-Input Project Switch

Decision:

- Awaiting-input state does not block project switching.
- The extension should switch immediately.

Implications:

- We do not require the user to finish or dismiss an awaiting-input
  session before switching projects.
- The old session is stopped as part of the switch, consistent with the
  project-switch rule above.

## Rationale

These decisions favor:

- one unambiguous active project across extension surfaces
- strong project-context isolation
- predictable interruption behavior
- minimal user friction when changing backend configuration
- compatibility with the existing CLI flow while the panel flow evolves

They intentionally reject:

- stale cross-project chat context
- background hidden sessions continuing after a project switch
- duplicate sessions from repeated Play actions
- backend switching that leaves ownership unclear

## Testing Consequences

The test plan in
[testing-plan.md](./testing-plan.md)
should continue to assert these decisions explicitly, especially for:

- project switch during streaming
- project switch during awaiting input
- backend switch during active work
- duplicate Play behavior
- reload and restore continuity

## Revision Notes

If the team changes any of these decisions later, update this file with:

- the new decision
- the date of the change
- the reason for the change
- the tests that must be updated as a result
