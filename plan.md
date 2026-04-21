# AI-Flow Implementation Plan

## Overview

This plan implements the AI-assisted modeling flow defined in the architecture
documents under [docs/architecture/ai-flow/](../../architecture/ai-flow/). It
delivers the `sim-flow` orchestrator, project templates, experiment tracking,
and per-step instruction files required to drive both the Direct Modeling Flow
(DMF) and the Design Study Flow (DSF) against the sim-foundation framework.

Planning assumptions:

1. sim-foundation owns orchestration, schemas, templates, and AI instructions.
   sim-models owns user model code. The orchestrator invokes AI clients
   (Claude, Codex, Copilot) non-interactively; there are no slash commands.
2. Every step runs as a work + critique session pair. The critique runs as a
   fresh, independent AI session and writes a critique file that the
   orchestrator scans for `UNRESOLVED:` / `BLOCKER:` lines.
3. A project is created once via `cargo generate`. A design study and the
   resulting DMF work live in the same project -- DS9 is an in-place state
   transition, not a new project.
4. Implementation delivers the DMF end-to-end first, then layers DSF on top.
   Experiment tracking lands between the analysis-light DMF steps (DM0-DM3)
   and the analysis-heavy DMF step (DM4).
5. Phase ordering reflects dependency order, not calendar order.
6. The architecture documents are the source of truth. Where the plan and
   the architecture diverge, the architecture wins -- update the architecture
   first, then align the plan.

## Table of Contents

- [/] [Phase 1 - Orchestrator Core](./01-phase-orchestrator-core.md)
- [/] [Phase 2 - Project Templates And Model-Project Bootstrap](./02-phase-project-templates.md)
- [ ] [Phase 3 - Direct Modeling Flow Step Implementation](./03-phase-dmf-implementation.md)
- [ ] [Phase 4 - Experiment Tracking](./04-phase-experiment-tracking.md)
- [ ] [Phase 5 - Design Study Flow Templates And Orchestrator Support](./05-phase-dsf-templates.md)
- [ ] [Phase 6 - Design Study Flow Step Implementation](./06-phase-dsf-implementation.md)
- [ ] [Phase 7 - Polish, Guided Mode, And Deferred Work](./07-phase-polish-and-deferred.md)
