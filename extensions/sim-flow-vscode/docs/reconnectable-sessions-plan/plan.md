# Reconnectable Sessions Plan

## Overview

This plan covers implementation of reconnectable `sim-flow` sessions in
the VS Code extension so the chat panel can resume active automated
flows after a webview disposal, extension reload, or window reload.

Today, active panel sessions are owned directly by
`ChatPanelProvider`. On dispose, the provider persists the latest
visible transcript, appends an interruption note, and tears down the
live `SessionPump`. That gives us safe transcript restore, but not true
continuity. The intended target recorded in
[mode-switching.md](../mode-switching.md) and
[testing-plan.md](../testing-plan.md) is stronger: the user should be
able to return to the same live flow session rather than only seeing a
dead restored transcript.

Initial scope:

- panel-driven automated `sim-flow auto` sessions launched from the
  dashboard or chat panel
- project-scoped reattachment for API-backed panel sessions
- continuity across webview disposal and extension/window reload

Explicit non-goals for the first implementation:

- reconnecting freeform direct panel chat replies
- changing the existing CLI-terminal lifecycle contract
- background hidden sessions surviving project switches

The implementation should preserve the product rules already locked in:

- dashboard and chat panel stay in sync on the active project
- project switch still implies stop
- source/model switch still implies stop plus relaunch
- duplicate Play remains a no-op for the same effective session

## Table Of Contents

- [Phase 1 - Session Contract And Runtime Foundations](./01-phase-session-contract-and-runtime-foundations.md)
- [Phase 2 - Extension Session Manager And Reattach Transport](./02-phase-extension-session-manager-and-reattach-transport.md)
- [Phase 3 - Chat Panel Integration And User Experience](./03-phase-chat-panel-integration-and-user-experience.md)
- [Phase 4 - Testing, Rollout, And Cleanup](./04-phase-testing-rollout-and-cleanup.md)
