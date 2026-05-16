/**
 * Free-function helpers extracted from `webview/host.ts`. None of
 * them touch dashboard state; they're constants / pure utilities
 * that were inlined at file-bottom and grew large enough to drift
 * out into their own module.
 */

import * as vscode from "vscode";

import type { StepMode } from "../../session/protocol-types";

export function buildSimulateAndIterateAppendix(simulatorPath: string): string {
  return [
    "## Simulate and iterate",
    "",
    "After every generated file is written, drive the SystemVerilog through",
    "the simulator the user has configured and iterate until simulation",
    "matches the Foundation model.",
    "",
    `- **Simulator binary**: \`${simulatorPath}\` (configured via the`,
    "  dashboard's Settings tab; `sim-flow.dashboard.verilogSimulatorPath`).",
    "  Invoke it from the project root, not from inside `generated/`.",
    "- **Compile + run**: prefer the `make sim` target from",
    "  `generated/test/Makefile`. If the Makefile's default tool doesn't",
    "  match the configured simulator, edit the Makefile so `make sim`",
    "  drives that simulator (don't add a second flow). The Makefile,",
    "  `sim.f`, and `tb_top.sv` together must produce a working invocation.",
    "- **Per-test runs**: `make sim TEST=<name>` should run a single UVM",
    "  test class. Use it to bisect failures.",
    "",
    "### Iteration loop",
    "",
    "1. Run `make sim`. Capture full stdout/stderr.",
    "2. Classify failures:",
    "   - **Compile / elaboration errors** -- syntax, missing typedefs,",
    "     port-width mismatches, struct layout drift. Fix the offending",
    "     file under `generated/`. NEVER edit `src/` or `tests/` to make",
    "     the SV happy -- the Foundation model is the reference.",
    "   - **UVM runtime errors** -- typically TB wiring (config_db, virtual",
    "     interfaces, analysis-port hookups). Fix in `generated/test/`.",
    "   - **Scoreboard mismatches** -- the SV behavior diverges from the",
    "     Rust model. The Rust model is the source of truth: re-derive the",
    "     RTL combinational body from `evaluate()`, the registered state",
    "     from `update()`, and re-emit the offending module. Confirm the",
    "     payload struct field order matches Rust before assuming the bug",
    "     is in the logic.",
    "3. Re-run `make sim`. Repeat until every test in the test plan passes.",
    "4. If you hit the same failure twice in a row without progress, stop",
    "   and report what you tried, the failing test, and the relevant",
    "   waveform / log excerpt. Don't churn the same file blindly.",
    "",
    "### Boundaries",
    "",
    "- All edits remain under `generated/`. `src/`, `tests/`, `.sim-flow/`,",
    "  and `docs/` stay read-only for this generation, including during",
    "  the simulate-and-iterate loop.",
    "- Don't lower coverage or skip tests to make simulation pass. If a",
    "  test is genuinely wrong (e.g. it encoded a Rust-only assumption),",
    "  call it out in your reply rather than silently disabling it.",
    "- Don't hand-edit waveforms or VCD output -- those are diagnostic",
    "  artifacts, not source. Fix the SV.",
  ].join("\n");
}

export function randomNonce(): string {
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let out = "";
  for (let i = 0; i < 32; i++) {
    out += chars[Math.floor(Math.random() * chars.length)];
  }
  return out;
}

export function readStepModeSetting(config: vscode.WorkspaceConfiguration): StepMode {
  const raw = (config.get<string>("flow.stepMode") ?? "manual").trim();
  return raw === "auto" ? "auto" : "manual";
}
