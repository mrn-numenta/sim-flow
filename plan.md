# AI-Flow Implementation Plan

## Overview

This plan implements the AI-assisted modeling flow defined in the architecture
documents under [docs/architecture/ai-flow/](../../architecture/ai-flow/). It
delivers the `sim-flow` orchestrator, project templates, experiment tracking,
per-step instruction files, and IDE / CLI hosts required to drive both the
Direct Modeling Flow (DMF) and the Design Study Flow (DSF) against the
sim-foundation framework.

Planning assumptions:

1. **sim-foundation owns orchestration, schemas, templates, and AI
   instructions.** sim-models owns user model code. The `sim-flow`
   orchestrator drives every session - it loads instructions, assembles
   messages, dispatches LLM calls (via a host or its own CLI-agent
   wrapper), validates artifacts, and advances state. Frontends are
   *renderers* over a host-neutral session protocol; they do not own
   step semantics.
2. Every step runs as a work + critique session pair. The critique runs
   as a fresh, independent AI session and writes a critique file that
   the orchestrator scans for `UNRESOLVED:` / `BLOCKER:` lines.
3. A project is created once via `cargo generate`. A design study and
   the resulting DMF work live in the same project -- DS9 is an in-place
   state transition, not a new project.
4. Implementation delivers the DMF end-to-end first, then layers DSF on
   top. Experiment tracking lands between the analysis-light DMF steps
   (DM0-DM3) and the analysis-heavy DMF step (DM4). The VS Code
   extension was the first frontend; Phase 9 pivots it to a renderer
   over the orchestrator-driven session protocol.
5. Phase ordering reflects dependency order, not calendar order.
6. The architecture documents are the source of truth. Where the plan
   and the architecture diverge, the architecture wins -- update the
   architecture first, then align the plan.

## Architecture pivot at Phase 9

Phases 1-8 produced a working orchestrator core (`sim-flow` CLI) plus a
chat-driven VS Code extension that owned much of the session
orchestration in TypeScript. A retrospective audit (recorded in the
Phase 8 status block and the architecture revision in
[06-vscode-extension.md](../../architecture/ai-flow/06-vscode-extension.md))
identified four classes of issues with that approach: step knowledge
duplicated across Rust and TS, no command to advance state after a
gate passes, no first-class tool concept for code-authoring steps, and
no way to host the same flow from a non-VS-Code IDE or from the
terminal without re-implementing orchestration.

Phase 9 commits to **orchestrator as master**: sim-flow drives every
session via a host-neutral JSONL protocol. The VS Code extension
becomes a renderer; a TerminalHost lets sim-flow run sessions from a
plain terminal using subscription-backed CLI agents (`claude`,
`codex`); other IDEs implement the same protocol to host the flow.
See [09-phase-orchestrator-driven-sessions.md](./09-phase-orchestrator-driven-sessions.md)
for the staged migration.

## Table of Contents

- [x] [Phase 1 - Orchestrator Core](./01-phase-orchestrator-core.md)
- [x] [Phase 2 - Project Templates And Model-Project Bootstrap](./02-phase-project-templates.md)
- [x] [Phase 3 - Direct Modeling Flow Step Implementation](./03-phase-dmf-implementation.md)
- [x] [Phase 4 - Experiment Tracking](./04-phase-experiment-tracking.md)
- [ ] [Phase 5 - Design Study Flow Templates And Orchestrator Support](./05-phase-dsf-templates.md)
- [ ] [Phase 6 - Design Study Flow Step Implementation](./06-phase-dsf-implementation.md)
- [ ] [Phase 7 - Polish, Guided Mode, And Deferred Work](./07-phase-polish-and-deferred.md)
- [/] [Phase 8 - VS Code Extension (chat-driven, superseded)](./08-phase-vscode-extension.md)
- [ ] [Phase 9 - Orchestrator-Driven Sessions And Multi-Host Support](./09-phase-orchestrator-driven-sessions.md)
- [ ] [Phase 10 - Multi-Model Adaptation And Runtime Profiles](./10-phase-multi-model-adaptation-and-runtime-profiles.md)
