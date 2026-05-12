/**
 * End-to-end live smoke test for manual-mode dashboard driving.
 *
 * The same `SocketSessionPump` instance that powers the
 * dashboard's per-step buttons is constructed directly here and
 * driven with the same calls those buttons issue (`pump.runStep`,
 * `pump.advance`, ...) against a real `sim-flow auto --step-mode
 * manual` subprocess that dispatches its LLM requests to a real
 * vLLM.
 *
 * Mirrors `tools/sim-flow/src/bin/e2e_manual.rs` on the extension
 * side.
 *
 * Skipped by default. Opt in with:
 *
 *   SIM_FLOW_E2E_LIVE=1 npx vitest run src/e2eManualLive.test.ts
 *
 * Preconditions: same as `e2eAutoLive.test.ts` (vLLM on
 * `http://localhost:8012/v1`, sim-flow binary built).
 *
 * What it tests:
 *   - The full button-equivalent call sequence on a real pump:
 *     `runStep(s, work)` -> wait for `sub-session-ended` ->
 *     `runStep(s, critique)` -> wait -> `advance(s)` -> wait for
 *     `state-advanced`, repeated per step.
 *   - The orchestrator's manual-mode `wait_for_command` dispatch.
 *   - The recently-landed manual-mode fixes (refused-advance
 *     `RequestUserInput`, walk-gate split, structural-gate-retry).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

vi.mock("vscode", () => {
  class CancellationTokenSource {
    readonly token = {
      get isCancellationRequested() {
        return false;
      },
      onCancellationRequested(_listener: () => void) {
        return { dispose() {} };
      },
    };
    cancel(): void {}
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

function readCurrentStep(projectDir: string): string | null {
  const statePath = path.join(projectDir, ".sim-flow", "state.toml");
  if (!fs.existsSync(statePath)) return null;
  const body = fs.readFileSync(statePath, "utf8");
  const m = body.match(/^current_step\s*=\s*"([^"]+)"/m);
  return m ? m[1] : null;
}

describe.skipIf(!LIVE)("e2e manual smoke (live vLLM)", () => {
  let foundationRoot: string;
  let simFlowBin: string;
  let smokeSpec: string;
  let tmpRoot: string;
  let projectDir: string;
  let livePump: SocketSessionPump | undefined;

  beforeEach(async () => {
    foundationRoot = findRepoRoot();
    simFlowBin = path.join(foundationRoot, "target", "debug", "sim-flow");
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
    const reachable = await vllmReachable(BASE_URL);
    if (!reachable) {
      throw new Error(
        `vLLM is not reachable at ${BASE_URL}/models. Start it or override ` +
          `SIM_FLOW_E2E_LIVE_BASE_URL.`,
      );
    }
    tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), "sim-flow-e2e-manual-"));
    projectDir = path.join(tmpRoot, "smoke");
    fs.mkdirSync(projectDir, { recursive: true });
    fs.writeFileSync(
      path.join(projectDir, "Cargo.toml"),
      `[package]\nname = "smoke_model"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n`,
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
    "walks DM0 work -> critique -> advance against vLLM (button-equivalent calls)",
    async () => {
      const sessionId = `e2e-manual-${Date.now()}`;
      const socketPath = path.join(tmpRoot, `${sessionId}.sock`);
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
        "manual",
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
              SIM_FLOW_TOOL_MODE: "native",
              SIM_FLOW_DISABLE_THINKING: "1",
            },
          },
        },
        llm,
      );
      livePump = pump;
      await pump.ready();

      const renderer = {
        markdown(_text: string) {},
        requestTokensEstimate(_tokens: number) {},
      };

      // Helper: drive `pump.runStep` and wait until the pump
      // settles (either sub-session-ended naturally OR parked at
      // request-user-input). Mirrors what the dashboard does on
      // each button click.
      async function driveCommand(
        send: () => void,
      ): Promise<ReturnType<SocketSessionPump["settle"]> extends Promise<infer T> ? T : never> {
        send();
        return pump.settle(renderer);
      }

      // DM0 work -> critique -> advance.
      const stepIds = ["DM0", "DM1", "DM2a", "DM2b", "DM2c", "DM2cd"];
      let lastStep = readCurrentStep(projectDir);
      for (const step of stepIds) {
        if (lastStep !== step) {
          // Either the run hasn't reached this step yet (advance
          // is the orchestrator's job) or it's already past it.
          // Stop walking on mismatch; the assertion below records
          // how far we got.
          break;
        }
        // Work.
        await driveCommand(() => pump.runStep!(step, "work"));
        // Critique.
        await driveCommand(() => pump.runCritique!(step));
        // Advance.
        await driveCommand(() => pump.advance!(step));
        lastStep = readCurrentStep(projectDir);
      }

      const statePath = path.join(projectDir, ".sim-flow", "state.toml");
      expect(fs.existsSync(statePath)).toBe(true);
      const passedSteps = fs
        .readFileSync(statePath, "utf8")
        .split("\n")
        .filter((l) => l.trim().startsWith("[gates.DM"))
        .map((l) => l.replace(/\[gates\.|]$/g, "").trim());
      expect(passedSteps).toContain("DM0");
      expect(passedSteps.length).toBeGreaterThanOrEqual(2);
    },
    30 * 60 * 1000,
  );
});
