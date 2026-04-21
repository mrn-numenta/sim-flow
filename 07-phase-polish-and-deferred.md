# Phase 7 - Polish, Guided Mode, And Deferred Work

Phase dependency: Phase 6 (DSF end-to-end). Pulls in capability/recommendation
ideas from [study-workflow-and-agent-adapters.md](../../architecture/study-workflow-and-agent-adapters.md)
and the deferred Phase 29 plan.

## Problem Statement

After Phase 6 the DMF and DSF are both usable end-to-end via the `sim-flow`
CLI. This phase finishes the UX polish (guided status recommendations,
session lifecycle hardening), adds the per-step client override surface,
captures the DM5 scoping work that was intentionally deferred, and
documents the framework-owned study-workflow concepts in a way that
future agent adapters can consume.

## Milestone 1 - Guided Status Recommendations

- [ ] Implement `sim-flow status` suggestion logic that proposes a next
  action based on current step, gate status, and available artifacts.
- [ ] Pull in the capability-advertisement and recommendation framing
  from the Phase 29 architecture doc: treat recommendations as a
  framework-owned query over state + artifacts, not LLM-inferred UX.
- [ ] Add tests covering: mid-DMF (DM2a incomplete), mid-DSF (2 of 3
  candidates passed DS5a), post-DSF-completion (DS9 passed, suggest
  DM0).

## Milestone 2 - Session Lifecycle Refinements

- [ ] Add per-step timeouts in `config.toml` under
  `[steps.<id>].timeout_seconds`.
- [ ] Implement timeout enforcement: kill the AI client subprocess
  and record the session as failed with a `TIMEOUT:` marker in the
  critique file.
- [ ] Implement Ctrl-C handling: propagate SIGINT to the subprocess,
  wait for graceful exit, then hard-kill after a grace period.
- [ ] Implement optional retry-on-failure (off by default): configurable
  retry count per step, skipping retries when the failure is a critique
  `BLOCKER:` vs a transient subprocess error.
- [ ] Document the lifecycle contract in doc 02.

## Milestone 3 - Per-Step Client Overrides

- [ ] Finalize the `[steps.<id>]` config.toml schema (client name,
  model, tool allowlist).
- [ ] Exercise an override in the end-to-end DMF and DSF validations
  (e.g., Claude for DM0-DM2, Codex for DM3).
- [ ] Document the override UX in the generated `CLAUDE.md` /
  `AGENTS.md`.

## Milestone 4 - DM5 External PPA Analysis

- [ ] Schedule a scoping session with the PPA flow engineer to define
  DM5 sub-sessions (DM5a analytical, DM5b SystemVerilog gen, DM5c
  external synthesis import).
- [ ] Update [02-direct-modeling-flow.md](../../architecture/ai-flow/02-direct-modeling-flow.md)
  DM5 section once scope is known.
- [ ] Author DM5 instruction files and gate checks.
- [ ] Wire DM5 into the Phase 3 DMF validation.
- [ ] Extend `experiments.db.ppa_estimates` write paths.

## Milestone 5 - Framework-Owned Workflow Concepts

- [ ] Port the useful ideas from
  [study-workflow-and-agent-adapters.md](../../architecture/study-workflow-and-agent-adapters.md)
  and the Phase 29 plan into the ai-flow architecture docs as
  first-class sections:
  - workflow-phase vocabulary (explore / compare / decide / promote)
  - capability advertisement surface
  - provenance / decision lineage query interface
- [ ] Mark the prior doc and Phase 29 plan as superseded once the
  content has a new home.
- [ ] Defer study-mode abstraction (research / hw-only / sw-only /
  hw-sw co-design) to a future phase; document the hook points so the
  extension is painless.

## Milestone 6 - Documentation Cleanup

- [ ] Ensure every architecture doc under
  [docs/architecture/ai-flow/](../../architecture/ai-flow/) reflects
  the shipped implementation.
- [ ] Add a top-level AI-flow overview in the main
  [architecture.md](../../architecture/architecture.md) TOC entry so
  discoverability is obvious.
- [ ] Retire the staged AGENTS.md / CLAUDE.md content in
  `sim-models/.codex-staging/` if it duplicates shipped templates.
- [ ] Update `CHANGELOG.md` at each milestone close.

## Milestone 7 - Adoption And Validation

- [ ] Drive one real-world LPDDR5X-adjacent model through the DMF end
  to end and log issues.
- [ ] Drive one real-world design study (small NoC or memory
  controller sizing) through the DSF end to end and log issues.
- [ ] Fold issues into follow-up milestones rather than patching the
  architecture mid-flight.

## Status

Not started. Gated on Phase 6. Milestone 4 (DM5) can proceed independently
once the PPA engineer is available.
