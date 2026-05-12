# 7. Session Protocol

## Purpose

Define the host-neutral protocol over which `sim-flow` (the
orchestrator) communicates with a host process during an interactive
session. Hosts include the VS Code extension, a future RustRover /
JetBrains plugin, and `sim-flow`'s own TerminalHost (used when
running `sim-flow session ...` from a plain terminal).

The protocol is the contract that lets us add new hosts without
changing orchestrator code, and add new orchestrator capabilities
without breaking existing hosts.

## Invocation

```text
sim-flow session <step>.<kind> [--jsonl] [--project <path>]
                                [--foundation-root <path>]
                                [--llm-backend <name>]
                                [--llm-model-family <id>]
                                [--llm-runtime-profile <id>]
                                [--llm-debug-adaptation]
                                [--candidate <name>]
```

- `--jsonl` selects JsonlHost. Without it, sim-flow runs in
  TerminalHost mode (interactive stdin/stdout for the user; LLM
  calls dispatched to a built-in `CliAgent` selected by
  `--llm-backend`).
- `--llm-backend` is opaque to the orchestrator; in JsonlHost mode
  it is echoed back inside `RequestLlmResponse` so the host can
  pick its own dispatcher. In TerminalHost mode it selects which
  `CliAgent` impl sim-flow uses internally.
- `--llm-model-family` and `--llm-runtime-profile` are optional
  explicit adaptation overrides. When omitted, the host or built-in
  agent infers the family from `--llm-model` and uses the backend's
  default runtime profile.
- `--llm-debug-adaptation` asks the host or built-in agent to surface
  backend/runtime/model-family diagnostics around each LLM request.
- All other flags are conventional (project dir, foundation root,
  candidate scope for DSF steps).

## Transport

Line-delimited JSON over stdio. Each line is exactly one JSON
object. UTF-8. No batching, no multiplexing, no continuation lines.
The first event in either direction is the handshake (below).

The wire format does not depend on stdio specifically; any
line-delimited JSON channel (Unix domain socket, TCP loopback) can
carry the same protocol. VS Code uses stdio; future hosts may pick
something else.

## Versioning And Handshake

```jsonc
// host -> orchestrator (first message)
{
  "event": "Hello",
  "protocolVersion": "1",
  "host": { "name": "sim-flow-vscode", "version": "0.2.0" },
  "capabilities": ["text", "markdown", "user-input", "llm-request",
                   "tool-notifications", "followups"]
}

// orchestrator -> host (in reply)
{
  "event": "HelloAck",
  "protocolVersion": "1",
  "simFlow": { "version": "0.6.0" },
  "session": { "step": "DM0", "kind": "work", "candidate": null },
  "stepDescriptor": { /* see Describe schema below */ }
}
```

Mismatched `protocolVersion` is fatal. The orchestrator emits
`SessionEnd { reason: "protocol-mismatch" }` and exits.
Capabilities are advisory; the orchestrator skips events the host
hasn't advertised support for.

## Event Schema

Defined as a Rust enum (`sim_flow::session::protocol::Event`)
serialized with `serde_json` and exported as JSON Schema via
`schemars` to `tools/sim-flow/docs/flow/session-protocol.schema.json`
on every build. Hosts that aren't Rust generate or hand-mirror types
from the schema.

### Orchestrator -> Host

| Event | Purpose |
| --- | --- |
| `HelloAck` | Handshake reply; carries the step descriptor. |
| `AssistantText` | A chunk of text from the LLM to render. `text: string`, `final: bool`. |
| `RequestUserInput` | Pause and wait for the user's reply. Optional `prompt` (banner / question text) and `placeholder` (textarea hint) so the host can tell the user what is being asked. |
| `RequestLlmResponse` | Ask the host to run an LLM call. `requestId`, `messages`, `tools`, `backend`, plus optional adaptation override / debug fields. |
| `ArtifactWritten` | A project file was just written by the orchestrator. `path`, `bytes`. |
| `ToolInvoked` | Notification only; the orchestrator already executed the tool. `name`, `args`, `status`, `durationMs`. |
| `PhaseChanged` | Code-step loop transition. `phase` is one of `author`, `build`, `test`, `coverage`, `done`. |
| `BuildOutput` | Build / test runner output. `command`, `stdoutTail`, `stderrTail`, `exitCode`. Hosts that surface failures should render the tails on non-zero exits. |
| `GateResult` | Gate evaluation result. `step`, `clean: bool`, `failures: [...]`. |
| `StateAdvanced` | The orchestrator advanced `current_step`. `from`, `to`. |
| `StepModeChanged` | Step-mode toggled. `mode` is `auto` or `manual`. Hosts mirror this on their toggle UI so the orchestrator's automatic flips (cap-exceeded drop to manual, etc.) stay visible. |
| `SubSessionStarted` | A new work / critique sub-session opened. `step`, `kind`. Hosts use this to bracket per-sub-session UI state (disable per-step buttons while busy). |
| `SubSessionEnded` | The current sub-session closed. `step`, `kind`, `outcome`. Pairs with `SubSessionStarted`. |
| `Followup` | Suggested next action for the host to render as a button. `label`, `action`. Hosts that declared the `followups` capability ship the action back via `UserMessage` on click. |
| `Diagnostic` | Non-fatal diagnostic for the host to display (e.g. truncated context warning). |
| `SessionEnd` | Session finished. `reason` is one of `completed`, `cancelled`, `error`, `protocol-error`, `protocol-mismatch`, `runaway-guard`. Optional `message`. |

### Host -> Orchestrator

| Event | Purpose |
| --- | --- |
| `Hello` | Handshake. |
| `UserMessage` | The user's reply to a `RequestUserInput`, OR a freeform message used as a typed equivalent of clicking a `Followup` chip. Also accepted as an idle-mode Q&A turn between sub-sessions (orchestrator dispatches a side-conversation LLM turn). |
| `LlmChunk` | Streaming LLM response chunk. `requestId`, `text`. |
| `LlmEnd` | LLM response finished. `requestId`, `stopReason`, optional `toolCalls`. Hosts that support native function-calling should populate `toolCalls` so the orchestrator's native dispatch path fires; an empty array makes the orchestrator fall back to fenced-block extraction from the assistant text. |
| `LlmError` | LLM dispatch failed. `requestId`, `kind`, `message`. |
| `RunStep` | Manual-mode command: run a sub-session. `step`, `kind` is `work` or `critique`. |
| `RunCritique` | Manual-mode command: explicit alias for `RunStep { kind: "critique" }`. |
| `RunGate` | Manual-mode command: re-evaluate the step's gate without re-running work / critique. `step`. |
| `Advance` | Manual-mode command: gate-check and advance `current_step`. `step`. On refused advance, the orchestrator emits `RequestUserInput`. |
| `Reset` | Manual-mode command: wipe a step's artifacts so the user can re-run it cleanly. `step`. |
| `SetStepMode` | Toggle between `auto` and `manual` step mode. |
| `Shutdown` | Tear the orchestrator down. Cancels any in-flight sub-session. |
| `Cancel` | User cancelled the session. |

**Capability gating.** `Hello.capabilities` declares what the host
can do. The orchestrator routes events accordingly:

- `"followups"` — host receives `Followup` events and ships actions
  back via `UserMessage`. Hosts without it still see the suggestion
  in `AssistantText` markdown.
- `"user-input"` — host accepts `RequestUserInput`. Without it the
  orchestrator surfaces a `Diagnostic` and ends the session.
- `"llm-request"` — host dispatches `RequestLlmResponse` and
  streams back `LlmChunk` / `LlmEnd`. Required for any sub-session.
- `"tool-notifications"` — host accepts `ToolInvoked` /
  `ArtifactWritten` events.
- `"markdown"` — host accepts markdown formatting in
  `AssistantText`. Plain-text hosts opt out.

## Lifecycle

1. Host spawns `sim-flow session <step>.<kind> --jsonl`.
2. Host sends `Hello`. Orchestrator responds with `HelloAck`
   carrying the step descriptor.
3. Orchestrator runs the session loop:
   - Possibly emits `AssistantText` (instruction-seeded opening).
   - Possibly emits `RequestLlmResponse`; host streams back
     `LlmChunk` / `LlmEnd`.
   - Possibly emits `ToolInvoked` notifications as it reads /
     writes / searches files.
   - For code steps, emits `PhaseChanged` and `BuildOutput`.
   - Emits `RequestUserInput` when stuck or when the step calls
     for human review; host sends `UserMessage` in reply.
4. When the step's success conditions are met, the orchestrator
   evaluates the gate. On success it runs `mark_passed` + `save`
   and emits `StateAdvanced`. On failure it emits `GateResult`
   with the failures and either retries (code steps with iteration
   left) or yields `RequestUserInput`.
5. Orchestrator emits `SessionEnd` and exits.

The host may emit `Cancel` at any point. The orchestrator stops
the LLM stream (sends back a `Cancel` signal to its own LLM
client), aborts in-flight tools, and emits `SessionEnd
{ reason: "cancelled" }`.

## Step Descriptor

The `HelloAck.stepDescriptor` payload tells the host what kind of
session it is hosting:

```jsonc
{
  "step": "DM2c",
  "kind": "work",
  "instructionPath": "/abs/path/to/dm2c-model-impl-plan.md",
  "phases": ["author", "build", "test"],
  "tools": ["read_file", "list_dir", "write_file", "search"],
  "expectedArtifacts": ["src/model/", "tests/"],
  "predecessorInputs": [
    "spec.md",
    "targets.md",
    "analysis/decomposition.md",
    "analysis/data-movement.md",
    "analysis/pipeline-mapping.md",
  ],
}
```

Hosts use this for display only (e.g., a header like "DM2c work
session, phases: author -> build -> test"). They do not need to
interpret it for control flow.

The same descriptor is available outside a session via
`sim-flow describe <step>.<kind> --json`, used by the dashboard
and by hosts that want to pre-render UI before launching the
session.

## Tool Notifications

The orchestrator executes tools itself (it has filesystem and
subprocess access). Tool events are _informational_ for the host:
they let the chat UI show "reading src/model/lib.rs" inline, with
no execution responsibility on the host side.

If we ever sandbox tools (require user approval for shell commands,
for example), we add `RequestToolApproval` / `ToolApprovalResult`
events. The current protocol does not need them.

## LLM Tool Use Inside `RequestLlmResponse`

When the orchestrator advertises tools to the LLM (per
[08-orchestrator-tools.md](08-orchestrator-tools.md)), the
`RequestLlmResponse` event carries the tool catalog the host should
forward to the LLM client:

```jsonc
{
  "event": "RequestLlmResponse",
  "requestId": "lr-42",
  "backend": "vscode",
  "modelFamilyId": "qwen3_6",
  "runtimeProfileId": "openai_compat_generic",
  "debugAdaptation": true,
  "messages": [...],
  "tools": [
    { "name": "read_file", "description": "...",
      "schema": { /* JSON Schema for args */ } }
  ]
}
```

`modelFamilyId` and `runtimeProfileId` are advisory overrides, not
independent transport selectors. Hosts use them to pin the adaptation
path when inference would be ambiguous or when a debugging session needs
deterministic behavior. `debugAdaptation` asks the host to include the
resolved backend/runtime/model-family/capability tuple in diagnostics and
error reporting.

If the LLM emits a tool call, the host returns it inside
`LlmChunk.toolCalls`. The orchestrator parses the tool call,
executes it, threads the result back into the LLM message
history, and emits a fresh `RequestLlmResponse` for the next turn.

For LLM clients that don't support native tool-use (some CLI
agents), the orchestrator falls back to fenced-block parsing
(see [08-orchestrator-tools.md](08-orchestrator-tools.md)). The
host doesn't need to know which mode is in use - it just relays
`LlmChunk` text.

## Errors

- Protocol-level errors (malformed JSON, unknown event,
  version mismatch) cause `SessionEnd { reason: "protocol-error" }`.
- Tool errors are non-fatal: the orchestrator surfaces them via
  `Diagnostic` and may retry.
- LLM errors (`LlmError` from host) are surfaced to the user via
  `Diagnostic`. The orchestrator may attempt a fallback backend
  (configurable) or emit `RequestUserInput { prompt: "LLM failed:
retry, switch backend, or cancel?" }`.

## Backwards Compatibility

The protocol carries a `protocolVersion` string. Breaking changes
(removed events, changed semantics) bump it. New optional fields
or new events are non-breaking and tolerated by older hosts (they
ignore unknown events).

## Schema And Code Generation

Single source of truth: the Rust enum
`sim_flow::session::protocol::Event` in
`tools/sim-flow/src/session/protocol.rs`. CI emits
`session-protocol.schema.json` next to this doc on every build.
The VS Code extension generates `src/session/protocol.types.ts`
from the schema during `npm run compile`.

Future RustRover / IntelliJ host: same schema, generate Kotlin /
Java types from it.

## Reference Implementations

- `sim-flow::session::host::JsonlHost` (Rust) - production JSONL
  pump.
- `sim-flow::session::host::TerminalHost` (Rust) - in-process host
  for CLI use.
- `sim-flow::session::host::TestHost` (Rust) - in-memory recorder
  for unit / integration tests.
- VS Code: `extensions/sim-flow-vscode/src/session/host.ts` -
  spawns the subprocess, pumps the protocol.
