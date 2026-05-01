# Phase 1 - Session Contract And Runtime Foundations

## Milestone 1 - Lock The Reconnect Contract

- [ ] Define the exact reconnectable-session scope in writing:
  automated panel sessions only, with direct panel chat and CLI mode
  explicitly out of scope for the first cut.
- [ ] Document the user-visible behavior for:
  panel close/open, extension reload, window reload, project switch,
  source/model switch, Stop, and natural session end.
- [ ] Define what “resume exactly where it left off” means for the
  first shipping version:
  transcript continuity, phase/tool/artifact continuity, awaiting-input
  continuity, and resumed event streaming.
- [ ] Define stale-session cleanup rules:
  when a reconnectable session is considered dead, how it is surfaced
  to the user, and when its persisted record is removed.

## Milestone 2 - Choose The Runtime Ownership Model

- [ ] Decide how a live orchestrator survives extension-host churn:
  detached child, durable helper process, or another explicit runtime
  host that is not owned by `ChatPanelProvider`.
- [ ] Define where reconnectable session metadata and discovery state
  live on disk:
  session id, socket path, runtime pid if needed, launch spec path,
  project, source, and model.
- [ ] Define lifecycle boundaries between the durable runtime and the
  extension:
  who creates the session, who can attach, who can cancel, and who is
  responsible for final cleanup.
- [ ] Define crash-recovery behavior for stale sockets, stale session
  records, and runtimes that ended while the extension was away.

## Milestone 3 - Extend The Session Protocol For Reattach

- [ ] Add a reconnect/attach handshake to the session protocol with a
  stable session id and protocol-version validation.
- [ ] Define the minimal snapshot the runtime must provide on attach:
  session tag, step descriptor, current phase/tool/artifact, pending
  awaiting-input state, pending request metadata, and terminal status.
- [ ] Define host-disconnect behavior while the runtime is waiting for:
  user input, host-mediated LLM dispatch, or normal event rendering.
- [ ] Decide whether the runtime replays a buffered event log, emits a
  compact state snapshot, or both when a host reattaches.
- [ ] Specify timeout and heartbeat behavior so reconnect failures do
  not leave the extension in an ambiguous half-attached state.
