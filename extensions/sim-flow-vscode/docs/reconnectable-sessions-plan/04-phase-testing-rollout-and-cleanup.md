# Phase 4 - Testing, Rollout, And Cleanup

## Milestone 1 - Automated Coverage

- [ ] Add manager-level tests for:
  launch, attach, reattach, dead-runtime detection, and cleanup.
- [ ] Upgrade the mocked dashboard/chat harness so reload scenarios
  assert true live reattach for reconnectable auto sessions rather than
  dead-transcript restore.
- [ ] Add regressions for reconnect during:
  streaming, awaiting-input, pending host-mediated LLM work, and
  session end.
- [ ] Add protocol-level tests for:
  stale session ids, attach version mismatch, malformed snapshot data,
  and runtime-side disconnect handling.

## Milestone 2 - Manual Smoke Coverage

- [ ] Reload the window during active streaming and verify the session
  resumes instead of downgrading to an interrupted transcript.
- [ ] Reload during awaiting-input and verify the next reply continues
  the same live flow.
- [ ] Verify project switch after reconnect still stops the old session
  and shows only the new project’s state.
- [ ] Verify source/model switch after reconnect still stops and
  relaunches cleanly on the new backend.
- [ ] Verify CLI-terminal flows remain unchanged by the reconnectable
  panel-session work.

## Milestone 3 - Documentation And Cleanup

- [ ] Update [mode-switching.md](../mode-switching.md) once true live
  reattach replaces the current dead-session restore contract.
- [ ] Update [testing-plan.md](../testing-plan.md) so reload/restore
  assertions move from “document current behavior” to “assert exact
  continuity”.
- [ ] Update [transition-graph.md](../transition-graph.md) to show live
  reattach rather than transcript-only restore.
- [ ] Update architecture docs if the extension/runtime boundary or
  host-session protocol changes materially.
- [ ] Narrow or remove the current dead restored-session UX once it is
  only needed for non-reconnectable modes.
