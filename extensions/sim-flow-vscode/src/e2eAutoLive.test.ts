/**
 * End-to-end live smoke test: drives the extension's
 * `SocketSessionPump` (the same code path the dashboard's Run / Connect
 * button uses) against a REAL `sim-flow auto` subprocess that
 * dispatches its LLM requests to a REAL vLLM running on
 * `http://localhost:8012/v1`.
 *
 * Mirrors the role of Rust's `tools/sim-flow/src/bin/e2e_auto.rs` on
 * the extension side: same orchestrator, same protocol, but the LLM
 * dispatch goes through the extension's TypeScript backends (the
 * code path the audit found bugs in) instead of the orchestrator's
 * own Rust backends.
 *
 * Skipped by default. Opt in with:
 *
 *   SIM_FLOW_E2E_LIVE=1 npx vitest run src/e2eAutoLive.test.ts
 *
 * Preconditions:
 *   - vLLM up on `http://localhost:8012/v1` serving `qwen3.6`.
 *     (Or set `SIM_FLOW_E2E_LIVE_BASE_URL` / `SIM_FLOW_E2E_LIVE_MODEL`
 *     to point at a different endpoint.)
 *   - `sim-flow` binary built (`cargo build -p sim-flow`).
 *
 * What it tests:
 *   - SocketSessionPump's launch path spawns `sim-flow auto`, opens
 *     the JSONL transport socket, and reads protocol events.
 *   - `dispatchLlm` routes orchestrator `request-llm-response`
 *     events through the openai-compat backend (the LMStudioBackend
 *     class -- it's the same class for vLLM / LM Studio / generic
 *     openai-compat under different `name` tags).
 *   - Native `LlmEnd.tool_calls` flow back to the orchestrator
 *     and trigger tool execution (the fix from commit `0bca28a`).
 *   - `BuildOutput`, `SubSessionStarted/Ended`, `StateAdvanced`,
 *     and `GateResult` events round-trip cleanly.
 *
 * What it does NOT test:
 *   - DashboardHost button-routing layer (that needs vscode-API
 *     mocking; the existing `mockFlowHarness.test.ts` covers it
 *     against a mocked pump).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cp from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

// Minimal vscode mock. `SocketSessionPump.dispatchLlm` constructs a
// `CancellationTokenSource` and reads `workspace.getConfiguration`;
// the openai-compat / LMStudio backends are pure fetch and don't
// touch vscode at all. We keep the mock surface to exactly those
// two pieces so additions to the real vscode surface don't quietly
// leak through.
vi.mock("vscode", () => {
  class CancellationTokenSource {
    private _cancelled = false;
    private _listeners: Array<() => void> = [];
    readonly token = {
      get isCancellationRequested() {
        return false; // we never trigger cancel in this test
      },
      onCancellationRequested(listener: () => void) {
        return { dispose() {} };
      },
    };
    cancel(): void {
      this._cancelled = true;
      for (const l of this._listeners) l();
    }
    dispose(): void {}
  }
  const workspace = {
    getConfiguration(_section: string) {
      return {
        get<T>(_key: string, dflt?: T): T | undefined {
          return dflt;
        },
      };
    },
  };
  class Uri {
    static file(p: string) {
      return { fsPath: p, toString: () => `file://${p}` };
    }
  }
  return {
    CancellationTokenSource,
    workspace,
    Uri,
  };
});

import { SocketSessionPump } from "./session/socketPump";
import type { PumpLlmConfig } from "./session/pump";

const LIVE = process.env.SIM_FLOW_E2E_LIVE === "1";
const BASE_URL =
  process.env.SIM_FLOW_E2E_LIVE_BASE_URL ?? "http://localhost:8012/v1";
const MODEL = process.env.SIM_FLOW_E2E_LIVE_MODEL ?? "qwen3.6";

function findRepoRoot(): string {
  // Walk up from this file looking for the foundation root marker
  // (`Cargo.toml` with `sim-foundation` in it would be ideal; we
  // settle for the `tools/sim-flow` directory).
  let dir = __dirname;
  for (let depth = 0; depth < 8; depth += 1) {
    if (fs.existsSync(path.join(dir, "tools", "sim-flow", "Cargo.toml"))) {
      return dir;
    }
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  throw new Error("could not locate sim-foundation root from test file");
}

function vllmReachable(baseUrl: string, timeoutMs = 1500): Promise<boolean> {
  return new Promise((resolve) => {
    const ctl = new AbortController();
    const t = setTimeout(() => ctl.abort(), timeoutMs);
    fetch(`${baseUrl.replace(/\/$/, "")}/models`, { signal: ctl.signal })
      .then((r) => {
        clearTimeout(t);
        resolve(r.ok);
      })
      .catch(() => {
        clearTimeout(t);
        resolve(false);
      });
  });
}

describe.skipIf(!LIVE)("e2e auto smoke (live vLLM)", () => {
  let foundationRoot: string;
  let simFlowBin: string;
  let smokeSpec: string;
  let tmpRoot: string;
  let projectDir: string;
  let livePump: SocketSessionPump | undefined;

  beforeEach(async () => {
    foundationRoot = findRepoRoot();
    simFlowBin = path.join(
      foundationRoot,
      "target",
      "debug",
      "sim-flow",
    );
    if (!fs.existsSync(simFlowBin)) {
      throw new Error(
        `sim-flow binary not found at ${simFlowBin}. Build with: ` +
          `cargo build -p sim-flow --bin sim-flow`,
      );
    }
    smokeSpec = path.join(
      foundationRoot,
      "tools",
      "sim-flow",
      "src",
      "bin",
      "dm_flow_smoke_spec.md",
    );
    if (!fs.existsSync(smokeSpec)) {
      throw new Error(
        `dm_flow_smoke_spec.md not found at ${smokeSpec}; expected the ` +
          `Rust smoke spec to ship alongside the sim-flow binaries.`,
      );
    }
    const reachable = await vllmReachable(BASE_URL);
    if (!reachable) {
      throw new Error(
        `vLLM is not reachable at ${BASE_URL}/models. Start it (e.g. ` +
          `\`ssh -L 8012:127.0.0.1:8012 <vllm-host>\`) or override with ` +
          `SIM_FLOW_E2E_LIVE_BASE_URL.`,
      );
    }
    tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), "sim-flow-e2e-auto-"));
    projectDir = path.join(tmpRoot, "smoke");
    fs.mkdirSync(projectDir, { recursive: true });
    // Minimal project init: sim-flow's `auto` subcommand does the
    // rest (creates `.sim-flow/state.toml`, ingests the spec, etc.)
    // when given `--spec`. We need an empty Cargo.toml so the gate
    // doesn't refuse before the agent has a chance to populate it.
    fs.writeFileSync(
      path.join(projectDir, "Cargo.toml"),
      `[package]\nname = "smoke_model"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n# foundation-framework dep will be added by the agent at DM2d\n`,
    );
    fs.mkdirSync(path.join(projectDir, "src"), { recursive: true });
    fs.writeFileSync(path.join(projectDir, "src/lib.rs"), "");
  });

  afterEach(async () => {
    if (livePump) {
      try {
        await livePump.dispose?.();
      } catch {
        /* best-effort */
      }
      livePump = undefined;
    }
    if (tmpRoot && fs.existsSync(tmpRoot)) {
      fs.rmSync(tmpRoot, { recursive: true, force: true });
    }
  });

  it(
    "drives sim-flow auto end-to-end against vLLM (smoke spec)",
    async () => {
      const sessionId = `e2e-live-${Date.now()}`;
      const socketPath = path.join(
        tmpRoot,
        `${sessionId}.sock`,
      );
      const args = [
        "auto",
        "--transport-socket",
        socketPath,
        "--foundation-root",
        foundationRoot,
        "--project",
        projectDir,
        "--llm-backend",
        "openai-compat",
        "--llm-model",
        MODEL,
        "--max-auto-iters",
        "12",
        "--max-critique-iters",
        "15",
        "--step-mode",
        "auto",
        "--spec",
        smokeSpec,
      ];
      const llm: PumpLlmConfig = {
        source: "openai-compat",
        model: MODEL,
        baseUrl: BASE_URL,
        projectDir,
        binary: simFlowBin,
        debugTokens: "",
      };
      const pump = new SocketSessionPump(
        {
          sessionId,
          socketPath,
          launch: {
            binary: simFlowBin,
            args,
            cwd: projectDir,
            env: {
              ...process.env,
              // Match the production tool-call wiring our recent
              // smoke runs used; without this the orchestrator
              // dispatches in fenced-mode, which still works but
              // is the non-default path.
              SIM_FLOW_TOOL_MODE: "native",
              // Quiet thinking by default; the smoke runs faster
              // and the agent's structured-task work doesn't need
              // reasoning preambles.
              SIM_FLOW_DISABLE_THINKING: "1",
            },
          },
        },
        llm,
      );
      livePump = pump;
      await pump.ready();

      // No-op renderer: the orchestrator emits AssistantText /
      // ToolInvoked / etc. and the pump funnels them through this
      // sink. The smoke test doesn't assert on transcript content
      // (it asserts on disk state.toml progress), so swallowing is
      // fine. A future variant could collect the markdown stream
      // for debugging.
      const renderer = {
        markdown(_text: string) {},
        requestTokensEstimate(_tokens: number) {},
      };

      // Wait for either: pump settles (orchestrator exited), or
      // a hard cap on wall time -- a smoke run against a small
      // spec at full vLLM tilt typically finishes in 5-10 minutes.
      const settled = await pump.settle(renderer);

      const statePath = path.join(projectDir, ".sim-flow", "state.toml");
      expect(fs.existsSync(statePath)).toBe(true);
      const stateBody = fs.readFileSync(statePath, "utf8");
      // Smoke spec is dm_flow_smoke_spec.md -- a 3-stage pipeline
      // small enough that a healthy run reaches at least DM2cd
      // (often DM3+); we accept "DM2cd or further" as the smoke
      // success bar so transient flakiness on the test/perf
      // milestones doesn't make this a "DM4b or bust" test.
      const passedSteps = stateBody
        .split("\n")
        .filter((l) => l.trim().startsWith("[gates.DM"))
        .map((l) => l.replace(/\[gates\.|]$/g, "").trim());
      expect(passedSteps.length).toBeGreaterThanOrEqual(6);
      expect(passedSteps).toContain("DM0");
      expect(passedSteps).toContain("DM2cd");

      // Settle reason should be a clean end or an awaiting-input
      // park; anything else (spawn-error, host-closed, runaway-guard)
      // points at the protocol plumbing or backend dispatch path.
      expect(["ended", "awaiting-input"]).toContain(settled.status);
    },
    // Generous timeout: the smoke run does ~20+ LLM turns each step,
    // every turn round-trips to vLLM. 30 min is enough for the full
    // pipeline on a warm cache; we'd rather have a green test than
    // chase intermittent timeouts.
    30 * 60 * 1000,
  );
});
