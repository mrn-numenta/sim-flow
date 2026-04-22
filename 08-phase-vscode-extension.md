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

- [ ] Register `@sim-flow` via
  `vscode.chat.createChatParticipant('sim-flow', handler)`.
- [ ] Implement `src/participant.ts` dispatcher parsing the slash
  commands listed in the architecture doc.
- [ ] Implement read-only commands first: `/status`, `/runs`, `/gate`.
  These respond with rendered text; no LLM call.
- [ ] Implement `/step <id>.work` and `/step <id>.critique`: seed the
  conversation context, invoke the language model (Milestone 7), and
  stream the response.
- [ ] Implement `/reset <step-id>` shelling out to the CLI.
- [ ] Tag each chat session with its `(step, kind)` via participant
  metadata so subsequent turns know the context.
- [ ] Provide followup suggestions (`vscode.ChatFollowup`) that
  suggest the next sensible action (`/gate`, `/step DM1.work`, etc.)
  based on orchestrator state.

## Milestone 7 - Step Gating And Session Isolation

- [ ] Implement `src/gate.ts`: for each chat request, look up the
  tagged `(step, kind)` and compare against `state.toml`. Refuse
  with a friendly message if mismatched.
- [ ] Preserve the fresh-critique invariant: the critique handler
  constructs a fresh `[system, user]` message array using ONLY the
  critique instructions and the artifact manifest; the work session's
  history is not included, even if the user ran `/step X.work` and
  `/step X.critique` in the same chat tab.
- [ ] Add a "revisit prior step" UX path: `/reset <step-id>` opens
  the relevant chat(s) (if any exist) and tells the user to resume
  there.
- [ ] Unit-test the gating logic with synthetic `state.toml`
  fixtures.

## Milestone 8 - LLM Source Abstraction

- [ ] Implement `src/llm.ts` with three backends:
  1. `vscode.lm` via `vscode.lm.selectChatModels(...)`.
  2. External API via fetch, with key read from
     `vscode.ExtensionContext.secrets`.
  3. CLI fallback: spawn `sim-flow run <step>` in a terminal and
     watch for the critique file to appear.
- [ ] Route chat-participant LLM calls through the configured
  `sim-flow.llm.source`.
- [ ] Expose a command `sim-flow.setApiKey` that prompts for the key
  and stores it in SecretStorage.
- [ ] When the source is `"cli"`, the chat participant surfaces a
  terminal pane and a notice "Interactive session is running in the
  terminal; return here for the critique".
- [ ] Sanity-test each backend against a trivial prompt.

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

In progress. Milestones 1 through 5 are complete:

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

Milestones 6-11 remain. DSF-specific hooks (M12) still depend on
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
