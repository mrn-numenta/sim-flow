# Phase 3 - Chat Panel Integration And User Experience

## Milestone 1 - Make The Panel A Thin Renderer Over The Manager

- [ ] Replace direct `activePump` ownership in `ChatPanelProvider` with
  manager-backed session observation and command forwarding.
- [ ] On `resolveWebviewView`, attempt to reattach to any live session
  for the active project before falling back to persisted dead-session
  transcript behavior.
- [ ] Preserve existing ordered post-state and conversation persistence
  behavior while moving live ownership to the manager.
- [ ] Ensure panel refresh, editor-change, and configuration-change
  flows all read from the manager’s live session state when available.

## Milestone 2 - Reattach UX

- [ ] Add a visible “reconnecting” state for the panel while attach is
  in progress.
- [ ] Restore the header state from live session data:
  phase, tool, artifact, stop availability, session label, and
  awaiting-input notice.
- [ ] Restore prompt-entry gating correctly after attach:
  disabled while streaming, enabled when awaiting input, disabled when
  the runtime is gone.
- [ ] Keep transcript continuity predictable:
  reuse persisted transcript state for already-rendered content and
  append only genuinely new live events after reattach.

## Milestone 3 - Failure And Edge UX

- [ ] Distinguish “reattaching”, “reattach failed”, “session ended while
  you were away”, and “session no longer live” in the panel notice
  text.
- [ ] Ensure Stop during or immediately after reconnect behaves
  predictably and cannot strand the panel in a false live state.
- [ ] Ensure clear-transcript continues to behave safely for dead
  sessions and remains blocked for live ones.
- [ ] Preserve the current project-specific isolation rules during
  reconnect so no stale state from another project can leak into the
  panel.
