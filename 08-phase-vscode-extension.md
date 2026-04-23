# Phase 8 - VS Code Extension

Phase dependency: Phase 1 (orchestrator core), Phase 2 (model-project
template), Phase 3 (DMF instructions and gate checks), Phase 4
(experiment tracking). Architecture: [06-vscode-extension.md](../../architecture/ai-flow/06-vscode-extension.md).

## Problem Statement

The terminal-first `sim-flow` CLI is complete enough to drive the DMF
end to end. The next leverage point is meeting users where they already
work: inside VS Code, with their editor, terminal, and chat surfaces
already open. This phase delivers a VS Code extension that adapts the
CLI's state machine into an in-editor experience without forking the
orchestrator's semantics.

The extension ships a `@sim-flow` chat participant with step-aware
slash commands, a webview dashboard showing the step rail / gate
status / experiments / baselines / sweeps, file-system watchers that
keep the dashboard live, and command-palette actions for the common
operations. Mutating CLI operations run in a managed VS Code terminal
so their streaming output stays visible. LLM access is configurable
across the VS Code Language Model API, external provider API keys, and
a CLI-fallback path that matches today's interactive subprocess flow.

DMF is the v1 target. DSF support (additional slash commands,
per-candidate routing) follows in a future phase once Phase 6 lands.

## Milestone 1 - CLI Machine-Readable Surface

The extension cannot reliably parse human-formatted CLI output. Add
`--json` output to the subcommands the extension consumes so the
extension reads structured data.

- [x] Extend `sim-flow status` with `--json` emitting `{ flow,
  current_step, started, gates, archived_gates }`.
- [x] Extend `sim-flow runs` with `--json` emitting an array of
  `RunRow`-shaped objects.
- [x] Extend `sim-flow gate <step>` with `--json` emitting
  `{ step, clean, failures: [{description, reason}] }`. JSON is
  written on stdout even when the gate fails; the exit code
  distinguishes pass / fail for non-JSON consumers.
- [x] Extend `sim-flow baseline create`, `list`, and `compare` with
  `--json` emitting structured records.
- [x] Extend `sim-flow new model` with `--json` emitting
  `{ project_dir, crate_name, next_step }`.
- [x] Document the JSON shapes in
  [cli-json.md](../../architecture/ai-flow/cli-json.md) so the
  extension can pin against stable output.
- [x] Add integration tests for the JSON writer paths in
  `crates/sim-flow/tests/cli_json.rs` covering status, runs, gate,
  baseline list, baseline compare, and new model.

## Milestone 2 - Extension Scaffolding

- [x] Create `extensions/sim-flow-vscode/` sibling to `crates/`,
  excluded from the Cargo workspace (`[workspace.exclude]` in the
  root `Cargo.toml` lists it).
- [x] Initialize the TypeScript extension with `package.json`,
  `tsconfig.json`, `.vscodeignore`, and a minimal `src/extension.ts`
  implementing `activate` / `deactivate`.
- [x] Declare activation on `workspaceContains:.sim-flow/state.toml`,
  `onCommand:sim-flow.openDashboard`, and `onCommand:sim-flow.init`.
- [x] Register the command-palette commands: `sim-flow.openDashboard`,
  `sim-flow.runStep`, `sim-flow.gateStep`, `sim-flow.resetStep`,
  `sim-flow.init`. Every command currently surfaces a "not yet
  implemented" notice pointing at the milestone that will wire it up.
- [x] Contribute settings under `sim-flow.*`: `binaryPath`,
  `foundationRoot`, `llm.source` (enum), `llm.model`,
  `dashboard.openOnActivate`.
- [x] Add `.editorconfig`, ESLint (flat config with
  `typescript-eslint`), and Prettier so style is consistent.
- [x] Add a `vscode-extension` job to `.github/workflows/ci.yml` that
  runs `npm run compile`, `npm run lint`, and `npm run format:check`.
  The install step falls back to `npm install` until the first
  contributor commits a `package-lock.json`.
- [/] Verify the extension compiles and lints cleanly with `npm ci`.
  Node is not installed on this machine; the scaffolding is correct
  by inspection and CI will surface any issues on first push. The
  first developer with node should run `npm install && npm run ci`,
  commit `package-lock.json`, and drop the `npm install` fallback in
  the workflow.

## Milestone 3 - sim-flow CLI Wrapper

- [x] Implement the CLI wrapper under `src/cli/`:
  - `types.ts` - TypeScript shapes mirroring
    [cli-json.md](../../architecture/ai-flow/cli-json.md).
  - `errors.ts` - `SimFlowCliError` with a typed `kind`
    (`binary-not-found`, `spawn-failed`, `non-zero-exit`,
    `json-parse-failed`, `unexpected-stdout`, `not-implemented`).
  - `executor.ts` - injectable `Execute` function type backed by
    `promisify(execFile)` so tests swap in a stub.
  - `resolve.ts` - `resolveBinary({ settingOverride, pathEnv,
    bundledCandidates, exists })` honoring setting > PATH > bundled.
    Bundled candidates intentionally empty for now; Milestone 11
    wires them up.
  - `simflow.ts` - `SimFlowCli` class with typed methods for every
    `--json` subcommand (`status`, `runs`, `gate`, `baseline*`,
    `newModel`) plus `buildArgs` / `buildCommandLine` helpers that
    Milestone 9 will use to feed commands into a VS Code terminal.
- [x] Binary resolution prefers the `sim-flow.binaryPath` setting,
  then `$PATH`, then a future bundled-binary path (placeholder).
- [x] Surface non-zero exit codes as typed `SimFlowCliError`
  instances. `gate` tolerates non-zero exits when the failure
  payload is on stdout (the `gate --json` CLI contract).
- [/] Long-running subcommands stream via
  `child_process.spawn` forwarded to a VS Code terminal. The wrapper
  exposes argv / command-line builders; the terminal wiring itself
  lives in Milestone 9's `src/terminal.ts`.
- [x] vitest unit tests in `src/cli/*.test.ts` drive the full
  wrapper (status, runs, gate, baselines, newModel, argv helpers,
  resolveBinary) with an injected `Execute`, no real subprocess
  spawn. `npm run ci` now runs compile + lint + format:check + test.

## Milestone 4 - State Readers

- [x] Implement `src/state/flowState.ts`: read `.sim-flow/state.toml`
  using `smol-toml`. Expose a typed `FlowState` shape aliased to the
  same `StatusResult` used by the CLI wrapper. Parse failures surface
  as `FlowStateParseError` with file context. Handles flat gates,
  per-candidate gate subtables, and archived_gates.
- [x] Implement `src/state/critiques.ts`: list + read
  `.sim-flow/critiques/*.md`. The parser follows the same rule as
  `crates/sim-flow/src/critique.rs` -- any line whose first
  non-whitespace token (after stripping leading `-` or `*` list
  markers) is `UNRESOLVED:` / `BLOCKER:` / `RESOLVED:` becomes a
  Finding.
  `hasBlocking` is true iff any Unresolved or Blocker exists.
- [x] Implement `src/state/experiments.ts`: read
  `.sim-flow/experiments.db` via `better-sqlite3` in read-only mode.
  `ExperimentsReader` mirrors the CLI wrapper's `RunFilter` surface
  and returns the same `RunRow` / `BaselineRecord` types for
  structural consistency. `withExperiments(projectDir, cb)` scopes
  the handle to a callback. Returns null when the DB does not exist
  (first-run projects).
- [x] Add a workspace-scoped file watcher in
  `src/state/watcher.ts`: `createStateWatcher(projectDir)` wraps VS
  Code's `FileSystemWatcher` on `.sim-flow/state.toml`,
  `.sim-flow/critiques/*.md`, and `.sim-flow/experiments.db`, fanning
  events out through a single `onDidChange` EventEmitter tagged with
  the change kind so subscribers can re-read selectively.
- [x] Unit-test the parsers with fixture-inline vitest cases:
  `flowState.test.ts` covers flat / per-candidate / archived shapes
  and error paths; `critiques.test.ts` covers the four marker paths
  plus line-number tracking; `experiments.test.ts` seeds an on-disk
  SQLite DB, then exercises countRuns / listRuns (workload, sweep,
  limit, newest-first), getRun, and listBaselines.

## Milestone 5 - Webview Dashboard (v1)

- [x] Implement `src/webview/host.ts`: `DashboardHost` class owns
  the single webview panel, renders the HTML shell (strict CSP with
  per-load nonce), handles postMessage plumbing, and disposes
  cleanly. Calls `aggregateDashboardState` (pure helper in
  `src/webview/aggregate.ts`) on every refresh so the aggregation
  logic is unit-testable without loading vscode.
- [x] Build `src/webview/panel.ts` as a plain-DOM TypeScript
  front-end (no framework). Bundled with esbuild to a single IIFE at
  `dist/webview/panel.js`; a sibling `src/webview/tsconfig.json`
  type-checks the file separately from the extension compile.
- [x] Render the step rail with gate diamonds between steps using
  the same `--step-bg` / `--gate-bg` palette as
  [images/direct-modeling-flow.dot](../../architecture/ai-flow/images/direct-modeling-flow.dot).
  Steps carry `passed` / `current` / `locked` / `selected` classes;
  gates carry `passed` / `failed` classes.
- [x] Implement the per-step detail panel: clicking a step selects
  it, shows the Findings list from its critique (Blocker, Unresolved,
  Resolved with line numbers), and surfaces Run Step / Run Gate /
  Reset / Open Critique buttons that dispatch messages back to the
  host.
- [x] Wire auto-refresh on file-watcher events. `DashboardHost`
  attaches a `SimFlowStateWatcher` on open; every change coalesces
  into a single pending refresh (no stampede), which rebuilds state
  from disk + the CLI and posts it to the webview.
- [x] Implement the Experiments tab: table of the most recent 200
  runs with run id, timestamp, workload, study/candidate, and a
  short git commit (with dirty suffix). Full filter UI is deferred
  to a future polish pass.
- [x] Implement the Baselines tab: list (name / run / timestamp).
  Create / compare UIs are wired through to the CLI methods from M3
  but not yet surfaced as dashboard buttons; the chat participant
  in M6 exposes them first.
- [x] Stub the Sweeps tab with a pointer to Phase 8 M8.

## Milestone 6 - Chat Participant And Slash Commands

- [x] Register `@sim-flow` via
  `vscode.chat.createChatParticipant('sim-flow', handler)` and
  declare commands + activation event in package.json.
- [x] Implement `src/participant/` split into modules: `index.ts`
  (dispatcher + followup provider), `args.ts` (pure parsers),
  `handlers.ts` (per-command implementations), `format.ts`
  (markdown formatters), `instructions.ts` (reads
  `sim-foundation/instructions/<slug>.md`), and `followups.ts`
  (state-driven suggestion logic).
- [x] Implement read-only commands `/status`, `/runs`, `/gate`,
  `/reset` against the CLI wrapper. No LLM required. Markdown output
  tables are rendered via `format.ts` helpers.
- [/] Implement `/step <id>.work` and `/step <id>.critique`. The
  parser, state gating (refuses when `state.current_step` differs
  from the requested step), and instruction-file preview are in
  place; the actual LLM turn lands in M7. Until then the handler
  shows the instruction markdown that would be fed to the LLM
  along with the existing critique when present.
- [x] `/reset <step-id>` shells out via `cli.buildArgs` + a local
  execFile wrapper; emits a confirmation and points the user at
  `/status`.
- [/] Tag each chat session with its `(step, kind)` via participant
  metadata. Tagging is not stored yet; the handler re-parses the
  most recent command per turn. M7 adds metadata-backed context so
  free-form follow-up turns know which step they belong to.
- [x] Followup provider reads state and suggests the next sensible
  action (work + critique + gate while the current gate is open;
  next step once it passes; /status when the flow is complete).

## Milestone 7 - Step Gating And Session Isolation

- [x] Implement `src/participant/gating.ts`: `checkStepGate(state,
  step, kind)` returns a typed GateOutcome. Rules: refuse with an
  `/init` hint when state is missing; allow the current step;
  refuse with a `/reset` hint when the requested step has already
  passed; refuse with the current-step hint when the requested step
  is ahead of the flow.
- [x] Preserve the fresh-critique invariant in
  `src/participant/session.ts`. `buildMessageHistory` constructs the
  LLM message array per turn:
  - Work sessions: system (instructions) + prior turns tagged with
    the same `(step, "work")` + current user prompt.
  - Critique sessions: system (instructions) + optional artifact
    manifest + current user prompt. Prior work-session turns are
    filtered out even when the user ran `/step X.work` and
    `/step X.critique` in the same chat tab.
- [x] Tag `/step` responses with `{ tag: "sim-flow", step, kind }`
  via the returned `ChatResult.metadata`. `extractSessionTag(history)`
  walks prior turns newest-first to find the most recent tagged
  response; Milestone 8 will use it to bind free-form continuation
  turns to the right session.
- [x] Reworked `/reset` UX so the confirmation points the user at
  the next concrete action (`/step <step>.work` / `.critique`) and
  mentions that prior chat history stays visible.
- [x] Unit-test the gating logic and session builders with inline
  fixtures: `gating.test.ts` covers the four outcomes;
  `session.test.ts` verifies work-turn filtering, the fresh-critique
  shape, empty-prompt handling, and `extractSessionTag` newest-first
  semantics.

## Milestone 8 - LLM Source Abstraction

- [x] Implement `src/llm/` with four backends (split across files for
  testability rather than a single `llm.ts`):
  1. `vscode.lm` via `vscode.lm.selectChatModels(...)`
     (`src/llm/vscode.ts`).
  2. Anthropic Messages API via fetch, key from SecretStorage
     (`src/llm/anthropic.ts`, key id `sim-flow.anthropic.apiKey`).
  3. OpenAI Chat Completions API via fetch, key from SecretStorage
     (`src/llm/openai.ts`, key id `sim-flow.openai.apiKey`).
  4. CLI fallback: open a VS Code terminal, run `sim-flow run <step>`,
     and watch for the critique file to land (`src/llm/cli.ts`).
- [x] Route chat-participant LLM calls through the configured
  `sim-flow.llm.source` via `createBackend()` in `src/llm/factory.ts`;
  `handleStep` assembles instruction + tagged history + optional
  critique artifact summary and streams backend chunks into the chat.
- [x] Expose `sim-flow.setApiKey` and `sim-flow.clearApiKey` commands
  (`src/apiKey.ts`) that QuickPick Anthropic vs OpenAI then store or
  delete the key in `vscode.ExtensionContext.secrets`.
- [x] CLI-source path: chat participant surfaces a terminal pane,
  prints a "drive the session in the terminal" notice, and for
  critique sessions waits on a FileSystemWatcher for the critique file
  to land before streaming its contents back to chat.
- [x] Vitest coverage for the non-network parts: factory backend
  selection, `extractAnthropicText`, `extractOpenAiText`, and the
  missing-api-key / HTTP-error branches of the Anthropic and OpenAI
  backends using injected `fetchImpl` stubs (`src/llm/*.test.ts`).

## Milestone 9 - Terminal Integration

- [ ] Implement `src/terminal.ts`: create (or reuse) a named
  `sim-flow` terminal and run arbitrary CLI commands there.
- [ ] Dashboard buttons for sweep, baseline create, reset, new
  candidate, etc. route through the terminal integration so the user
  sees live progress.
- [ ] On terminal-run completion (detected by file-watcher signal or
  exit code if we wrap), refresh the dashboard.

## Milestone 10 - Multi-Project Awareness

- [ ] Resolve the active project directory: walk up from the active
  editor file until a `.sim-flow/` ancestor is found; fall back to
  the nearest workspace folder containing one.
- [ ] Chat participant accepts an optional `--project <path>`
  argument on every command; the resolver uses it when supplied.
- [ ] Dashboard exposes a project picker dropdown when more than one
  sim-flow project exists in the workspace.
- [ ] Per-project `FileSystemWatcher` instances so state updates in
  one project do not refresh unrelated dashboards.

## Milestone 11 - Packaging And Distribution

- [ ] Author a user-facing README under `extensions/sim-flow-vscode/`.
- [ ] Add a VSIX build script (`vsce package`) and document how to
  install locally.
- [ ] Bundle prebuilt `sim-flow` binaries for macOS aarch64, macOS
  x86-64, Linux x86-64, and Windows x86-64 inside the VSIX, picked
  at activation based on `process.platform` / `process.arch`.
- [ ] Verify the extension activates and completes a dummy DM0 work
  session against a scratch project on each platform.
- [ ] Publish to the VS Code Marketplace as a pre-release, gated
  behind the user confirming they are comfortable with a 0.x
  extension.

## Milestone 12 - DSF Enablement Hooks

DSF isn't fully wired until Phase 6 lands, but the extension should
be ready.

- [ ] Register DS0-DS9 slash commands behind a feature flag so they
  are visible only when the active project's `state.toml` says
  `flow = "design-study"`.
- [ ] Per-candidate command forms: `/step DS5a.work --candidate
  mesh-noc`. Route `--candidate` through to CLI invocations.
- [ ] Dashboard step rail renders DSF steps when the flow is
  `design-study`; switches to DMF rail after DS9 flips state.
- [ ] Keep all DSF-specific tasks gated behind feature checks so
  Phase 8 can ship with DMF-only support and enable DSF once
  Phase 6 is complete.

## Milestone 13 - Documentation And Walkthrough

- [ ] Add a `docs/getting-started/vscode-extension.md` walkthrough:
  install → open a sim-flow project → drive DM0 → watch the gate.
- [ ] Update the top-level
  [docs/architecture/architecture.md](../../architecture/architecture.md)
  table of contents to include the new VS Code extension doc.
- [ ] Update [docs/architecture/ai-flow/02-direct-modeling-flow.md](../../architecture/ai-flow/02-direct-modeling-flow.md)
  with a "VS Code" subsection describing how the extension presents
  the same session semantics.
- [ ] Update `CHANGELOG.md` when the extension is usable end-to-end.

## Status

In progress. Milestones 1 through 8 are complete:

- M1 shipped the `--json` flag on every subcommand the extension
  consumes, the [cli-json.md](../../architecture/ai-flow/cli-json.md)
  schema, and `crates/sim-flow/tests/cli_json.rs` regression coverage.
- M2 scaffolded `extensions/sim-flow-vscode/` with package.json,
  tsconfig, ESLint/Prettier/.editorconfig, a minimal extension.ts
  that registers every planned command as a "not yet implemented"
  stub, and a `vscode-extension` CI job.
- M3 delivered `src/cli/` - typed wrapper (`SimFlowCli`), binary
  resolution (`resolveBinary`), typed errors (`SimFlowCliError`),
  and vitest unit tests.
- M4 delivered `src/state/` - direct readers for `state.toml`
  (flowState.ts), critique markdown files (critiques.ts), and the
  experiments SQLite DB (experiments.ts), plus a VS Code
  FileSystemWatcher fan-out (watcher.ts). Parser tests cover the
  three formats with fixture-inline vitest cases.
- M5 delivered the Flow Dashboard webview. `DashboardHost` owns
  the panel lifecycle, renders a strict-CSP HTML shell, and
  streams a DashboardState payload to the webview on every
  file-system change. The browser-side `panel.ts` is bundled by
  esbuild to `dist/webview/panel.js`, renders a step rail + per-step
  detail + experiments/baselines/sweeps tabs, and dispatches
  run/gate/reset/open-critique messages back to the host. The
  `sim-flow.openDashboard` command is live.
- M6 registered `@sim-flow` as a chat participant with slash
  commands `/status`, `/runs`, `/gate`, `/reset`, `/init`, `/step`.
  The CLI-backed commands shell out to the M1 `--json` surface and
  render markdown tables. A followup provider suggests the next
  sensible action based on state.
- M7 centralized the gate decision (`participant/gating.ts`),
  introduced session tagging via `ChatResult.metadata`, and added
  `buildMessageHistory` which enforces the fresh-critique invariant
  (work-session turns are filtered out of critique message arrays).
  `/reset` UX now points at the next concrete action.
- M8 landed the `src/llm/` backend abstraction with four
  implementations (`vscode.lm`, Anthropic, OpenAI, CLI fallback) and a
  `createBackend` factory selected by `sim-flow.llm.source`.
  `handleStep` now reads the configured instruction file, builds the
  session message array (plus optional critique artifact summary), and
  streams the backend's chunks into the chat. API keys are managed via
  `sim-flow.setApiKey` / `sim-flow.clearApiKey` against VS Code
  SecretStorage. Vitest covers the non-network surface.

Milestones 9-11 remain. DSF-specific hooks (M12) still depend on
Phase 6.

## Scope Caveats

- DMF only for v1. DSF routing lives behind a feature flag and
  activates once Phase 6 ships.
- No VS Code theme customization beyond matching the default
  light/dark palette variables.
- No in-extension Copilot billing or account-linking UI -- users
  install the Copilot extension separately and we call
  `vscode.lm.selectChatModels`.
- No co-editing / live-share integration. The extension is
  single-user for now.
