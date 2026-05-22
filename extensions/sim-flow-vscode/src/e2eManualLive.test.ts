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
import * as cp from "node:child_process";
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
const BASE_URL = process.env.SIM_FLOW_E2E_LIVE_BASE_URL ?? "http://localhost:8012/v1";
const MODEL = process.env.SIM_FLOW_E2E_LIVE_MODEL ?? "qwen3.6";

function findRepoRoot(): string {
  let dir = __dirname;
  for (let depth = 0; depth < 8; depth += 1) {
    if (fs.existsSync(path.join(dir, "tools", "sim-flow", "Cargo.toml"))) {
      return dir;
    }
    const parent = path.dirname(dir);
    if (parent === dir) {
      break;
    }
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
  if (!fs.existsSync(statePath)) {
    return null;
  }
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
    const projectName = "smoke";
    const libraryPath = path.join(foundationRoot, "..", "sim-models");
    const newRes = cp.spawnSync(
      simFlowBin,
      [
        "--foundation-root",
        foundationRoot,
        "new",
        "model",
        projectName,
        "--destination",
        tmpRoot,
        "--library-path",
        libraryPath,
        "--skip-cargo-check",
      ],
      { encoding: "utf8" },
    );
    if (newRes.status !== 0) {
      throw new Error(
        `sim-flow new model failed (exit ${newRes.status}):\n` +
          `stdout: ${newRes.stdout}\nstderr: ${newRes.stderr}`,
      );
    }
    projectDir = path.join(tmpRoot, projectName);
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
      // Unix domain sockets on macOS are limited to ~104 bytes
      // (`SUN_LEN`). The `tmpRoot` is `mkdtempSync` under
      // `os.tmpdir()` which on macOS is a long
      // `/var/folders/.../T/sim-flow-e2e-manual-XXXXXX/` path,
      // and appending a ms-precision sessionId pushes past the
      // limit. Park the socket file in `/tmp` (always short) and
      // give it a short name; the project tree stays in `tmpRoot`
      // for filesystem-cleanup symmetry with the existing test
      // bootstrap.
      const socketPath = `/tmp/sfm-${Date.now() % 1_000_000}.sock`;
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
        // openai-compat factory uses `lmstudioBaseUrl` as the
        // override (then falls back to `localhost:1234/v1`).
        // `baseUrl` is reserved for `server:<name>` resolution; it
        // has no effect for plain openai-compat sources.
        lmstudioBaseUrl: BASE_URL,
        projectDir,
        binary: simFlowBin,
        debugTokens: "raw,events,llm",
        // Override the default 30 s SSE idle timeout for slow
        // first-token latency on large remote vLLM endpoints.
        // See e2eAutoLive.test.ts for the rationale.
        streamIdleTimeoutMs: 5 * 60 * 1000,
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
      // Launch `pump.settle(renderer)` as a long-running background
      // promise. `settle()` is the pump's "the renderer is bound,
      // process events" mode -- without it, every protocol event
      // (including `request-llm-response`) gets queued in
      // `queuedEvents` and the LLM dispatch never fires. In manual
      // mode the promise never resolves on its own (the orchestrator
      // parks silently in `wait_for_command` between commands), so
      // we just leak the promise on purpose and tear it down via
      // `pump.dispose()` in `afterEach`.
      const settlePromise = pump.settle(renderer);
      // Mark as observed so vitest doesn't warn about an unhandled
      // rejection if the pump tears down before we're done.
      settlePromise.catch(() => undefined);

      // Manual mode doesn't trigger `pump.settle()` between
      // commands -- after a sub-session ends, the orchestrator
      // parks on the next `auto_host.read()` waiting for the next
      // command, without emitting RequestUserInput or SessionEnd.
      // So we wait on `onSubSessionChanged` transitions instead
      // (the same signal the dashboard uses to re-enable per-step
      // buttons).
      //
      // Race: when the test sends a command, the host event has to
      // travel over the socket, the orchestrator has to read it,
      // dispatch, and emit `sub-session-started`, then THAT event
      // has to come back over the socket. On a warm laptop this is
      // ~50-200 ms; a poll of `pump.inSubSession` right after
      // `send()` returns FALSE in that window. We mitigate by:
      // - Subscribing to `onSubSessionChanged` BEFORE checking
      //   `pump.inSubSession`, so an already-arrived `true` event
      //   is read synchronously and a pending one fires our listener.
      // - Tracking "did we see busy at any point?" so we don't
      //   resolve on a stale "always-idle" reading after the
      //   sub-session has already ended (sub-session-started +
      //   sub-session-ended could in principle both land before our
      //   subscription, though in practice runStep / runCritique
      //   take seconds at minimum).
      function waitForCommandComplete(
        startTimeoutMs: number,
        completeTimeoutMs: number,
      ): Promise<void> {
        return new Promise((resolve, reject) => {
          let sawBusy = pump.inSubSession;
          const startTimer = sawBusy
            ? null
            : setTimeout(() => {
                disposer();
                reject(
                  new Error(
                    `command did not open a sub-session within ${startTimeoutMs}ms ` +
                      `(likely rejected by a Diagnostic; check the orchestrator log)`,
                  ),
                );
              }, startTimeoutMs);
          const completeTimer = setTimeout(() => {
            if (startTimer) {
              clearTimeout(startTimer);
            }
            disposer();
            reject(
              new Error(
                `command did not complete within ${completeTimeoutMs}ms ` +
                  `(sawBusy=${sawBusy}, currentBusy=${pump.inSubSession})`,
              ),
            );
          }, completeTimeoutMs);
          const disposer = pump.onSubSessionChanged((busy) => {
            if (busy) {
              sawBusy = true;
              if (startTimer) {
                clearTimeout(startTimer);
              }
              return;
            }
            // busy === false. Only treat as "command complete" once
            // we've actually observed a busy phase -- otherwise we
            // could resolve on a pre-existing idle state before the
            // host event even reaches the orchestrator.
            if (sawBusy) {
              clearTimeout(completeTimer);
              if (startTimer) {
                clearTimeout(startTimer);
              }
              disposer();
              resolve();
            }
          });
        });
      }

      async function driveCommand(send: () => void): Promise<void> {
        send();
        await waitForCommandComplete(30 * 1000, 15 * 60 * 1000);
      }

      // Wait for state.toml's `current_step` to differ from `prev`.
      // `Advance` doesn't open a sub-session (it's a synchronous
      // gate-eval + state.toml write on the orchestrator side), so
      // `waitForIdle` returns immediately and the next
      // `readCurrentStep` could observe stale state. Poll until
      // either the step changes or the timeout fires. A refused
      // advance leaves the step unchanged; the timeout is how we
      // detect that case.
      async function waitForStepChange(prev: string, timeoutMs: number): Promise<string | null> {
        const deadline = Date.now() + timeoutMs;
        // First tick: small wait so the orchestrator can read the
        // Advance from the socket and write state.toml. 50 ms is
        // far below the minimum end-to-end of an Advance round-trip;
        // real failures take seconds at minimum.
        await new Promise((r) => setTimeout(r, 100));
        while (Date.now() < deadline) {
          const cur = readCurrentStep(projectDir);
          if (cur !== prev) {
            return cur;
          }
          await new Promise((r) => setTimeout(r, 250));
        }
        return null;
      }

      // Preserve diagnostics on any failure path. Without this the
      // `afterEach` wipes `.sim-flow/logs/` before we can see why a
      // command was rejected / timed out.
      function snapshotDiagnostics(tag: string): void {
        const debugCopy = path.join(os.tmpdir(), `e2e-manual-debug-${tag}-${Date.now()}`);
        try {
          fs.cpSync(projectDir, debugCopy, { recursive: true });

          console.log(`[e2e-manual] preserved at: ${debugCopy}`);
        } catch (err) {
          console.log(`[e2e-manual] preserve failed: ${(err as Error).message}`);
        }
        const logsDir = path.join(projectDir, ".sim-flow", "logs");
        if (fs.existsSync(logsDir)) {
          for (const f of fs.readdirSync(logsDir)) {
            const body = fs.readFileSync(path.join(logsDir, f), "utf8");

            console.log(`[e2e-manual] ${f} (last 60 lines):`);

            console.log(body.split("\n").slice(-60).join("\n"));
          }
        }
      }

      // Smoke target: walk just DM0. Manual mode commands are
      // single-pass (no critique-retry loop), so even on a model
      // that produces a clean DM0 in one shot, DM1+ critiques
      // routinely flag blockers and a one-shot Advance is refused.
      // Verifying DM0 end-to-end is enough to prove the RunStep ->
      // RunCritique -> Advance dispatch path works; multi-step
      // walks belong in the auto live test.
      const stepIds = ["DM0"];
      let lastStep = readCurrentStep(projectDir);
      try {
        for (const step of stepIds) {
          if (lastStep !== step) {
            // Either the run hasn't reached this step yet (advance
            // is the orchestrator's job) or it's already past it.
            // Stop walking on mismatch; the assertion below records
            // how far we got.
            break;
          }
          await driveCommand(() => pump.runStep!(step, "work"));
          await driveCommand(() => pump.runCritique!(step));
          // `Advance` doesn't open a sub-session bracket -- it's a
          // synchronous gate-eval + state.toml write on the
          // orchestrator side. So we send it and wait for the
          // observable side-effect (state.toml's `current_step`
          // changing) rather than for an `onSubSessionChanged`
          // transition that will never come.
          pump.advance!(step);
          const next = await waitForStepChange(lastStep, 30 * 1000);
          if (next === null) {
            // Advance refused -- stop walking. The orchestrator
            // emitted a Diagnostic explaining why; the assertion
            // below records gates passed so far.
            break;
          }
          lastStep = next;
        }
      } catch (err) {
        snapshotDiagnostics("error");
        throw err;
      }
      // Snapshot success runs too so we can audit a passing run.
      snapshotDiagnostics("complete");

      const statePath = path.join(projectDir, ".sim-flow", "state.toml");
      expect(fs.existsSync(statePath)).toBe(true);
      const passedSteps = fs
        .readFileSync(statePath, "utf8")
        .split("\n")
        .filter((l) => l.trim().startsWith("[gates.DM"))
        .map((l) => l.replace(/\[gates\.|]$/g, "").trim());
      // Smoke target: DM0 passes via the full RunStep -> RunCritique
      // -> Advance command sequence. Manual mode doesn't retry on
      // critique blockers (unlike auto's max_critique_iters loop), so
      // any step whose critique finds blockers stops the walk. DM0
      // is the simplest step (spec only) and reliably passes in one
      // cycle; downstream steps are model-dependent. Auto mode is
      // the right place to test multi-step progress; this test
      // exists to prove the manual dispatch surface works.
      expect(passedSteps).toContain("DM0");
    },
    30 * 60 * 1000,
  );
});
