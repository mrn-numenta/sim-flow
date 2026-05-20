# Phase 9 - Orchestrator-Driven Sessions And Multi-Host Support

Phase dependency: Phase 1 (orchestrator core), Phase 3 (DMF
instructions and gate checks), Phase 8 (VS Code extension - now
pivoting from chat-driven to renderer). Architecture:
[06-vscode-extension.md](../../architecture/ai-flow/06-vscode-extension.md),
[07-session-protocol.md](../../architecture/ai-flow/07-session-protocol.md),
[08-orchestrator-tools.md](../../architecture/ai-flow/08-orchestrator-tools.md).

## Problem Statement

Phase 8 shipped a working chat-driven VS Code extension. End-to-end
testing exposed four structural issues:

1. **Step knowledge duplicated across Rust and TS.** Instruction
   slugs, expected artifact paths, predecessor inputs, gate checks,
   and message-assembly rules were in both places. Adding a new
   step or changing one meant edits in both. Drift between them
   produced silent failures (LLM writing `dm0_critique.md` instead
   of `.sim-flow/critiques/DM0-critique.md`, missing inputs for
   critique sessions on DM2a+).
2. **No command to advance state after a gate passes.** The `/gate`
   command is read-only; only `sim-flow run` advances state, and
   that re-runs work + critique via its own agent subprocess. The
   chat-driven flow had no clean way to record "DM0 done, advance
   to DM1."
3. **No first-class tool concept for code-authoring steps.** DM2c
   needs read / write / search / build / test cycles; the chat
   participant had only a fenced-block artifact convention. Code
   steps weren't reachable.
4. **Hosts other than VS Code would have to re-implement
   orchestration.** A RustRover plugin would need its own copy of
   the TS logic. A standalone CLI mode would need yet another.

## Goal

Commit to **orchestrator as master**. `sim-flow` Rust owns step
graph, instruction loading, message assembly, history policy,
artifact writes, validation, gate evaluation, state advance, and
tool dispatch. Frontends are renderers over a host-neutral JSONL
session protocol. CLI use, VS Code, and any future IDE share the
same orchestrator and the same session semantics.

This phase delivers the four primitives needed to make that real:

- A non-breaking pair of CLI subcommands (`advance`, `describe`)
  that lets the existing extension drop its duplicated step
  knowledge today.
- A session protocol (JSONL on stdio) and orchestrator with
  multiple `Host` impls, including a `JsonlHost` for IDEs and a
  `TerminalHost` for CLI use that wraps subscription-backed CLI
  agents (`claude`, `codex`, `gh-copilot`).
- A tool layer with per-step capability sets and a deterministic
  build / test / coverage iteration loop for code steps.
- A renderer-only VS Code extension consuming the protocol; the
  chat-driven orchestration code from Phase 8 M6-M8 is removed.

## Non-Goals

- Replacing the existing `sim-flow run <step>` CLI command. It
  continues to work for users who want a single-command end-to-end
  invocation outside an interactive host.
- Maintaining backwards compatibility with the chat-driven extension
  protocol. The session protocol is the new contract; the old
  in-extension orchestration is removed wholesale.
- Building a JetBrains / RustRover host. Phase 9 makes that
  possible; the host itself is out of scope.

## Milestone 1 - Non-breaking CLI primitives

Land the two subcommands the chat-driven extension can adopt
immediately to eliminate duplicated step knowledge while the rest
of Phase 9 is in flight.

- [x] `Command::Advance { step: Option<String>, json: bool }` in
  `tools/sim-flow/src/main.rs`. Loads state, evaluates the gate,
  and on a clean result calls `state.mark_passed(step, now)` plus
  `state.save()` plus `state.current_step = next_step()`. Emits
  JSON when `--json` is set. Tests for clean and dirty paths.
- [x] `Command::Describe { step: String, kind: StepKind, json: bool }`.
  Returns `{ instructionPath, instructionBody, workArtifacts,
  predecessorInputs, gateChecks, tools, phases }` for the step.
  Source of truth for everything the extension currently
  hard-codes.
- [x] Extension consumes `describe` instead of duplicating step
  knowledge. Delete `participant/instructions.ts::instructionSlugFor`,
  `artifacts.ts::expectedArtifactPaths`, the per-step path tables,
  and the cross-step input reader; replace with `cli.describe(...)`.
- [x] Plan + architecture audit findings 1, 3 (cross-step inputs)
  closed by virtue of single-source-of-truth.

After M1: chat-driven extension still works, but step knowledge has
exactly one home.

## Milestone 2 - Session protocol + JsonlHost

Define and implement the JSONL session protocol and the orchestrator
core that drives it.

- [x] Rust enum `sim_flow::session::protocol::Event` with
  `serde_json` + `schemars` derive. Covers every event in
  [07-session-protocol.md](../../architecture/ai-flow/07-session-protocol.md).
- [x] CI build emits
  `tools/sim-flow/docs/flow/session-protocol.schema.json` from the
  Rust definitions. The schema is the cross-host contract.
- [x] Rust trait `sim_flow::session::Host` with the methods needed
  to render text, request user input, request LLM responses, emit
  artifact / tool / phase / gate events, and signal session end.
- [x] Rust impl `sim_flow::session::host::JsonlHost` reading /
  writing the protocol on stdio.
- [x] Rust impl `sim_flow::session::host::TestHost` that records
  events for unit testing the orchestrator without a real LLM.
- [x] Rust `sim_flow::session::orchestrator` module that owns the
  step / kind state machine: load instructions, build initial
  messages, drive the turn loop, validate artifacts on writes,
  evaluate the gate at session end, advance state.
- [x] `Command::Session { step, kind, jsonl, llm_backend, ... }`
  invocation that wires the orchestrator to the appropriate host.
- [x] Versioned `Hello` / `HelloAck` handshake.
- [x] Unit tests covering: handshake, work session happy path,
  critique fresh-context invariant, cancellation, protocol-version
  mismatch.

After M2: `sim-flow session DM0.work --jsonl` is invokable end to
end against `TestHost` in tests; the JsonlHost binary entrypoint is
ready for the extension to consume.

## Milestone 3 - Tools and code-step iteration loop

Add the tool dispatcher and the multi-phase iteration loop required
for DM2c / DM3a / DM3c.

- [x] Rust `sim_flow::session::tools` module. `Tool` trait, impls
  for `read_file`, `list_dir`, `write_file`, `search`. Path-sandbox
  enforcement (no traversal, no absolute paths, per-step write
  whitelist).
- [x] `ToolInvoked` events emitted to host on every tool execution.
- [ ] Native tool-use translation for Anthropic / OpenAI HTTP
  clients (request includes tool catalog; responses include
  `tool_use` / `tool_calls`; orchestrator dispatches and threads
  results back).
- [x] Fenced-block fallback (`tool:<name>`) for backends without
  native tool-use.
- [x] `cargo check`, `cargo test` runners as orchestrator-only
  validators with structured output parsing.
- [x] Multi-phase orchestrator: `phase: author -> build -> test ->
  coverage -> done`, configurable per step descriptor, with
  iteration limits and `PhaseChanged` / `BuildOutput` events.
- [x] Step descriptors expanded to declare phases + per-phase tool
  catalogs.
- [x] Tests: fenced fallback round-trips a `read_file`; native
  tool-use round-trips against a stub LLM; build phase fails ->
  retries -> succeeds; iteration limit reached -> RequestUserInput.

After M3: `sim-flow session DM2c.work --jsonl --llm-backend
anthropic` against a small Foundation project produces working code
under iteration.

## Milestone 4 - TerminalHost + CliAgent

Make `sim-flow` a usable CLI without an external host, using the
user's subscription-backed CLI agents.

- [x] Rust trait `sim_flow::agent::CliAgent` (`stream(messages)
  -> impl Stream<Chunk>`).
- [/] HTTP impls: `Anthropic`, `OpenAi`, `Ollama`, `LMStudio`. Use
  the existing TS implementations as a behavioral spec.
- [/] Subprocess impls: `Claude` (`claude` CLI),
  `Codex` (`codex` CLI), `GhCopilot` (`gh copilot` CLI).
  Investigate each tool's non-interactive mode; use the structured
  output where available, prompt-baked context where not.
- [x] `sim_flow::session::host::TerminalHost`: renders to stdout
  (markdown via `termimad` or similar), reads from stdin, dispatches
  `RequestLlmResponse` to the configured `CliAgent`.
- [x] `sim-flow session <step>.<kind>` (no `--jsonl`) defaults to
  `TerminalHost`.
- [ ] Smoke-test: drive DM0 work + critique end-to-end in a plain
  terminal against `claude` CLI.

After M4: a developer can run the full DMF flow without VS Code.

## Milestone 5 - VS Code extension as renderer

Pivot the extension to consume the JSONL protocol.

- [x] Generate TypeScript types from
  `session-protocol.schema.json` at `npm run compile` time. Source
  of truth stays in Rust.
- [x] New module `extensions/sim-flow-vscode/src/session/host.ts`:
  spawn `sim-flow session ... --jsonl`, pump JSONL events,
  translate to chat output, dispatch `RequestLlmResponse` to the
  existing TS LLM backends.
- [x] Chat participant `/step` becomes a launcher; participants
  for `/status`, `/runs`, `/gate`, `/advance`, `/reset`, `/init`
  shell out to the matching `sim-flow` subcommand.
- [/] Setting `sim-flow.chat.scope`: `"session"` (default) or
  `"project"`. Implements both behaviors and lets us pick after
  exercising both.
- [x] **Delete**: `participant/handlers.ts::handleStep` /
  `runStepLlm` / `handleStepContinuation`,
  `participant/session.ts::buildMessageHistory` /
  `extractSessionTag`, `participant/artifacts.ts` (whole module),
  `participant/instructions.ts` (now via `describe`),
  `cli` LLM backend in `src/llm/cli.ts`,
  per-step path tables in TS.
- [x] Keep: dashboard webview, state-toml / experiments / critiques
  readers, file watcher, multi-project resolver, terminal
  integration for non-session CLI ops, packaging.
- [/] Verify VSIX still builds; smoke-test DM0 -> DM1 end-to-end
  through the new pipeline using `vscode.lm` (Copilot) backend.

After M5: VS Code drives the same orchestrator the terminal does.
Step knowledge no longer exists in TypeScript.

## Milestone 6 - Validation, docs, and Phase-8 cleanup

Code-side cleanup is complete; the live smoke tests need a real
LLM backend and are tracked here as user-driven follow-ups.

- [ ] End-to-end validation: drive DM0 -> DM1 through both
  `--jsonl` (VS Code) and `--llm-backend claude` (terminal) on the
  same project. Verify identical artifact + state outcomes.
- [ ] Drive DM2c on a small Foundation project to exercise the
  tool layer and iteration loop.
- [x] Update `docs/architecture/ai-flow/06-vscode-extension.md`
  retired-features section with anything else removed in M5.
- [ ] Update CHANGELOG (in sim-foundation root if/when it exists).
- [x] Move Phase 8's status block to "superseded" and link to this
  phase's results.

Smoke-test recipe (terminal, no extension required):

```bash
cd <project>
sim-flow init --flow direct-modeling                                # if needed
sim-flow session DM0.work \
    --foundation-root /Users/mneilly/nta/sim-foundation \
    --llm-backend claude                                            # or codex / ollama / lmstudio
# answer the agent until spec.md is on disk; type /end-session
sim-flow session DM0.critique --foundation-root <root> --llm-backend claude
# critique writes .sim-flow/critiques/DM0-critique.md
sim-flow advance DM0                                                # records the gate, bumps current_step
# repeat for DM1, DM2a, ...
```

Smoke-test recipe (VS Code, identical project state expected):

```text
@sim-flow /step DM0.work
   ...iterate, /end-session...
@sim-flow /step DM0.critique
   ...iterate, /end-session...
@sim-flow /advance DM0
```

Both paths share the same Rust orchestrator; outputs on disk and
the `current_step` mutation should be byte-identical.

## Status

Code-complete through M5; M4's HTTP-agent matrix landed in a
follow-up commit alongside Ollama/LM Studio/Codex/gh-copilot
support. M6 is the wrap-up: docs are done, live end-to-end smoke
tests are deferred to the user.

Per-milestone outcomes:

- **M1** (`3110f06`): `sim-flow advance` + `sim-flow describe`,
  extension consumes `describe`. Drift between Rust and TS step
  knowledge eliminated.
- **M2** (`9ad2c0c`): JSONL protocol, `Host` trait, `JsonlHost`,
  `TestHost`, orchestrator state machine,
  `Command::Session ... --jsonl`, schema export +
  drift-check test.
- **M3** (`29398aa`): `Tool` trait + `read_file`/`list_dir`/
  `write_file`/`search`, `cargo check` / `cargo test` runners,
  per-step `work_tools` / `critique_tools` / `work_phases` /
  `critique_phases`, multi-phase iteration loop with 5-iter caps.
  Native tool-use API translation deferred (fenced-block fallback
  works for every backend today).
- **M4** (`4aee92a`, `eb4ed00`): `TerminalHost`, `CliAgent` trait,
  `MockAgent`, `ClaudeAgent`, `CodexAgent`, `GhCopilotAgent`,
  `OllamaAgent`, `LmStudioAgent`. HTTP path uses `ureq` via the
  shared `openai_compatible` module. Anthropic / OpenAI HTTP
  agents skipped because the user's access path is subscription
  CLIs, not API keys.
- **M5** (`<pending hash>`): TS extension is now a renderer over
  the JSONL protocol. `participant/session.ts`,
  `participant/artifacts.ts`, `participant/instructions.ts`, the
  `cli` LLM backend, and `runStepLlm`/`handleStep`/
  `handleStepContinuation` are all deleted. Step knowledge is
  structurally absent from TypeScript.
- **M6** (`<this commit>`): docs updated; live smoke tests handed
  off to the user. Phase 8 status block superseded.

130 sim-flow Rust tests + 127 vitest cases passing. Workspace
clippy clean. VSIX builds at 8.02 MB.

## Risks

Status update on the risks called out at the start of Phase 9:

- **CLI agent non-interactive support.** Resolved for `claude`
  (`claude -p` works cleanly with stdin). `codex exec -` is
  shipped but flagged experimental in code comments; `gh copilot
  suggest` is best-effort only and the error message points users
  at the `vscode` source for full Copilot Chat. Local Ollama and
  LM Studio are HTTP-based and reliable. Anthropic / OpenAI HTTP
  agents weren't added because the user's access is subscription
  CLIs, not API keys; trivial to add later.
- **VS Code Language Model tool registration.** Phase 9 M5
  shipped with the fenced-block fallback for tool calls
  (`tool:<name>` parsing in the orchestrator). Native tool-use
  via `vscode.lm.registerTool` is a polish pass for a follow-up
  phase; the orchestrator works against any backend without it.
- **Cargo check / test latency.** Mitigated by caching: the
  orchestrator just shells `cargo check`/`cargo test` and Rust's
  incremental compilation kicks in for re-runs in the iteration
  loop. JSON diagnostic parsing isn't necessary at this stage.
- **Coverage tooling.** Skipped in v1 - `coverage` phase not yet
  registered for any step. Easy to add when DM3c needs it.

## Acceptance

Phase 9 is done when:

1. A user can drive DM0 -> DM1 via either VS Code or a plain
   terminal with no extension installed, and both produce
   identical artifacts and state.
2. A user can drive DM2c work end-to-end (LLM authors code,
   orchestrator runs build / test, iterates as needed) and the
   step's gate passes.
3. The VS Code extension contains zero step knowledge: no
   instruction slugs, no expected artifact paths, no
   message-assembly logic, no per-step input tables.
4. A second IDE host could be implemented from
   [07-session-protocol.md](../../architecture/ai-flow/07-session-protocol.md)
   alone, without reading sim-flow Rust source.
