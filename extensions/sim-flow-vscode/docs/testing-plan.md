# sim-flow VS Code Testing Plan

## Overview

This document captures the test strategy for the sim-flow VS Code
dashboard and chat panel, with emphasis on pain points, transition
states, user actions at inconvenient times, and behavioral handoffs
between extension surfaces.

The mode-switching product decisions that this test plan depends on are
recorded separately in [mode-switching.md](./mode-switching.md).

The core principle is that the most expensive bugs are not simple
rendering issues. They appear when ownership, project context, session
state, or backend selection changes while work is already in flight.
The test plan therefore prioritizes:

- dashboard -> chat panel handoff
- project A -> project B switching
- API-backed panel flow vs CLI-backed terminal flow routing
- idle -> streaming -> awaiting-input -> resumed -> ended transitions
- stop, reload, relaunch, and source-switch actions during active work

The extension must preserve the existing working CLI flow while adding
the panel-driven flow for API-backed backends.

For now, the primary regression focus is the chat-panel-backed API
path. CLI-backed terminal flow is documented as a separate test track so
we do not accidentally apply panel-specific expectations to terminal
ownership and reload behavior.

## Behavioral Contract

The tests in this plan assume the following user-visible behavior:

- [x] The dashboard and chat panel always stay in sync on the active
  project.
- [x] Switching projects immediately changes both surfaces to the new
  project.
- [x] Chat history, header state, token counts, and prompt context are
  project-specific. Messages from one project must never be shown in or
  contribute to another project's context.
- [x] Switching projects during an active session implicitly stops the
  old session.
- [x] Switching `sim-flow.llm.source` during an active session stops
  the old session and transparently relaunches on the new source.
- [x] Reload should ideally restore the exact prior state, including
  active-session continuity when implemented.
- [x] Pressing Play while already in the same effective session is a
  no-op.
- [x] Awaiting-input state does not block project switching.
- [x] CLI-backed sources continue to use the terminal-based flow and
  must not regress while the panel flow evolves.

## Test Layers

### Unit Tests

These cover small, deterministic logic with no extension host wiring.

- [x] Step-action gating and dashboard button visibility
- [x] Chat-panel transcript state reducers
- [x] Markdown rendering and whitespace preservation
- [x] Token estimation and per-entry accounting
- [x] Pump markdown classification and filtering of presentation-only
  notes
- [ ] Active-project and source-switch helper logic once extracted into
  pure helpers

### Mocked Integration Tests

These are the main regression net for dashboard/chat behavior. They use
mocked VS Code surfaces, mocked LLM responses, and mocked orchestrator
sessions so the extension can be exercised end to end without manual
reload cycles.

Current harness:

- [x] [src/mockFlowHarness.test.ts](../src/mockFlowHarness.test.ts)
- [x] Persistent fixture projects under
  [testdata/mock-flow](../testdata/mock-flow)

Current covered scenarios:

- [x] Full mocked dashboard -> chat auto flow for `example` through
  DM4b
- [x] Project switching without transcript mixing
- [x] Resume after orchestrator `awaiting-input`
- [x] Stop and relaunch of a running auto session
- [x] Stop of an in-flight direct panel response

Current emphasis:

- [x] Chat-panel-backed API session lifecycle
- [/] API <-> CLI routing boundaries where they affect panel behavior
- [ ] Full CLI-mode lifecycle contract, to be covered as a separate
  follow-on track

### Manual Smoke Tests

These are reserved for behavior that mocks do not fully prove yet.

- [ ] Real VS Code panel placement and focus behavior
- [ ] Real extension reload / window reload behavior
- [ ] Real backend connectivity and authentication flows
- [ ] Terminal lifecycle and control-socket behavior for CLI sources
- [ ] Visual compactness and readability in the actual panel shell

## Persistent Fixtures

The test suite uses stable fixture projects so failures are easier to
reproduce and extend.

- [x] `example`
  - 1 GHz
  - 7 nm
  - 3-stage pipeline
  - 4-byte RGBA input
  - grayscale conversion
  - BGRA output with first three bytes swapped into B,G,R order
- [x] `other-project`
  - smaller alternate project used for context-switch isolation tests

## Phase 1 - Context Ownership And Surface Synchronization

### Milestone 1 - Active Project Synchronization

- [ ] Switching projects updates both dashboard and chat panel to the
  same project in the same refresh cycle.
- [ ] Dashboard Play for project B while project A is visible switches
  the chat panel to project B immediately.
- [ ] Header state (`projectLabel`, `currentStep`, phase, tool,
  artifact, token totals) updates to the new project without showing
  stale values from the previous project.
- [ ] Clearing transcript in one project does not affect another
  project's stored transcript.

### Milestone 2 - Context Isolation

- [ ] Prompt construction for project B never includes messages from
  project A.
- [ ] Persisted workspace state remains keyed per project.
- [ ] Switching away from a project hides its transcript immediately.
- [ ] Returning to a project restores only that project's transcript and
  derived header state.
- [ ] Project-specific token totals do not leak across projects.

### Milestone 3 - Inconvenient Project Switches

- [ ] Switch project while direct panel response is streaming.
- [ ] Switch project while orchestrated auto session is streaming.
- [ ] Switch project while the orchestrator is awaiting input.
- [ ] Switch project while the final chunk or completion note is being
  posted.
- [ ] Verify that switching projects implicitly stops the old session.

## Phase 2 - Session Lifecycle And Handoffs

### Milestone 1 - Core State Machine

- [ ] Cover all panel lifecycle transitions:
  - `idle -> streaming`
  - `streaming -> awaiting-input`
  - `awaiting-input -> streaming`
  - `streaming -> ended`
  - `streaming -> cancelled`
  - `awaiting-input -> cancelled`
  - `ended -> relaunched`
- [ ] Verify `canStop`, notice text, prompt enablement, and session
  label in each state.

### Milestone 2 - Duplicate Or Redundant User Actions

- [ ] Duplicate Play in the same effective session is ignored.
- [ ] Rapid repeated Play clicks do not create duplicate sessions.
- [ ] Repeated Stop clicks do not corrupt transcript state or append
  duplicate terminal notes.
- [ ] Send prompt while already streaming is ignored or blocked
  cleanly.
- [ ] Clear transcript while a session is active is blocked cleanly.

### Milestone 3 - Dashboard <-> Chat Handoffs

- [ ] Dashboard Play opens or focuses the chat panel on the correct
  project and session.
- [ ] Stop from the chat panel leaves dashboard state coherent after
  refresh.
- [ ] Dashboard project switch changes the panel immediately.
- [ ] Chat-panel-driven project switch changes the dashboard
  immediately.

## Phase 3 - LLM Source Switching And Routing

### Milestone 1 - API Source Switching

- [ ] Switch API source while idle and verify the next prompt uses the
  new source.
- [ ] Switch API source while direct response is streaming; old session
  stops and the next prompt runs on the new source.
- [ ] Switch API source while orchestrated auto session is running; old
  session stops and transparently relaunches from the current step on
  the new source.
- [ ] Header source label and backend-specific notice text update
  immediately when the source changes.

### Milestone 2 - CLI Compatibility And Routing

- [ ] API-backed sources route dashboard Play into the chat panel.
- [ ] CLI-backed sources route dashboard Play into the existing
  terminal flow.
- [ ] Switching from API -> CLI reverts Play to terminal routing and
  adjusts panel prompt-entry behavior coherently.
- [ ] Switching from CLI -> API restores panel-driven chat and Play
  routing.
- [ ] Existing terminal/control-socket flow continues to work without
  regression.

Scope note:

- The near-term goal here is to verify correct routing boundaries and
  safe handoff between panel mode and CLI mode.
- Deep CLI lifecycle expectations should be tested in a separate CLI
  track rather than inferred from panel-mode tests.

### Milestone 3 - Duplicate Session Detection

- [ ] If Play is pressed for the same project and same effective
  session, ignore the request.
- [ ] If Play is pressed for a different project while one session is
  active, the old session stops and the new project becomes active.
- [ ] If Play is pressed after a source switch, ensure relaunch occurs
  on the new source, not the stale one.

## Phase 4 - Reload, Restore, And Persistence

### Milestone 1 - Restoring Inactive State

- [ ] Reload with no active session restores the correct project's
  transcript, token counts, and header pills.
- [ ] Switching projects after reload restores isolated state for each
  project.
- [ ] Legacy presentation-only notes remain filtered after reload.

### Milestone 2 - Restoring Active State

- [ ] Reload during direct-response streaming.
- [ ] Reload during orchestrated session streaming.
- [ ] Reload during orchestrator `awaiting-input`.
- [ ] Document current behavior for each case and promote tests to
  assert exact reattach once reconnectable sessions are implemented.

### Milestone 3 - Exact Continuity

- [ ] Restore exactly where the user left off after reload.
- [ ] Reattach to active panel-driven sessions instead of only showing
  persisted transcript state.
- [ ] Preserve project-specific active-session ownership across reload.

## Phase 5 - Output Correctness And Signal Quality

### Milestone 1 - Transcript Quality

- [ ] Normal assistant output remains visible in the transcript.
- [ ] Tool fences and internal tool-call payloads stay hidden.
- [ ] Phase and tool churn remain in the header rather than cluttering
  the transcript.
- [ ] Whitespace survives chunked streaming.
- [ ] Markdown rendering works for headings, lists, blockquotes, code
  fences, inline code, emphasis, and safe links.

### Milestone 2 - Token And Metadata Accuracy

- [ ] Per-prompt input token estimate is shown on the correct entry.
- [ ] Response token estimate is tracked per assistant turn.
- [ ] Total up/down token counts in the header stay project-specific
  and correct across switches, stops, relaunches, and reloads.
- [ ] Phase, tool, and artifact header pills clear correctly when a
  session ends or is stopped.

## Phase 6 - Error And Recovery Paths

### Milestone 1 - Missing Or Invalid Context

- [ ] No active project can be resolved.
- [ ] Spec path is missing or invalid.
- [ ] Flow-state read fails.
- [ ] Workspace has multiple projects but no usable active selection.

### Milestone 2 - Backend Failures

- [ ] API key missing.
- [ ] Backend unavailable or connection refused.
- [ ] LLM error before any text arrives.
- [ ] LLM error after partial text arrives.
- [ ] Unsupported source selected in the panel.

### Milestone 3 - Orchestrator Failures

- [ ] Pump returns malformed or unexpected markdown/event output.
- [ ] Orchestrator cancels a session.
- [ ] Orchestrator ends without visible assistant content.
- [ ] Auto session is stopped during a gate or step transition.

## Immediate Priorities

The next highest-value additions are:

- [ ] Project switch during active streaming
- [ ] Project switch during `awaiting-input`
- [ ] Duplicate Play no-op for same project/session
- [ ] API -> API source switch during active use
- [ ] API -> CLI and CLI -> API routing regressions
- [ ] Reload / restore per-project transcript state
- [ ] Reload during active session, with current-vs-target behavior
  captured explicitly

Near-term focus note:

- Continue driving these priorities through the chat-panel-backed API
  path first.
- Return to the CLI-specific lifecycle track after the panel-mode
  contract is better covered.

## Notes

- Mocked integration coverage should remain the primary debug loop for
  dashboard/chat work.
- Manual smoke tests should be short and purposeful, not the main
  development loop.
- Once reconnectable sessions exist, reload/restore tests should be
  upgraded from "document current behavior" to "assert exact
  continuity."
