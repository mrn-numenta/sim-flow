# sim-flow VS Code Extension

Internal VS Code adapter for the
[sim-flow](../../tools/sim-flow/) AI-assisted modeling orchestrator.
Provides a Flow Dashboard, a `@sim-flow` chat participant, and
terminal integration for long-running CLI runs.

This extension is **not published to the VS Code Marketplace**.
Distribution is via a VSIX produced by `vsce package` and installed
locally with `code --install-extension`.

See:

- [docs/architecture/ai-flow/06-vscode-extension.md](../../docs/architecture/ai-flow/06-vscode-extension.md) — design.
- [docs/plan/ai-flow/08-phase-vscode-extension.md](../../docs/plan/ai-flow/08-phase-vscode-extension.md) — milestone breakdown.

## Install

### Prerequisites

- **VS Code 1.95+**.
- **The `sim-flow` CLI** reachable from the extension at activate
  time. The resolver tries, in order:
  1. `sim-flow.binaryPath` setting (absolute path, overrides
     everything).
  2. `sim-flow` on `$PATH`.
  3. A binary bundled inside the VSIX at
     `bin/<platform>-<arch>/sim-flow[.exe]` (only populated for
     builds intended for distribution — local dev VSIXes skip this).
- A **sim-foundation checkout** for instruction templates and for
  the `sim-flow` binary itself.

### Build the binary locally

```bash
cd <sim-foundation-checkout>
cargo install --path tools/sim-flow
```

This drops `sim-flow` into `~/.cargo/bin/`. Make sure that directory
is on `$PATH`.

### Build and install the VSIX

```bash
cd <sim-foundation-checkout>
npm --prefix tools/sim-flow/extensions/sim-flow-vscode install
npm --prefix tools/sim-flow/extensions/sim-flow-vscode run package
code --install-extension ./tools/sim-flow/extensions/sim-flow-vscode/build/sim-flow-vscode-<version>.vsix
```

After install, reload the VS Code window. A project is detected
automatically when a workspace folder contains `.sim-flow/state.toml`
(or any descendant does).

### Uninstall or update

```bash
code --uninstall-extension numenta.sim-flow-vscode
```

Then reinstall with a new VSIX.

## Configure

Open **Settings → sim-flow** (or edit `.vscode/settings.json`):

| Key                                 | Default                     | Notes                                                                                      |
| ----------------------------------- | --------------------------- | ------------------------------------------------------------------------------------------ |
| `sim-flow.binaryPath`               | `""`                        | Explicit CLI path. Empty falls back to `$PATH`, then to a bundled binary.                  |
| `sim-flow.foundationRoot`           | `""`                        | Absolute path to your sim-foundation checkout. Threaded to the CLI as `--foundation-root`. |
| `sim-flow.llm.source`               | `"vscode"`                  | `vscode` / `anthropic` / `openai` / `ollama` / `lmstudio` / `cli`.                         |
| `sim-flow.llm.model`                | `""`                        | Model id for the chosen source; empty uses the source's default.                           |
| `sim-flow.llm.ollama.baseUrl`       | `http://localhost:11434/v1` | Override for a non-default Ollama host.                                                    |
| `sim-flow.llm.lmstudio.baseUrl`     | `http://localhost:1234/v1`  | Override for a non-default LM Studio host.                                                 |
| `sim-flow.dashboard.openOnActivate` | `false`                     | Auto-open the Flow Dashboard when a project is detected.                                   |

Set API keys (Anthropic / OpenAI / optional Ollama+LM Studio auth)
via the **sim-flow: Set LLM API Key** command; they land in VS Code
SecretStorage.

## Use

### Flow Dashboard

Command palette → **sim-flow: Open Flow Dashboard**. If your workspace
holds more than one sim-flow project, you'll get a picker. The
dashboard:

- Shows the step rail with gate diamonds.
- Surfaces per-step findings, gate failures, and action buttons.
- Streams the experiments/baselines/sweeps tabs from the project's
  `experiments.db`.
- Auto-refreshes on any `.sim-flow/` change via a project-scoped
  file watcher.

Buttons:

- **Run Step** → dispatches `sim-flow.runStep`, which opens a named
  `sim-flow` terminal and shells `sim-flow run <step>`.
- **Reset** → shells `sim-flow reset <step>` through the same
  terminal.
- **Run Gate** → runs the gate in-process via the CLI wrapper (fast,
  JSON-returning; no terminal).
- **Open Critique** → opens the `<step>-critique.md` file.

### Chat participant

Summon with `@sim-flow` in Copilot Chat. Slash commands:

| Command                                                                     | What it does                                                             |
| --------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| `/status`                                                                   | Flow + gate status.                                                      |
| `/runs [--workload W] [--candidate C] [--study S] [--sweep ID] [--limit N]` | Recent runs.                                                             |
| `/gate [step] [--candidate C]`                                              | Structural gate check.                                                   |
| `/reset <step>`                                                             | Reset a step + cascade downstream gates.                                 |
| `/step <step>.work`                                                         | Start a work session; streams from the configured LLM backend.           |
| `/step <step>.critique`                                                     | Start a critique session (fresh message history, sees artifact summary). |
| `/init`                                                                     | Initialize `.sim-flow/state.toml` in the current workspace.              |

Every command accepts `--project <path>` to target a specific
project in a multi-project workspace, e.g.
`@sim-flow /status --project /repos/model-a`.

### LLM backends

- **`vscode`** — routes through `vscode.lm.selectChatModels`.
  Requires Copilot or another Language Model API provider.
- **`anthropic`** — direct Anthropic Messages API; needs an API key.
- **`openai`** — direct OpenAI Chat Completions; needs an API key.
- **`ollama`** — local Ollama server's OpenAI-compatible endpoint.
  No key needed for default local installs.
- **`lmstudio`** — local LM Studio server; same shape as Ollama.
  Set `sim-flow.llm.model` to a model loaded in LM Studio.
- **`cli`** — opens a VS Code terminal and runs `sim-flow run
<step>` interactively; the chat picks up the critique file once
  it lands.

## Development

```bash
cd tools/sim-flow/extensions/sim-flow-vscode
npm install
```

| Script                      | Purpose                                                          |
| --------------------------- | ---------------------------------------------------------------- |
| `npm run compile`           | `tsc` extension + webview type-check + `esbuild` webview bundle. |
| `npm run compile:extension` | Extension only.                                                  |
| `npm run compile:webview`   | Webview bundle only.                                             |
| `npm run watch`             | Recompile extension on save.                                     |
| `npm run lint`              | ESLint, zero warnings tolerated.                                 |
| `npm run format`            | `prettier --write`.                                              |
| `npm run format:check`      | Used by CI.                                                      |
| `npm run test`              | `vitest run`.                                                    |
| `npm run test:watch`        | `vitest` in watch mode.                                          |
| `npm run ci`                | `compile && lint && format:check && test`.                       |
| `npm run package`           | Stage the extension under `build/` and produce a VSIX there. |

Run the dev host: open `tools/sim-flow/extensions/sim-flow-vscode/` in VS Code and
press **F5** ("Run Extension"). Open any folder with
`.sim-flow/state.toml` to trigger activation.

### Bundled binaries

`npm run package` now stages the packaged extension under:

```text
build/stage/bin/
  <platform>-<arch>/sim-flow[.exe]
  <platform>-<arch>/libpdfium.{dylib,so} | pdfium.dll
```

The bundling script copies `target/release/sim-flow` plus the matching
PDFium shared library from `tools/sim-flow/vendor/pdfium/` into that
staging tree before it invokes `vsce package`. Local developer builds
that rely on `$PATH` can still skip the bundled binary entirely; the
resolver silently falls through when the packaged binary is missing.
