# Phase 8 - VS Code Extension (chat-driven; superseded by Phase 9)

> **Status as of Phase 9 pivot**: Milestones 1-11 of this phase are
> complete and shipped a working chat-driven VS Code extension that
> owned much of the session orchestration in TypeScript. End-to-end
> testing exposed structural issues (step knowledge duplicated across
> Rust and TS, no command to advance state after a gate passes,
> no first-class tool concept for code-authoring steps, hosts other
> than VS Code would have to re-implement orchestration). Phase 9
> commits to **orchestrator as master**: sim-flow drives sessions via
> a host-neutral JSONL protocol, and the VS Code extension becomes a
> renderer. See
> [09-phase-orchestrator-driven-sessions.md](./09-phase-orchestrator-driven-sessions.md)
> for the migration plan and
> [06-vscode-extension.md](../../architecture/ai-flow/06-vscode-extension.md)
> for the new architecture.
>
> The milestones below remain useful as a record of what was
> implemented (CLI JSON surface, state readers, dashboard webview,
> chat participant, gating, session tagging, LLM source abstraction,
> terminal integration, multi-project awareness, internal packaging).
> Most of the in-extension orchestration code from M6-M8 is removed
> in Phase 9; the dashboard, file-system watcher, multi-project
> awareness, and packaging plumbing carry forward unchanged.

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
  `tools/sim-flow/tests/cli_json.rs` covering status, runs, gate,
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
  `tools/sim-flow/src/critique.rs` -- any line whose first
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

- [x] Implement `src/llm/` with six backends (split across files for
  testability rather than a single `llm.ts`):
  1. `vscode.lm` via `vscode.lm.selectChatModels(...)`
     (`src/llm/vscode.ts`).
  2. Anthropic Messages API via fetch, key from SecretStorage
     (`src/llm/anthropic.ts`, key id `sim-flow.anthropic.apiKey`).
  3. OpenAI Chat Completions API via fetch, key from SecretStorage
     (`src/llm/openai.ts`, key id `sim-flow.openai.apiKey`). Shares
     a base class (`src/llm/openai-compat.ts`) with the two
     OpenAI-compatible local backends below.
  4. Ollama via its OpenAI-compatible endpoint
     (`src/llm/ollama.ts`, default base URL
     `http://localhost:11434/v1`, key optional).
  5. LM Studio via its OpenAI-compatible endpoint
     (`src/llm/lmstudio.ts`, default base URL
     `http://localhost:1234/v1`, key optional).
  6. CLI fallback: open a VS Code terminal, run `sim-flow run <step>`,
     and watch for the critique file to land (`src/llm/cli.ts`).
- [x] Route chat-participant LLM calls through the configured
  `sim-flow.llm.source` via `createBackend()` in `src/llm/factory.ts`;
  `handleStep` assembles instruction + tagged history + optional
  critique artifact summary and streams backend chunks into the chat.
- [x] Expose `sim-flow.setApiKey` and `sim-flow.clearApiKey` commands
  (`src/apiKey.ts`) that QuickPick the provider (Anthropic, OpenAI,
  Ollama, LM Studio) then store or delete the key in
  `vscode.ExtensionContext.secrets`. Ollama and LM Studio keys are
  optional - they only matter when fronting a remote instance behind
  an auth proxy.
- [x] CLI-source path: chat participant surfaces a terminal pane,
  prints a "drive the session in the terminal" notice, and for
  critique sessions waits on a FileSystemWatcher for the critique file
  to land before streaming its contents back to chat.
- [x] Vitest coverage for the non-network parts: factory backend
  selection across all six sources, `extractAnthropicText` /
  `extractOpenAiText`, the missing-api-key / HTTP-error branches for
  Anthropic and OpenAI, and the base-URL / optional-auth behavior of
  Ollama and LM Studio - all exercised via injected `fetchImpl` stubs
  (`src/llm/*.test.ts`).

## Milestone 9 - Terminal Integration

- [x] Implement `src/terminal.ts`: `SimFlowTerminal` owns a named
  `sim-flow` terminal, reveals it without stealing focus, sends the
  caller-supplied command line, and transparently recreates the
  terminal if the user closed it.
- [x] Wire the dashboard's existing `Run Step` and `Reset` buttons
  through the terminal via the `sim-flow.runStep` /
  `sim-flow.resetStep` commands (previously "not yet implemented"
  stubs). `sim-flow.init` also runs through the shared terminal for
  consistency.
- [x] Dashboard refresh happens automatically: the M4 state watcher
  on `.sim-flow/state.toml`, critiques, and `experiments.db` already
  posts a `state-update` to the webview on any file change the CLI
  makes, so no explicit completion signal is required.
- [x] Drop the dead `sim-flow.gateStep` command (no caller - the
  dashboard's `Run Gate` button runs in-process via the CLI wrapper).
- [ ] Follow-up: add dashboard buttons for `sweep`, `baseline create`,
  and (DSF) `new candidate` once those tabs grow authoring UI. The
  terminal routing is already in place; these are purely webview
  additions.

## Milestone 10 - Multi-Project Awareness

- [x] Resolve the active project directory: walk up from the active
  editor file until a `.sim-flow/` ancestor is found; fall back to
  the nearest workspace folder containing one. Lives in
  `src/context.ts::resolveProjectDir` (pre-existing) alongside the
  new `findProjectCandidates` scanner and `pickProject` QuickPick
  helper.
- [x] Chat participant accepts an optional `--project <path>`
  argument on every command. Implementation: `extractProjectHint`
  in `src/participant/args.ts` strips the flag once in the
  dispatcher, then `resolveContext({ projectDir: hint })` trusts
  the hint (verifying `.sim-flow/state.toml` exists). The hint is
  never visible to per-command parsers; handlers see the cleaned
  prompt via the new `HandlerArgs.prompt` field.
- [x] Dashboard exposes a project picker when more than one
  sim-flow project exists in the workspace.
  `extension.ts::selectProjectDir` calls `findProjectCandidates` +
  `pickProject`; single-candidate workspaces skip the prompt.
- [x] Per-project `FileSystemWatcher` instances so state updates
  in one project do not refresh unrelated dashboards. Each project
  gets its own `DashboardHost` in a `Map<projectDir, DashboardHost>`,
  and each host creates its own `createStateWatcher(projectDir)`
  instance. The shared terminal is also per-project
  (`Map<projectDir, SimFlowTerminal>`) so parallel runs from
  different projects do not interleave output.

## Milestone 11 - Internal Packaging

This extension is an internal tool and is not published to the VS
Code Marketplace. Distribution is via the VSIX artifact plus an
install doc.

- [x] Author an internal README under `extensions/sim-flow-vscode/`
  covering prerequisites, `vsce package`,
  `code --install-extension sim-flow-vscode-<version>.vsix`,
  settings, slash commands, LLM backends, and the `bin/` layout for
  bundled binaries.
- [x] `npm run package` wraps `vsce package` and produces
  `sim-flow-vscode-<version>.vsix` (pre-existing script, now
  documented and verified end-to-end). `*.vsix` is gitignored.
- [x] Binary bundling infrastructure: `src/cli/bundled.ts` owns the
  platform → subdirectory mapping and candidate-path factory.
  `activate()` seeds `setBundledRoot(context.extensionUri.fsPath)`
  and both `tryResolveBinary` call sites pass `bundledCandidates`
  into `resolveBinary`. When a VSIX ships with
  `bin/<platform>-<arch>/sim-flow[.exe]` that candidate resolves
  after the setting override and `$PATH`; when the directory is
  absent the resolver falls through transparently. Supported
  triples today: `darwin-arm64`, `darwin-x64`, `linux-x64`,
  `win32-x64`.
- [ ] Follow-up (not needed for the internal dev loop): populate
  the `bin/` directory with prebuilt binaries for distribution
  VSIXes. Tick once someone on the team wants a zero-PATH install.
- [ ] Follow-up: verify the extension activates and completes a
  dummy DM0 work session against a scratch project. Marked as a
  post-install smoke test rather than a CI matrix item.

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

In progress. Milestones 1 through 11 are complete:

- M1 shipped the `--json` flag on every subcommand the extension
  consumes, the [cli-json.md](../../architecture/ai-flow/cli-json.md)
  schema, and `tools/sim-flow/tests/cli_json.rs` regression coverage.
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
- M8 landed the `src/llm/` backend abstraction with six
  implementations (`vscode.lm`, Anthropic, OpenAI, Ollama, LM Studio,
  CLI fallback) and a `createBackend` factory selected by
  `sim-flow.llm.source`. Ollama and LM Studio share the OpenAI
  Chat Completions wire format with OpenAI itself via a common
  `OpenAiCompatibleBackend` base; their default base URLs are
  configurable through `sim-flow.llm.ollama.baseUrl` and
  `sim-flow.llm.lmstudio.baseUrl`. `handleStep` now reads the
  configured instruction file, builds the session message array
  (plus optional critique artifact summary), and streams the
  backend's chunks into the chat. API keys are managed via
  `sim-flow.setApiKey` / `sim-flow.clearApiKey` against VS Code
  SecretStorage. Vitest covers the non-network surface.
- M9 added `src/terminal.ts` (`SimFlowTerminal`) - a reusable named
  VS Code terminal that the dashboard's Run Step / Reset buttons and
  the `sim-flow.init` command now route through. The three stub
  command handlers are gone. Dashboard refresh after a terminal run
  is driven by the existing file watcher on `.sim-flow/`, so no new
  signalling is needed. The dead `sim-flow.gateStep` command (no
  caller, no keybinding) was removed. Follow-up: add dashboard
  buttons for `sweep` / `baseline create` when those tabs get
  authoring UI.
- M10 added multi-project awareness. `src/context.ts` gained
  `findProjectCandidates` (workspace scan via `findFiles`) and
  `pickProject` (QuickPick), and `resolveContext` now accepts a
  `{ projectDir }` hint that the chat participant feeds from its
  `--project <path>` flag. The dispatcher strips the flag via
  `extractProjectHint` and passes the cleaned prompt through
  `HandlerArgs.prompt` so per-command parsers never see it.
  `DashboardHost` and `SimFlowTerminal` are both keyed by project
  dir now, so opening the dashboard against one project does not
  collide with another and `/reset DM0 --project /repo/model-b`
  does the expected thing in a multi-project workspace.
- M11 made the extension installable as an internal VSIX. The
  README covers `cargo install --path tools/sim-flow`,
  `npm run package`, `code --install-extension ...`, settings,
  slash commands, and LLM backends. `src/cli/bundled.ts` adds a
  platform-aware lookup for `bin/<os>-<arch>/sim-flow[.exe]`
  inside the VSIX (populated only for distribution builds - local
  developer builds rely on `$PATH`). `npm run package` produces a
  clean ~95 KB VSIX with zero bundled binaries. Per-platform
  smoke-test and actually populating the `bin/` tree are tracked
  as post-install follow-ups rather than milestone blockers.

**Superseded by [Phase 9](./09-phase-orchestrator-driven-sessions.md).**
M1-M11 above shipped, but the chat-driven orchestration logic in
M6-M8 (handleStep / runStepLlm / handleStepContinuation /
buildMessageHistory / extractArtifacts / per-step path tables /
the `cli` LLM backend) was removed in Phase 9 M5 when sim-flow
became the master and the extension became a renderer over the
JSONL session protocol. The dashboard, state readers, file
watcher, multi-project resolver, terminal integration for
non-session ops, and packaging plumbing carry forward unchanged.
M9's stub-deletion + terminal-routing work also remains valid;
its dashboard "Run Step" button now spawns a chat-tab session
backed by `sim-flow session ... --jsonl` rather than a
`sim-flow run <step>` terminal invocation.

M12 (DSF hooks) is unchanged — still blocked on Phase 6.
M13 (docs) folded into Phase 9 M6.

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
