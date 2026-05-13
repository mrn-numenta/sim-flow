# 8. Orchestrator Tools And Code-Step Iteration

## Purpose

Define how `sim-flow`'s orchestrator gives an LLM agent enough
filesystem and shell access to author code (DM2c, DM3a, DM3c)
without ceding control of the flow. Code-authoring steps need
iterative build / test / coverage cycles that don't fit into a
single LLM turn; this doc defines the structure.

The design preserves the four orchestrator-driven goals stated in
the architecture pivot:

1. **Strict flow** - the orchestrator decides what runs and when.
2. **Drift prevention** - the agent's capabilities are bounded
   per (step, kind) and per phase.
3. **Bounded context** - validation runs in dedicated phases with
   iteration limits; no runaway agent loops.
4. **Single source of truth** - tool execution is centralized in
   sim-flow Rust; hosts only render notifications.

## Two Classes Of Capability

The orchestrator distinguishes between:

### Agent-callable tools

Bounded I/O the agent uses to do its job within a turn. The
orchestrator advertises these to the LLM at the start of each
session via the `tools` field of `RequestLlmResponse`. Today's
catalog:

| Tool         | Args                                     | Notes                                      |
|--------------|------------------------------------------|--------------------------------------------|
| `read_file`  | `path: string` (relative)                | Read a project file. Path-sandboxed.       |
| `list_dir`   | `path: string` (relative)                | List a project directory.                  |
| `write_file` | `path: string`, `content: string`        | Write a project file. Path-sandboxed.      |
| `search`     | `pattern: string`, `path?: string`       | Ripgrep-style search within the project.   |

All tools are scoped to the project directory; the orchestrator
rejects path-traversal and absolute paths.

### Orchestrator-only validators

Decisive, deterministic checks the orchestrator runs at known
points in a session. The agent **never** invokes these directly;
they fire automatically when the orchestrator's iteration loop
advances to the corresponding phase.

| Validator         | When                                   | Outcome on failure                |
|-------------------|----------------------------------------|-----------------------------------|
| `cargo check`     | start of `build` phase                 | feed errors to agent, retry       |
| `cargo test`      | start of `test` phase                  | feed test output, retry           |
| `cargo llvm-cov`  | start of `coverage` phase (optional)   | inject report as next-phase input |
| Gate evaluation   | after final phase                      | refuse advance, emit `GateResult` |
| `mark_passed`     | when gate evaluates clean              | emit `StateAdvanced`              |

This separation is the structural drift prevention. The agent has
agency within bounds; the orchestrator decides when those bounds
have been satisfied.

## Per-Step Write Scoping

The orchestrator advertises the **same universal tool catalog** at
every step (`read_file`, `list_dir`, `write_file`, `edit_file`,
`search`, `run_cargo`). Per-step *tool* gating was tried and removed:
it was cosmetic (the fenced artifact-write convention bypassed any
catalog change) and it encouraged tool-name hallucination when an
agent saw a familiar tool absent from the current step's catalog.

What's actually enforced is per-step **write-path scoping**. Every
step descriptor declares `work_write_paths`: the project-relative
prefixes the work session may write to. The same allowlist binds
all three write surfaces:

- `write_file` tool calls (native and fenced).
- `edit_file` tool calls (native and fenced).
- The fenced `` ```<path>\n...\n``` `` artifact-write convention.

A path matches the allowlist when it equals an entry verbatim, or
(for entries ending in `/`) starts with that prefix. Critique
sessions ignore `work_write_paths` and are independently constrained
to a single `docs/critiques/{step_id}-critique.md` file.

| Step  | Work `work_write_paths`              |
|-------|--------------------------------------|
| DM0   | `docs/`                              |
| DM1   | `docs/`                              |
| DM2a  | `docs/`                              |
| DM2b  | `docs/`                              |
| DM2c  | `docs/`                              |
| DM2d  | `src/`, `tests/`, `Cargo.toml`       |
| DM3a  | `docs/`                              |
| DM3b  | `tests/`, `src/`                     |
| DM3c  | `tests/`, `src/`                     |
| DM4a  | `docs/`                              |
| DM4b  | `docs/`                              |

Rejections surface to the agent as a tool-result error (for
`write_file` / `edit_file` calls) or as a `Diagnostic` event with
the artifact path and the allowlist (for fenced artifact writes).
The agent sees the allowed set, so it can either correct the path
or — if the new location is a deliberate widening — flag it for the
operator.

## Iteration Loop For Code Steps

DM2c, DM3a, and DM3c run as multi-phase loops, not single turns.
The orchestrator emits `PhaseChanged` events at each transition:

```text
phase: author
  - tool catalog: read_file, list_dir, write_file, search
  - turn loop: orchestrator <-> LLM via RequestLlmResponse
  - exit when: agent indicates "ready for build" OR turn-limit
    reached

phase: build
  - orchestrator runs `cargo check` (or step-configured equivalent)
  - emits BuildOutput with stdout/stderr tail and exit_code
  - on failure: feed errors as next user message, return to author
    with iteration counter
  - on success: advance to test
  - iteration limit: 5 (configurable)

phase: test
  - orchestrator runs `cargo test [--test ...]`
  - emits BuildOutput
  - on failure: feed output back, return to author
  - on success: advance to coverage (if enabled) or done
  - iteration limit: 5

phase: coverage (optional)
  - orchestrator runs the configured coverage tool
  - injects report as a system message for the next phase
    (typically a final author pass to close gaps) or directly
    feeds into critique
  - advance to done

phase: done
  - orchestrator validates gate
  - on clean: mark_passed, emit StateAdvanced, SessionEnd { reason:
    "completed" }
  - on dirty: emit GateResult with failures, RequestUserInput
    asking the user to triage
```

When iteration limits are exhausted, the orchestrator emits
`RequestUserInput` with a summary of what failed and hands control
to the human. No silent infinite loops.

## LLM Tool-Use API

How the LLM expresses a tool call depends on the backend; the
orchestrator normalizes both:

### Native tool-use (HTTP backends, vscode.lm)

Anthropic Messages API, OpenAI Chat Completions function calling,
and `vscode.lm` (via `LanguageModelTool`) all support a structured
tool-call API. The orchestrator advertises tools in the request,
the model returns a `tool_use` / `tool_calls` payload, the
orchestrator dispatches and threads results back as `tool_result`
turns. Cleanest path; preferred when available.

### Fenced-block fallback (CLI agents)

Some subprocess CLI agents (`claude` non-interactive,
`codex` non-interactive, certain Ollama models) don't expose a
tool-call API. The orchestrator instead instructs the agent to
emit fenced blocks the same way it emits artifact writes:

````text
```tool:read_file
src/model/lib.rs
```
````

The orchestrator's parser handles `tool:<name>` blocks the same way
it handles file-path blocks. Same path-safety rules. The
orchestrator replies with a system message containing the result.

The dispatcher chooses native vs fallback per backend; the agent
sees the same conceptual tool catalog either way.

## Tool Execution Lives In sim-flow

When the orchestrator runs a tool, it executes it directly:

- File I/O via `std::fs`.
- Search via the `grep`/`ripgrep` crate (or shell-out to `rg` if
  available).
- `cargo` commands as subprocesses with structured output flags
  where supported.

The host gets a `ToolInvoked` notification (informational) so the
chat UI can show "reading src/model/lib.rs..." inline. The host
does not run the tool. This keeps tool semantics identical across
hosts and concentrates audit / sandbox concerns in one place.

If we ever need user approval before destructive operations (e.g.,
`shell_command`), we add a `RequestToolApproval` event - explicit
extension of the protocol, not a quiet behavior change.

## Coverage Tooling

`cargo llvm-cov` is the working assumption for line coverage. The
exact tool can be configured per step in the step descriptor.
Coverage is optional in v1 and can be skipped per project.

## Risks

- **Long compile loops**: a fresh DM2c project may spend 30s+ per
  `cargo check`. Mitigation: rely on incremental compilation,
  prefer `cargo check` over `cargo build`, and parse JSON
  diagnostic output to feed errors back fast.
- **Malformed tool calls**: typos like `tool:read_files` should
  reply with an error tool-result, not silently no-op.
- **Workspace mutation outside of step scope**: per-step
  `work_write_paths` enforced at the tool dispatcher AND the
  artifact-write extractor (single source of truth, no fenced-block
  bypass).
- **Coverage tool brittleness**: skip in v1 for any project where
  the tool isn't installed; emit a `Diagnostic` instead of failing
  the gate.

## Relationship To Other Architecture Docs

- Session protocol that carries `RequestLlmResponse` /
  `ToolInvoked` / `PhaseChanged` / `BuildOutput`:
  [07-session-protocol.md](07-session-protocol.md).
- Step semantics this layers under:
  [02-direct-modeling-flow.md](02-direct-modeling-flow.md).
- Renderer (VS Code) that displays tool / phase / build output:
  [06-vscode-extension.md](06-vscode-extension.md).
