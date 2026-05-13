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
    // Bootstrap a fresh project the same way the Rust robustness
    // study does: `sim-flow new model <name> --destination <parent>`
    // creates the project skeleton (Cargo.toml, src/lib.rs, src/
    // main.rs, .sim-flow/state.toml at DM0) so `sim-flow auto` has
    // a valid starting point. Same convention as the Rust
    // `run-robustness-study.sh`.
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
    if (!fs.existsSync(path.join(projectDir, ".sim-flow", "state.toml"))) {
      throw new Error(
        `expected .sim-flow/state.toml after \`sim-flow new model\`; got: ` +
          fs.readdirSync(projectDir).join(", "),
      );
    }
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
      // Unix domain sockets on macOS cap path length at ~104 bytes
      // (`SUN_LEN`). `os.tmpdir()` on macOS resolves to a long
      // `/var/folders/.../T/sim-flow-e2e-auto-XXXXXX/` path; tacking
      // a ms-precision sessionId on top is borderline (~101 bytes)
      // and a longer mkdtemp suffix puts us over. Park the socket
      // file in `/tmp` (always short) and keep the project tree in
      // `tmpRoot` for filesystem-cleanup symmetry.
      const socketPath = `/tmp/sfa-${Date.now() % 1_000_000}.sock`;
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
        // For source=`openai-compat` the backend factory reads
        // `lmstudioBaseUrl` (then falls back to LM Studio's default
        // `localhost:1234/v1`). Passing `baseUrl` alone has no
        // effect here -- that field is reserved for `server:<name>`
        // resolution via `sim-flow.llm.servers`. Using
        // `lmstudioBaseUrl` is the lowest-friction override since
        // every openai-compat dispatch consults it.
        lmstudioBaseUrl: BASE_URL,
        projectDir,
        binary: simFlowBin,
        // Enable full debug logging in the spawned orchestrator so
        // failures preserve `.sim-flow/logs/sim-flow-chat.log` with
        // raw protocol I/O. The pump constructor uses this for its
        // own DebugLog and forwards it as SIM_FOUNDATION_DEBUG to
        // the spawned process.
        debugTokens: "raw,events,llm",
        // Override the default 30 s SSE idle timeout: vLLM serving
        // a 27B model on the first DM0 turn prefills a ~8 KTok
        // system stack (tools + framework TOC + step prompt) and
        // routinely needs >30 s before the first delta arrives.
        // 5 min is well above realistic first-token latency on
        // production hardware; subsequent turns are far faster
        // thanks to prefix-cache hits.
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

      // Auto mode is supposed to drive itself end-to-end, but
      // sub-session boundaries don't resolve `settle()` -- only
      // `session-end` (clean finish), `request-user-input` (park),
      // or transport termination do. So in practice settle() resolves
      // ONCE per outer-run: either on `session-end` (the happy path)
      // or on a park (orchestrator hit a question it can't answer
      // itself, e.g. LlmError, refused-advance, DM0 clarification).
      //
      // We treat awaiting-input as "the agent gave up; let's freeze
      // the diagnostics and let the assertion phase explain why"
      // rather than burning the LLM trying to push past whatever it
      // got stuck on.
      const startedAt = Date.now();
      const settled = await pump.settle(renderer);
      const elapsedMs = Date.now() - startedAt;
      // eslint-disable-next-line no-console
      console.log(
        `[e2e-auto] settle (${elapsedMs}ms):`,
        JSON.stringify(settled),
      );

      const statePath = path.join(projectDir, ".sim-flow", "state.toml");
      // Always preserve diagnostics so the test failure tells the
      // operator *why* the auto run stopped early. Without this the
      // `afterEach` wipes `.sim-flow/logs/` before we can read it.
      const debugCopy = path.join(
        os.tmpdir(),
        `e2e-auto-debug-${Date.now()}`,
      );
      try {
        fs.cpSync(projectDir, debugCopy, { recursive: true });
      } catch (err) {
        // eslint-disable-next-line no-console
        console.log("[e2e-auto] cpSync failed:", (err as Error).message);
      }
      // eslint-disable-next-line no-console
      console.log("[e2e-auto] project preserved at:", debugCopy);
      if (fs.existsSync(statePath)) {
        // eslint-disable-next-line no-console
        console.log(
          "[e2e-auto] state.toml:\n" + fs.readFileSync(statePath, "utf8"),
        );
      } else {
        // eslint-disable-next-line no-console
        console.log("[e2e-auto] no state.toml; project files:",
          fs.readdirSync(projectDir));
      }
      const logsDir = path.join(projectDir, ".sim-flow", "logs");
      if (fs.existsSync(logsDir)) {
        for (const f of fs.readdirSync(logsDir)) {
          const body = fs.readFileSync(path.join(logsDir, f), "utf8");
          // eslint-disable-next-line no-console
          console.log(`[e2e-auto] ${f} (last 80 lines):`);
          // eslint-disable-next-line no-console
          console.log(body.split("\n").slice(-80).join("\n"));
        }
      }
      expect(fs.existsSync(statePath)).toBe(true);
      const stateBody = fs.readFileSync(statePath, "utf8");
      // Smoke target. The full DM flow has ~12 steps end-to-end and a
      // warm-cache run on production hardware can reach DM4b in
      // ~15 min. The test target is intentionally looser: we want a
      // signal that the protocol plumbing, LLM dispatch, and
      // orchestrator auto loop all work together, not a "DM4b or
      // bust" benchmark.
      //
      // The bar is "DM0 gate passes AND at least one downstream step
      // gates clean." That confirms the work-critique-advance loop
      // closes for two consecutive steps, which is the smallest
      // proof that auto mode is genuinely driving forward. Tighten
      // when the vLLM endpoint is warmer / faster.
      const passedSteps = stateBody
        .split("\n")
        .filter((l) => l.trim().startsWith("[gates.DM"))
        .map((l) => l.replace(/\[gates\.|]$/g, "").trim());
      expect(passedSteps).toContain("DM0");
      expect(passedSteps.length).toBeGreaterThanOrEqual(2);

      // Settle reason should be a clean end or an awaiting-input
      // park; anything else (spawn-error, host-closed, runaway-guard)
      // points at the protocol plumbing or backend dispatch path.
      expect(["ended", "awaiting-input"]).toContain(settled.status);
    },
    // Generous timeout: the smoke run does ~20+ LLM turns each step,
    // every turn round-trips to vLLM. Real-world observation:
    // DM0 ~5 min, DM1 ~8 min, each downstream step similar with
    // multiple critique-retry cycles. 45 min leaves headroom for
    // the first 2-3 steps without chasing intermittent timeouts.
    45 * 60 * 1000,
  );
});
