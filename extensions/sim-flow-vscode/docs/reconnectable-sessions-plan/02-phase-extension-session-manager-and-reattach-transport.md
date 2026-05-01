# Phase 2 - Extension Session Manager And Reattach Transport

## Milestone 1 - Introduce A Workspace-Scoped Session Manager

- [x] Move ownership of panel auto sessions out of
  `ChatPanelProvider` into a workspace-scoped service created from the
  extension activation path.
- [x] Define the manager’s public API:
  launch, attach, observe state, send user input, stop, dispose, and
  forget dead sessions.
- [/] Persist a reconnectable session record per project in
  `workspaceState`:
  session id, project dir, spec path, source, model, launch mode,
  last-known live status, and any attach metadata needed on restore.
  Current implementation persists project, source, model, spec path,
  awaiting-input status, and update time; stable session identity and
  attach metadata are still pending.
- [x] Ensure the manager, not the webview provider, is the single owner
  of live session state and attach status.

## Milestone 2 - Implement The Reattach Transport

- [ ] Add an extension-side client for the runtime’s reconnectable
  socket or equivalent transport.
- [ ] Support first attach and later reattach through the same client
  abstraction.
- [ ] Route orchestrator events, awaiting-input prompts, session-end
  notices, and host-mediated LLM requests through the manager instead
  of directly through `ChatPanelProvider`.
- [ ] Ensure only one active extension attachment owns a reconnectable
  session at a time.
- [ ] Ensure attach failures downgrade cleanly to “session no longer
  live” rather than hanging the panel.

## Milestone 3 - Preserve Existing Reconciliation Rules

- [x] Keep project-switch behavior authoritative:
  switching projects still stops the old reconnectable session and
  clears its live attachment.
- [x] Keep source/model switching authoritative:
  switching backends still stops the old session and relaunches on the
  new configuration instead of attempting mixed-backend migration.
- [x] Keep duplicate Play handling intact:
  if the requested launch matches the active reconnectable session,
  ignore it.
- [x] Keep CLI routing behavior intact:
  reconnectable manager logic must not accidentally take over
  terminal-backed flows.
