# sim-flow VS Code Transition Graph

## Overview

This diagram is a compact view of the panel/dashboard lifecycle that
the mocked regression harness is exercising. It focuses on the
transition-heavy edges where ownership, routing, or visible context can
change while work is already in flight.

Read this together with:

- [testing-plan.md](./testing-plan.md)
- [mode-switching.md](./mode-switching.md)

## Session Graph

```mermaid
stateDiagram-v2
  [*] --> Idle

  Idle --> DirectStreaming: send prompt\n(API source)
  Idle --> AutoStreaming: dashboard Play\n(API source)
  Idle --> TerminalRouted: dashboard Play\n(CLI source)

  DirectStreaming --> Idle: response ends
  DirectStreaming --> Idle: stop
  DirectStreaming --> Idle: reload/restore\n(currently transcript restore)

  AutoStreaming --> AwaitingInput: orchestrator requests input
  AutoStreaming --> Idle: session ends
  AutoStreaming --> Idle: stop
  AutoStreaming --> TerminalRouted: source switch API -> CLI
  AutoStreaming --> AutoStreaming: source switch API -> API\n(stop + relaunch)
  AutoStreaming --> Idle: reload/restore\n(currently transcript restore)

  AwaitingInput --> AutoStreaming: user reply
  AwaitingInput --> Idle: stop
  AwaitingInput --> TerminalRouted: source switch API -> CLI
  AwaitingInput --> AwaitingInput: source switch API -> API\n(stop + relaunch)
  AwaitingInput --> Idle: reload/restore\n(currently transcript restore)

  Idle --> Idle: duplicate Play\n(same effective session)

  state "Project/Source Reconciliation" as Reconcile {
    [*] --> ActiveContext
    ActiveContext --> ActiveContext: refresh
    ActiveContext --> ActiveContext: header/token update
    ActiveContext --> ActiveContext: tool/phase update
  }

  DirectStreaming --> Reconcile: active project changes
  AutoStreaming --> Reconcile: active project changes
  AwaitingInput --> Reconcile: active project changes
  Reconcile --> Idle: old session stopped,\nnew project becomes active

  DirectStreaming --> Reconcile: llm source changes
  AutoStreaming --> Reconcile: llm source changes
  AwaitingInput --> Reconcile: llm source changes
  Reconcile --> DirectStreaming: next direct prompt on new API source
  Reconcile --> AutoStreaming: auto session relaunched on new API source
  Reconcile --> TerminalRouted: rerouted to terminal on CLI source
```

## Intent

This graph encodes the current product decisions:

- dashboard and chat panel share one active project
- project switch implicitly stops the old session
- source switch stops the old session and relaunches or reroutes
- duplicate Play in the same effective session is a no-op
- reload currently restores transcript state, with true live reattach
  as a future target

## Current Coverage Themes

- direct reply lifecycle: stop, reload, project switch, source switch,
  immediate re-prompt
- auto session lifecycle: stop, relaunch, awaiting-input, source
  switch, project switch
- restore behavior: interrupted direct replies, interrupted auto
  sessions, delayed persistence writes, project-specific transcript
  isolation
- race handling: repeated Stop, duplicate Play, source switch plus
  immediate Play, project switch plus immediate Play

## Gaps To Keep In View

- true live session reattachment after reload
- deeper CLI-mode lifecycle graph and tests
- manual VS Code shell behaviors such as panel focus and placement
