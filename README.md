# sim-flow

`sim-flow` is the AI-assisted workflow orchestrator for sim-foundation.

It drives the modeling flows, manages per-project state under `.sim-flow/`,
loads step prompts, runs work and critique sessions, evaluates gates, and
tracks experiment runs and baselines.

## Status

- `sim-flow` is primarily a CLI tool, not a public Rust library.
- The package keeps a library target only to support the `sim-flow` binary,
  integration tests, and internal helper binaries.
- Internal Rust modules live under `src/__internal/` and are not intended as a
  supported external API.

## Main Commands

Run the CLI with:

```sh
cargo run -p sim-flow -- <command> ...
```

Common entrypoints:

```sh
# initialize a Direct Modeling project in the current directory
cargo run -p sim-flow -- init --flow direct-modeling

# inspect project state
cargo run -p sim-flow -- status

# run the current step
cargo run -p sim-flow -- run

# run gate validation only
cargo run -p sim-flow -- gate DM0 --json

# create a new model project from the built-in template
cargo run -p sim-flow -- new model my-model --destination /tmp
```

## Documentation

Detailed design and workflow docs live under `tools/sim-flow/docs/flow/`.

- `01-workflows.md`: workflow overview
- `02-direct-modeling-flow.md`: Direct Modeling Flow details
- `03-design-study-flow.md`: Design Study Flow details
- `04-experiment-tracking.md`: run indexing, baselines, and sweeps
- `05-templates.md`: generated project templates
- `06-vscode-extension.md`: editor host architecture
- `07-session-protocol.md`: JSONL session protocol
- `08-orchestrator-tools.md`: tool-call contract
- `cli-json.md`: machine-readable CLI output

## VS Code Extension

`sim-flow` also ships with an internal VS Code extension under:

`tools/sim-flow/extensions/sim-flow-vscode/`

Useful entrypoints:

```sh
# build + install the extension locally
./scripts/install-vscode-extension.sh
```

- Extension README:
  `tools/sim-flow/extensions/sim-flow-vscode/README.md`
- Extension architecture notes:
  `tools/sim-flow/docs/flow/06-vscode-extension.md`

The dashboard's `Spec:` field is a spec path field, not a raw text box. The
extension persists the typed path per project, passes it to `sim-flow auto`
as `--spec <path>`, and `sim-flow` ingests that file into
`.sim-flow/source-spec.*`, `.sim-flow/source-spec-toc.md`, and
`.sim-flow/spec-pages/` before the first DM0 turn.

## Internal Binaries

The package also contains a few internal developer utilities under
`src/bin/`, such as:

- `session_protocol_schema`
- `probe_ingest`
- `pty_inject_probe`
- `dm_flow_smoke`

These are maintenance and debugging tools, not public examples.
