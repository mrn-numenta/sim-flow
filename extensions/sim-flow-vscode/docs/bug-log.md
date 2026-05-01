# sim-flow VS Code Bug Log

## Overview

This log tracks bugs found during dashboard/chat-panel testing and the
corresponding fixes. It is intentionally short and focused on
user-visible failures, root causes, and the tests that now guard
 against regression.

## 2026-05-01

### Round 1 - Stop/Resume Harness

- [x] Direct-response stop note could be lost after cancellation.
  - Symptom: stopping an in-flight direct panel reply could drop the
    "Stopping response" note from the persisted conversation.
  - Root cause: the direct-response path persisted a stale local
    conversation snapshot in `finally`, overwriting the stop note added
    concurrently.
  - Guard: `stops an in-flight direct panel reply without losing the
    stop note` in
    [src/mockFlowHarness.test.ts](../src/mockFlowHarness.test.ts).

- [x] `canStop` stayed true after a session had already ended or been
  stopped.
  - Symptom: the panel header and stop affordance still behaved as if a
    session were running after completion/cancellation.
  - Root cause: `activePump` was cleared only after posting the final
    state update.
  - Guard: stop/relaunch and resume scenarios in
    [src/mockFlowHarness.test.ts](../src/mockFlowHarness.test.ts).

### Round 2 - Mode-Switching Regression Tests

- [x] The chat panel stayed pinned to the old project during an active
  auto session.
  - Symptom: switching the active project while auto was running kept
    the panel on the old project's transcript and header state.
  - Root cause: panel context resolution preferred `activePump` project
    ownership over the current active project and did not reconcile
    project switches as explicit transitions.
  - Guard: `switches projects during an active auto session and stops
    the old session` in
    [src/mockFlowHarness.test.ts](../src/mockFlowHarness.test.ts).

- [x] LLM source changes did not stop and relaunch active auto
  sessions.
  - Symptom: changing `sim-flow.llm.source` during an active session
    left the old backend running instead of switching over.
  - Root cause: configuration change listeners only refreshed rendered
    state; they did not reconcile active session ownership or relaunch
    with the new backend settings.
  - Guard: `switches llm source by stopping and relaunching the active
    auto session` in
    [src/mockFlowHarness.test.ts](../src/mockFlowHarness.test.ts).

- [x] Duplicate Play in the same effective session surfaced an error
  instead of acting as a no-op.
  - Symptom: pressing Play again during the same active auto session
    produced a "Session already active" transcript error.
  - Root cause: duplicate launches were treated the same as conflicting
    launches; there was no equivalence check for same-project,
    same-source, same-spec sessions.
  - Guard: `ignores duplicate Play for the same active auto session` in
    [src/mockFlowHarness.test.ts](../src/mockFlowHarness.test.ts).

### Round 3 - Reload And CLI-Routing Regression Tests

- [x] API -> CLI source switch relaunched a panel session instead of
  moving the flow to the terminal path.
  - Symptom: changing `sim-flow.llm.source` from an API backend to a
    CLI backend during an active auto session spawned a second panel
    pump instead of transparently handing control to the terminal-based
    flow.
  - Root cause: source-switch reconciliation treated every backend
    change as another panel-session relaunch and did not branch to
    `sim-flow.runFlowTerminal` for terminal-only backends.
  - Guard: `switches from an api source to a cli source by stopping the
    panel session and routing to terminal` in
    [src/mockFlowHarness.test.ts](../src/mockFlowHarness.test.ts).

- [x] In-progress auto-session transcript output was lost after panel
  reload.
  - Symptom: reloading or closing the chat panel during an active
    orchestrated session restored only the initial launch note instead
    of the latest visible assistant output.
  - Root cause: incremental auto-session transcript updates were kept in
    memory but not persisted before provider disposal canceled the live
    pump.
  - Guard: `restores the latest visible auto-session transcript after
    provider reload` in
    [src/mockFlowHarness.test.ts](../src/mockFlowHarness.test.ts).

### Round 4 - Direct Reply Transition Regression Tests

- [x] Direct panel replies could restore stale project/source context
  after a project switch or LLM-source switch.
  - Symptom: after canceling a live direct reply because the active
    project or LLM source changed, the panel could snap back to the old
    project's label or the old source label once the canceled request
    unwound.
  - Root cause: the direct-response `finally` path always posted the
    original request context instead of re-reading the current active
    panel context after reconciliation.
  - Guard:
    - `switches projects during a direct panel reply and drops stale
      response context`
    - `switches llm source during a direct panel reply and stops the
      stale response`
    in
    [src/mockFlowHarness.test.ts](../src/mockFlowHarness.test.ts).

### Round 5 - Direct Reply Stop-Race Regression Tests

- [x] Repeated Stop presses during a direct reply appended duplicate
  cancellation notes.
  - Symptom: pressing Stop multiple times before the first cancellation
    fully unwound could add the same "Cancellation requested for the
    current model response." note more than once.
  - Root cause: the direct-response state had no "stop already
    requested" guard, so each Stop action appended a new note while the
    same in-flight request was still canceling.
  - Guard: `does not append duplicate stop notes when stop is pressed
    repeatedly during a direct reply` in
    [src/mockFlowHarness.test.ts](../src/mockFlowHarness.test.ts).

### Round 6 - Immediate Re-Prompt Transition Regression Tests

- [x] Immediate re-prompts after a project switch or LLM-source switch
  could be dropped during direct replies.
  - Symptom: if the user switched projects or changed the LLM source
    and immediately sent another chat message, the new prompt could be
    ignored and the panel would only show the stale-response stop note.
  - Root cause: `sendPrompt()` rejected all prompts while any
    direct-reply request was still marked `inFlight`, even if that
    request had already become stale and should have been cancelled by
    mode-switch reconciliation.
  - Guard:
    - `accepts a new prompt immediately after switching projects during
      a direct reply`
    - `accepts a new prompt immediately after switching llm sources
      during a direct reply`
    in
    [src/mockFlowHarness.test.ts](../src/mockFlowHarness.test.ts).
