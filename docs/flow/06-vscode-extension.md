# 6. VS Code Extension (renderer)

## Purpose

Define the architecture of the VS Code extension that surfaces the
`sim-flow` orchestrator inside the editor. As of Phase 9, the
extension is a **renderer** over the host-neutral session protocol
defined in [07-session-protocol.md](07-session-protocol.md); the
orchestrator (sim-flow Rust binary) is the master of every session.
Earlier revisions of this doc described a chat-participant-driven
adapter that owned much of the session logic in TypeScript; that
design has been retired in favor of orchestrator-driven sessions.
See the Phase 9 plan for the migration steps.

## Design Goals

1. The extension contains **zero step knowledge**. Step graph,
   instruction loading, message assembly, history policy, artifact
   path conventions, gate semantics, and state advancement live in
   `sim-flow` Rust.
2. Reuse VS Code's chat, editor, and terminal surfaces. Do not rebuild
   chat widgets or text editors.
3. Render the orchestrator's session events (`AssistantText`,
   `ArtifactWritten`, `RequestUserInput`, `GateResult`, etc.) into
   VS Code chat output, file watchers, and notifications.
4. Provide LLM-call dispatch for the host-mediated backends VS Code
   has special access to: in particular `vscode.lm` for
   subscription-backed Copilot / Claude / Codex access. Other LLM
   backends (HTTP-based Anthropic, OpenAI, Ollama, LM Studio, plus
   subprocess CLI-agent wrappers) live in `sim-flow` Rust and are
   accessible from any host.
5. Cross-IDE portability is a property of the protocol, not the
   extension. RustRover or another IDE implements the same JSONL
   protocol to host the same flow.

## Non-Goals

- Implementing orchestration logic. The extension does not parse
  step IDs, decide when to advance state, or assemble LLM message
  arrays.
- Owning the LLM backend matrix beyond `vscode.lm`. HTTP and
  CLI-subprocess backends live in `sim-flow` Rust.
- Owning artifact-write conventions, file-validation, or build/test
  loops. Those are orchestrator concerns and execute inside
  `sim-flow`.
- Cross-IDE portability inside the extension package. The extension
  is VS-Code-specific; portability is supplied by the protocol.

## High-Level Architecture

```text
                  +------------------------------------------+
                  |  sim-flow orchestrator (Rust)            |
                  |  - step graph, message assembly          |
                  |  - artifact write & validation           |
                  |  - per-step tool catalog & sandboxing    |
                  |  - build/test/coverage iteration loop    |
                  |  - gate evaluation, state advance        |
                  +-------------------+----------------------+
                                      |
                            JSONL session protocol
                            (stdio, line-delimited)
                                      |
                                      v
+------------------------------------+ | +-----------------------------+
|  VS Code Extension (renderer)      | | |  Terminal CLI               |
|                                    | | |  sim-flow session ...       |
|  - chat-participant pump           | | |  - prints to stdout         |
|  - dashboard webview (read-only    | | |  - reads from stdin         |
|    over `sim-flow status --json`)  | | |  - dispatches LLM calls to  |
|  - dispatches RequestLlmResponse   | | |    a CliAgent (claude /     |
|    to `vscode.lm` (Copilot/Claude/ | | |    codex / gh copilot /     |
|    Codex via VS Code subscriptions)| | |    HTTP backend)            |
|  - renders ToolInvoked / artifact  | | |                             |
|    notifications, diff hints, etc. | | +-----------------------------+
+------------------------------------+
```

The extension consists of:

1. A **chat participant** registered as `@sim-flow` whose handler
   spawns `sim-flow session <step>.<kind> --jsonl` as a subprocess,
   pumps the JSONL protocol, renders events to the chat stream,
   captures user replies, and dispatches `RequestLlmResponse` events
   to its LLM clients.
2. A **dashboard webview** rendering state read from
   `sim-flow status --json`, `runs --json`, `gate --json`, and the
   project's `.sim-flow/` files. Buttons that _do_ things just spawn
   `sim-flow session ...` (for sessions) or invoke other CLI
   subcommands (`init`, `reset`, `advance`).
3. A **file-system watcher** on `.sim-flow/state.toml`,
   `.sim-flow/critiques/`, and `.sim-flow/experiments.db` so the
   dashboard auto-refreshes when artifacts land.
4. A **VS Code Language Model dispatcher** (`vscode.lm`) that
   responds to orchestrator-emitted `RequestLlmResponse` events.
   The dispatcher now resolves an explicit adaptation tuple per
   request:
   - transport/backend kind
   - runtime capability profile
   - model-family profile
   - response normalizer
     Other LLM backends are accessed via sim-flow's own `CliAgent` /
     HTTP layer; the extension does not duplicate them.

## Primary surface: chat-panel + dashboard webviews

The `@sim-flow` chat participant still ships (slash commands stay
useful for one-shot probes -- `/status`, `/runs`), but **most**
user interaction now happens through two webviews driven by a live
JSONL pump (`src/session/socketPump.ts`):

1. **Chat-panel webview** (`src/chatPanel/`) — multi-turn transcript
   and composer. Renders `AssistantText`, `ToolInvoked`,
   `ArtifactWritten`, `Diagnostic`, `BuildOutput` (with `stdout_tail`
   and `stderr_tail` on non-zero exit), `GateResult`,
   `StateAdvanced`, and `Followup` quick-action chips.
   `RequestUserInput`'s `prompt` + `placeholder` fields render as
   a banner above the textarea so the user knows what's being
   asked. `LlmEnd.tool_calls` flows back structurally when the
   backend emits native tool calls (the fenced fallback parser
   stays as a safety net).
2. **Dashboard webview** (`src/webview/`) — per-step rail with
   action buttons. Button clicks dispatch `HostEvent::RunStep`,
   `RunCritique`, `RunGate`, `Advance`, `Reset`, `Shutdown` over
   the **live JSONL socket** -- not as fresh `sim-flow` CLI
   invocations. The pump tracks sub-session brackets
   (`SubSessionStarted` / `SubSessionEnded`) and gates the buttons
   on `inSubSession`; `StepModeChanged` keeps the manual/auto
   toggle in sync when the orchestrator flips mode for its own
   reasons (cap-exceeded drop to manual, etc.).

The legacy CLI-spawn path lives on as a fallback for
chat-participant slash commands and the "Attach to Running
Watcher" picker, but it's no longer the primary surface.

```text
Dashboard click "Run Step DM2c"
   -> SocketSessionPump.runStep("DM2c", "work")
   -> { event: "run-step", step: "DM2c", kind: "work" }   --> orchestrator
   <-- SubSessionStarted / RequestLlmResponse / LlmChunk / LlmEnd / ToolInvoked / ArtifactWritten / ...
   <-- SubSessionEnded -> dashboard buttons re-enable
   <-- Followup { label: "Retry", action: "/retry" }      -> chat panel renders chip
   <-- RequestUserInput { prompt: "...", placeholder: "..." }
                                                          -> chat panel banner above composer
   ... user types or clicks chip ...
   -> { event: "user-message", text: "..." }              --> orchestrator
```

The fresh-critique invariant, gate refusal logic, instruction
loading, message assembly, milestone-walk scoping, walk vs step
gate evaluation, and DM0-interactive ask-questions flow all happen
in `sim-flow`. The extension sees only protocol events; it cannot
accidentally violate session invariants by editing TS code.

## Slash command compatibility (legacy CLI mode)

For diagnostic / scripted use, `@sim-flow` chat participant slash
commands still spawn fresh CLI subprocesses:

```text
@sim-flow /status          -> sim-flow status --json (rendered as a table)
@sim-flow /step DM0.work   -> sim-flow session DM0.work --jsonl
@sim-flow /step DM0.critique-> sim-flow session DM0.critique --jsonl
@sim-flow /gate DM0        -> sim-flow gate DM0 --json (read-only)
@sim-flow /advance DM0     -> sim-flow advance DM0 --json (gate + mark_passed)
@sim-flow /reset DM0       -> sim-flow reset DM0
@sim-flow /init            -> sim-flow init
@sim-flow /runs ...        -> sim-flow runs --json
```

`--project <path>` continues to be accepted on every command and is
forwarded as a CLI flag.

## Terminal Integration

Retained for one-shot CLI operations (`init`, `reset`) and as a
fallback "open this in a terminal" option for users who prefer to
drive `sim-flow session` themselves outside of chat. The shared
`SimFlowTerminal` from M9 still applies; its scope shrinks to
non-session commands.

## LLM Access

Two paths, decided per-session by the user's `sim-flow.llm.source`
setting and forwarded to sim-flow via a CLI flag:

1. **Host-mediated (`vscode`)**: orchestrator emits
   `RequestLlmResponse` events, the extension dispatches to
   `vscode.lm.selectChatModels(...).sendRequest(...)` and streams
   `LlmChunk` events back. This is the path that uses the user's
   Copilot / Claude / Codex VS Code subscription.
   The request is shaped through the extension's runtime/model-family
   adaptation layer before dispatch, and the streamed response is
   normalized before the orchestrator sees it so reasoning never leaks
   into artifact writes.

2. **Orchestrator-internal**: any backend `sim-flow` knows how to
   reach without VS Code's help. Includes:
   - HTTP (`anthropic`, `openai`, `ollama`, `lmstudio`)
   - Subprocess CLI agents (`claude`, `codex`, `gh-copilot`)
     sim-flow handles these directly; no host events fire.

API keys for HTTP backends still flow through VS Code SecretStorage
when configured from the extension; the extension exports them as
environment variables to the spawned `sim-flow session` subprocess
rather than calling the APIs itself.

## Configuration

Settings contributed under `sim-flow.*`:

- `sim-flow.binaryPath` (string)
- `sim-flow.foundationRoot` (string)
- `sim-flow.llm.source`: `"vscode" | "anthropic" | "openai" |
"ollama" | "lmstudio" | "claude-cli" | "codex-cli" | "gh-copilot"`
- `sim-flow.llm.model` (string)
- `sim-flow.llm.modelFamily` (string; optional explicit adaptation
  override, otherwise inferred from the model id)
- `sim-flow.llm.runtimeProfile` (string; optional explicit runtime
  capability override, otherwise the source/backend default)
- `sim-flow.llm.debugAdaptation` (bool; when true, render extra
  backend/runtime/model-family diagnostics around LLM requests)
- `sim-flow.llm.ollama.baseUrl`, `sim-flow.llm.lmstudio.baseUrl`
- `sim-flow.dashboard.openOnActivate` (bool)
- `sim-flow.chat.scope`: `"session"` (default - each session opens a
  fresh chat tab; the tab closes when `SessionEnd` fires) or
  `"project"` (sticky tab per project; sessions stream into the
  same tab). User-configurable so we can compare both styles before
  committing.

## Activation

`workspaceContains:**/.sim-flow/state.toml` plus the chat participant
registration. The `sim-flow.openDashboard` command also activates.

## Multi-Root Workspaces

Unchanged: the dashboard scans for `.sim-flow/state.toml` under each
workspace folder and prompts the user when more than one project is
present. The chat participant accepts `--project <path>` on every
command and forwards it to `sim-flow`.

## Packaging And Distribution

Internal tool; not published to the VS Code Marketplace.
Distribution is via the VSIX artifact produced by `vsce package`
and attached to CI, installed with `code --install-extension`.
A prebuilt `sim-flow` binary for the platforms the team uses
(macOS Apple Silicon and Linux x86-64 as the baseline; other
targets are added on demand) is bundled inside the extension so
first-run works without requiring users to `cargo install`
separately. The extension falls back to any `sim-flow` on `$PATH`
if the bundled binary fails to launch (architecture mismatch, etc).

## Relationship To Other Architecture Docs

- Session-protocol contract:
  [07-session-protocol.md](07-session-protocol.md).
- Tool taxonomy and code-step iteration loop (DM2c / DM3a / DM3c
  authoring):
  [08-orchestrator-tools.md](08-orchestrator-tools.md).
- Step semantics (work / critique / gate):
  [02-direct-modeling-flow.md](02-direct-modeling-flow.md).
- Experiments DB schema:
  [04-experiment-tracking.md](04-experiment-tracking.md).
- Adapter pattern:
  [study-workflow-and-agent-adapters.md](../study-workflow-and-agent-adapters.md).

## What Was Retired In Phase 9

For traceability, here's what Phase 9 M5 removed from the
extension when sim-flow became the master and the extension
became a renderer:

**Deleted modules:**

- `src/participant/session.ts` + `session.test.ts` —
  `buildMessageHistory`, `extractSessionTag`, `LlmMessage`
  interface, `TurnView` shape. Session tagging now travels in
  `ChatResult.metadata` as a `pumpKey` that looks up a live
  `SessionPump` in the registry.
- `src/participant/artifacts.ts` + `artifacts.test.ts` —
  `extractArtifacts`, `writeArtifacts`,
  `ARTIFACT_CONVENTION_SYSTEM`, `expectedArtifactPaths`,
  `isSafeRelativePath`. The orchestrator parses fenced artifact
  blocks itself in `tools/sim-flow/src/session/orchestrator.rs`.
- `src/participant/instructions.ts` — instruction-slug map and
  file loader. The orchestrator's `instructions::load` is now
  the only loader; the extension never resolves slugs.
- `src/llm/cli.ts` — the CLI fallback backend that opened a
  VS Code terminal and ran `sim-flow run <step>`. Replaced by
  the `SessionPump` spawning `sim-flow session ... --jsonl`.

**Deleted handlers:**

- `participant/handlers.ts::handleStep`
- `participant/handlers.ts::handleStepContinuation`
- `participant/handlers.ts::runStepLlm`
- `participant/handlers.ts::buildCritiqueInputs`

**Removed surfaces:**

- `cli` value of the `sim-flow.llm.source` enum (no longer
  needed; standalone CLI use goes through `sim-flow session`
  directly).
- The artifact-write convention as a TS-side parser (still
  documented in the agent's system prompt by the orchestrator,
  but parsing happens in Rust now).
- Per-step path tables in TS.
- The M9 dashboard "Run Step" → terminal flow (now opens a chat
  tab that hosts a `SessionPump`).

**Carried forward unchanged:**

- Dashboard webview, including the step rail, gate diamonds, and
  experiments / baselines / sweeps tabs.
- State readers (`src/state/flowState.ts`,
  `src/state/critiques.ts`, `src/state/experiments.ts`).
- File watcher (`src/state/watcher.ts`).
- Multi-project resolver in `src/context.ts`.
- `SimFlowTerminal` for non-session CLI ops (`sim-flow.init`,
  `sim-flow.resetStep`, etc).
- VSIX packaging + bundled-binary lookup.
- Every LLM backend in `src/llm/` except `cli` — they're now
  the host-side dispatchers for `RequestLlmResponse` events
  arriving from the orchestrator.
