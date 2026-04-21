# Phase 1 - Orchestrator Core

Phase dependency: the ai-flow architecture documents in
[docs/architecture/ai-flow/](../../architecture/ai-flow/).

## Problem Statement

The `sim-flow` orchestrator is the entry point for every AI-assisted flow
action. It owns state, config, AI client invocation, critique-file parsing,
and gate validation. None of these pieces exist yet. This phase delivers a
flow-agnostic orchestrator crate that can run a single step's work + critique
session pair, validate the resulting artifacts and critique file against a
gate descriptor, and update state. DMF- and DSF-specific step definitions
are layered on top in Phases 3 and 6.

## Milestone 1 - Crate Skeleton And CLI

- [ ] Add `crates/sim-flow/` to the workspace with `clap`, `toml`, `serde`,
  `regex`, and `rusqlite` dependencies.
- [ ] Implement the top-level CLI with subcommands: `init`, `status`, `run`,
  `gate`, `reset`, `config`, `new`.
- [ ] Add `--foundation-root` global flag and `SIM_FOUNDATION_ROOT` env var
  resolution with walk-up fallback from the binary location.
- [ ] Wire `sim-flow --version` to the crate version and emit structured
  exit codes (`0` success, non-zero categorized failures).

## Milestone 2 - State Management

- [ ] Define the `state.toml` schema covering `flow`, `current_step`,
  `started`, and the `[gates]` table.
- [ ] Implement load/save with atomic writes (write-temp, fsync, rename).
- [ ] Implement forward-transition validation (prerequisite gate must pass).
- [ ] Implement back-transition behavior that resets the re-entered step
  and every downstream gate to `passed = false`.
- [ ] Add a `[gates.ds]` preservation path so DS9 can flip `flow` from
  `design-study` to `direct-modeling` without losing DSF gate history.
- [ ] Add unit tests covering load, save, round-trip, forward gate,
  back-transition cascade, and the DS-to-DM flip.

## Milestone 3 - Config Management

- [ ] Define the `.sim-flow/config.toml` schema matching doc 02, including
  `[client]`, `[client.claude]`, `[client.codex]`, `[client.copilot]`, and
  optional `[steps.<id>]` overrides.
- [ ] Implement load-with-precedence: `config.toml` > CLI flags > env vars.
- [ ] Surface effective config via `sim-flow config show`.
- [ ] Add unit tests for precedence, missing file defaults, and per-step
  client override resolution.

## Milestone 4 - AI Client Abstraction

- [ ] Define a `Client` trait with `invoke(prompt, instructions, tools)`
  returning exit status, stdout, and stderr.
- [ ] Implement `clients/claude.rs` wrapping the Claude Code CLI. Resolve
  the correct system-prompt flag at implementation time against the
  installed Claude Code version.
- [ ] Implement `clients/codex.rs` using `codex exec` with workspace-write
  sandbox and `AGENTS.md` instruction injection.
- [ ] Implement `clients/copilot.rs` using the Copilot CLI with
  `--allow-all-tools`.
- [ ] Add a `clients/mock.rs` fixture for deterministic tests.
- [ ] Write integration tests that invoke the mock client through the
  full `sim-flow run` path without needing a real LLM.

## Milestone 5 - Work + Critique Session Execution

- [ ] Implement `StepRunner` that loads a step descriptor (prompt file,
  gate checks, instructions) and runs the work session followed by a
  fresh critique session.
- [ ] Confirm the critique session is spawned as a brand-new client
  invocation with no shared conversation state.
- [ ] Pass the critique session a pointer to the artifacts produced by
  the work session (file list, directory paths) rather than the work
  session's transcript.
- [ ] Surface session stdout/stderr through the CLI with per-session log
  files under `.sim-flow/logs/<step>-{work,critique}-<timestamp>.log`.

## Milestone 6 - Critique Parsing And Gate Validation

- [ ] Implement the critique-file parser: any line whose first
  non-whitespace token is `UNRESOLVED:` or `BLOCKER:` triggers a gate
  failure; `RESOLVED:` is informational.
- [ ] Define a `GateCheck` trait covering file-exists, regex-match,
  shell-command (e.g., `cargo build`), and critique-scan variants.
- [ ] Implement the gate runner that evaluates every check for a step
  and reports the set of failures.
- [ ] Ensure `sim-flow gate <step>` runs gate validation only (no
  session spawn) so users can re-check after manual fixes.
- [ ] Add unit tests covering each check variant and happy/sad paths.

## Milestone 7 - Instructions Directory

- [ ] Create `sim-foundation/instructions/` as the canonical location for
  step prompts and critique prompts.
- [ ] Define the filename convention (`dm0-specification.md`,
  `dm0-critique.md`, etc.) and the prompt-file format (YAML/TOML frontmatter
  for metadata, markdown body for the prompt).
- [ ] Implement a loader that resolves instruction files from
  `<foundation-root>/instructions/` and fails loudly if a requested file
  is missing.
- [ ] Add a single placeholder instruction pair (`smoke-work.md`,
  `smoke-critique.md`) used by the end-to-end smoke test in Milestone 8.

## Milestone 8 - End-To-End Smoke Test

- [ ] Add an integration test that runs a dummy step through the full
  pipeline (state, config, mock client, work session, critique session,
  gate checks) inside a tempdir.
- [ ] Verify gate failure paths: missing artifact, failing shell check,
  `BLOCKER:` in critique.
- [ ] Verify back-transition resets downstream gates correctly across a
  multi-step sequence.

## Status

Not started.
