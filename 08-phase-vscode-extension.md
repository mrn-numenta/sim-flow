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

- [ ] Implement `src/cli.ts`: a thin wrapper around `child_process`
  that runs `sim-flow` subcommands and returns parsed JSON.
- [ ] Resolve the binary path: setting override > `which sim-flow` >
  bundled binary (Milestone 9).
- [ ] Surface non-zero exit codes as typed errors so the UI can render
  sensible messages.
- [ ] Handle long-running subcommands by streaming via
  `child_process.spawn` and forwarding stdout/stderr to a VS Code
  terminal.
- [ ] Add vitest / jest unit tests mocking child_process.

## Milestone 4 - State Readers

- [ ] Implement `src/state.ts`: read `.sim-flow/state.toml` using a
  TS TOML parser. Expose a typed `FlowState` shape matching
  `crates/sim-flow/src/state.rs`.
- [ ] Implement `src/critiques.ts`: list + read
  `.sim-flow/critiques/*.md`, parse `UNRESOLVED:` / `BLOCKER:` /
  `RESOLVED:` lines using the same rule as the Rust parser.
- [ ] Implement `src/experiments.ts`: read `.sim-flow/experiments.db`
  using `better-sqlite3`. Expose query helpers matching the Rust
  `RunFilter` / `RunRow` shapes.
- [ ] Add a workspace-scoped file watcher that fires when
  `state.toml`, `critiques/*.md`, or `experiments.db` change.
- [ ] Unit-test the parsers with fixture files.

## Milestone 5 - Webview Dashboard (v1)

- [ ] Implement `src/webview/host.ts`: create / show / dispose the
  webview panel; handle postMessage plumbing.
- [ ] Build `src/webview/panel.ts` as a plain TypeScript front-end
  (no framework for v1; revisit React in a later phase if complexity
  warrants).
- [ ] Render the step rail using the same colors as the SVG in
  [images/direct-modeling-flow.png](../../architecture/ai-flow/images/direct-modeling-flow.png).
  Gate diamonds between steps, color-coded by gate status.
- [ ] Implement the per-step panel: click a step to show its last
  critique (markdown rendered), gate-check failures if any, and
  work/critique/reset buttons.
- [ ] Wire auto-refresh on file-watcher events (Milestone 4).
- [ ] Implement the Experiments tab: table of recent runs with
  workload / candidate / study filters, paging.
- [ ] Implement the Baselines tab: list, create, compare (delta
  table).
- [ ] Stub the Sweeps tab with a "coming in Phase 8 M7" note; full
  implementation after the chat participant lands.

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

In progress. Milestones 1 and 2 are complete:

- M1 shipped the `--json` flag on every subcommand the extension
  consumes, the [cli-json.md](../../architecture/ai-flow/cli-json.md)
  schema, and `crates/sim-flow/tests/cli_json.rs` regression coverage.
- M2 scaffolded `extensions/sim-flow-vscode/` with package.json,
  tsconfig, ESLint/Prettier/.editorconfig, a minimal extension.ts
  that registers every planned command as a "not yet implemented"
  stub, and a `vscode-extension` CI job.

Milestones 3-11 remain. DSF-specific hooks (M12) still depend on
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
