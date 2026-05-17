/**
 * CI-friendly end-to-end smoke for `sim-flow auto`.
 *
 * Mirrors `e2eAutoLive.test.ts` but replaces the vLLM dependency
 * with an embedded HTTP server that speaks the openai-compat
 * subset of the protocol. The server returns scripted tool-call
 * SSE chunks for DM0 work / critique so the orchestrator advances
 * deterministically without any LLM compute.
 *
 * What stays real:
 *   - The orchestrator subprocess (`sim-flow auto`) and the full
 *     JSONL transport.
 *   - The extension's `SocketSessionPump` -> openai-compat backend
 *     -> fetch -> mock server path.
 *   - Native tool-call assembly (`LlmEnd.tool_calls`).
 *   - Sub-session brackets, structural-gate eval, state.toml
 *     advances.
 *
 * What's mocked:
 *   - The HTTP endpoint at `/v1/chat/completions` -- emits a
 *     write_file tool call sized for the current step.
 *
 * Always runnable -- no SIM_FLOW_E2E_LIVE gate, no external
 * dependencies. Designed for CI.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cp from "node:child_process";
import * as fs from "node:fs";
import * as http from "node:http";
import * as os from "node:os";
import * as path from "node:path";
import type { AddressInfo } from "node:net";

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
  return { CancellationTokenSource, workspace, Uri };
});

import { SocketSessionPump } from "./session/socketPump";
import type { PumpLlmConfig } from "./session/pump";

const MODEL = "mock-model";

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

/**
 * Minimal openai-compat mock server. Parses the most recent user
 * message to figure out which step + kind the orchestrator is on,
 * then emits a single `write_file` tool call SSE chunk with content
 * that satisfies the structural gate for that step. Falls back to
 * an empty `stop`-reason response on unrecognized inputs (lets the
 * orchestrator's auto pump retry / wind down naturally).
 */
function buildMockServer(requestLog: Array<{ url: string; step: string | null; kind: string | null }>): http.Server {
  return http.createServer((req, res) => {
    if (req.method === "GET" && req.url?.startsWith("/v1/models")) {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(
        JSON.stringify({
          object: "list",
          data: [{ id: MODEL, object: "model", owned_by: "mock" }],
        }),
      );
      return;
    }
    if (req.method !== "POST" || !req.url?.startsWith("/v1/chat/completions")) {
      res.writeHead(404);
      res.end();
      return;
    }
    let body = "";
    req.on("data", (chunk: Buffer) => {
      body += chunk.toString("utf8");
    });
    req.on("end", () => {
      let parsed: {
        messages: { role: string; content: string }[];
        stream?: boolean;
      };
      try {
        parsed = JSON.parse(body) as typeof parsed;
      } catch {
        res.writeHead(400);
        res.end(JSON.stringify({ error: { message: "bad JSON" } }));
        return;
      }
      // Figure out which step/kind we're being asked for. The
      // orchestrator's system messages carry "step DM0" / "kind
      // work" markers; the easiest signal is the step instruction
      // header ("# DM<N> - <Name> (work session)" / "(critique
      // session)").
      const systemText = parsed.messages
        .filter((m) => m.role === "system")
        .map((m) => m.content)
        .join("\n");
      const stepMatch = systemText.match(/# (DM\d[a-z]*) - .*\((work|critique) session\)/);
      const step = stepMatch?.[1] ?? null;
      const kind = stepMatch?.[2] ?? null;
      requestLog.push({ url: req.url ?? "", step, kind });

      const writeFileCall = pickResponseForStep(step, kind);
      const id = `chatcmpl-mock-${Date.now()}`;
      const tokensIn = parsed.messages.reduce(
        (sum, m) => sum + (m.content?.length ?? 0),
        0,
      );
      const promptTokens = Math.ceil(tokensIn / 4);
      // Streaming branch: the orchestrator's new `dispatch_streaming`
      // path sends `stream: true` to enable live token rendering and
      // mid-dispatch cancellation. Mirror that: emit SSE
      // (`data: <json>\n\n` frames terminated by `data: [DONE]`).
      // The framing exactly matches what vLLM / LM Studio / Ollama
      // emit so the orchestrator's SSE parser sees the same wire
      // shape it would in production.
      if (parsed.stream === true) {
        res.writeHead(200, {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
          Connection: "keep-alive",
        });
        const baseChunk = (delta: Record<string, unknown>, finishReason?: string) => ({
          id,
          object: "chat.completion.chunk",
          model: MODEL,
          choices: [
            {
              index: 0,
              delta,
              finish_reason: finishReason ?? null,
            },
          ],
        });
        const send = (obj: unknown): void => {
          res.write(`data: ${JSON.stringify(obj)}\n\n`);
        };
        send(baseChunk({ role: "assistant", content: "" }));
        if (writeFileCall) {
          send(
            baseChunk({
              tool_calls: [
                {
                  index: 0,
                  id: `call_mock_${Date.now()}`,
                  type: "function",
                  function: {
                    name: "write_file",
                    arguments: JSON.stringify(writeFileCall),
                  },
                },
              ],
            }),
          );
          send(baseChunk({}, "tool_calls"));
        } else {
          send(baseChunk({ content: "done." }));
          send(baseChunk({}, "stop"));
        }
        // Final usage chunk (stream_options.include_usage path) so
        // metrics.tokens_{in,out} populate the same as non-streaming.
        res.write(
          `data: ${JSON.stringify({
            id,
            object: "chat.completion.chunk",
            model: MODEL,
            choices: [],
            usage: {
              prompt_tokens: promptTokens,
              completion_tokens: writeFileCall ? 16 : 1,
              total_tokens: promptTokens + (writeFileCall ? 16 : 1),
            },
          })}\n\n`,
        );
        res.write("data: [DONE]\n\n");
        res.end();
        return;
      }
      // Non-streaming branch (kept for back-compat with any caller
      // that still sends `stream: false`). Single JSON response:
      //   { choices: [{ message: { role, content, tool_calls? },
      //                 finish_reason }], usage }
      let responseBody: Record<string, unknown>;
      if (writeFileCall) {
        responseBody = {
          id,
          object: "chat.completion",
          model: MODEL,
          choices: [
            {
              index: 0,
              message: {
                role: "assistant",
                content: "",
                tool_calls: [
                  {
                    id: `call_mock_${Date.now()}`,
                    type: "function",
                    function: {
                      name: "write_file",
                      arguments: JSON.stringify(writeFileCall),
                    },
                  },
                ],
              },
              finish_reason: "tool_calls",
            },
          ],
          usage: {
            prompt_tokens: promptTokens,
            completion_tokens: 16,
            total_tokens: promptTokens + 16,
          },
        };
      } else {
        responseBody = {
          id,
          object: "chat.completion",
          model: MODEL,
          choices: [
            {
              index: 0,
              message: { role: "assistant", content: "done." },
              finish_reason: "stop",
            },
          ],
          usage: {
            prompt_tokens: promptTokens,
            completion_tokens: 1,
            total_tokens: promptTokens + 1,
          },
        };
      }
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify(responseBody));
    });
  });
}

/**
 * Track which (step, kind) write_file calls we've already returned
 * during this run so the second request doesn't loop on the same
 * artifact. Real LLMs follow up a successful tool call with a
 * "done" message; the mock has to emulate that by short-circuiting
 * to a no-op stop response after the first artifact is on disk.
 */
const writtenArtifacts = new Set<string>();

function pickResponseForStep(
  step: string | null,
  kind: string | null,
): { path: string; content: string } | null {
  const key = `${step ?? ""}:${kind ?? ""}`;
  if (writtenArtifacts.has(key)) {
    // Already wrote the artifact for this (step, kind). Returning
    // null makes the mock emit a plain "done." stop response, which
    // ends the LLM turn and lets the orchestrator advance to the
    // next sub-session (work -> critique -> gate -> next step).
    return null;
  }
  if (step === "DM0" && kind === "work") {
    writtenArtifacts.add(key);
    return {
      path: "docs/spec.md",
      content: SPEC_MD_FIXTURE,
    };
  }
  if (step === "DM0" && kind === "critique") {
    writtenArtifacts.add(key);
    return {
      path: "docs/critiques/DM0-critique.json",
      content: CRITIQUE_JSON_FIXTURE,
    };
  }
  // Unknown step -- return null so the orchestrator's auto pump
  // re-prompts (or, after max_auto_iters, falls through to
  // RequestUserInput which our `settle()` treats as `awaiting-input`).
  return null;
}

// Minimum-viable spec.md that passes DM0's structural gate. The
// gate requires "clock frequency" matching `\d+\s*(MHz|GHz)` and
// "gates per cycle" matching `[Gg]ates\s+per\s+cycle.*\d+`. Other
// content is free-form.
const SPEC_MD_FIXTURE = `# Design Specification (mock)

## Metadata
- Design name: Mock Smoke Pipeline
- Spec status: draft
- Clock frequency: 1 GHz
- Technology node: 7 nm

## Assumptions And Constraints
Gates per cycle: 25

## External interfaces
- in_valid / in_ready / in_data[31:0]: ready/valid input handshake.
- out_valid / out_ready / out_data[31:0]: ready/valid output handshake.

## Functional behavior
A trivial 1-stage datapath: out = in + 1 each accepted cycle.

## Timing
End-to-end latency: 1 cycle.

## Reset
Active-low synchronous reset clears all registers.

## Auto-decisions
- decided to use a minimal 1-stage pipeline; rationale: smallest viable design that exercises the DM flow gates.
`;

// Critique with zero blockers so the auto driver advances cleanly.
const CRITIQUE_JSON_FIXTURE = JSON.stringify(
  {
    step: "DM0",
    summary: "Mock critique: spec.md has all required fields and no blockers.",
    findings: [
      {
        kind: "resolved",
        section: "Metadata",
        title: "Clock frequency and gates-per-cycle present",
        body: "1 GHz and 25 gates per cycle are explicitly declared.",
      },
    ],
    notes: "Mock critique generated by e2eAutoMock test fixture.",
  },
  null,
  2,
);

describe("e2e auto smoke (mock LLM)", () => {
  let foundationRoot: string;
  let simFlowBin: string;
  let tmpRoot: string;
  let projectDir: string;
  let server: http.Server;
  let serverUrl: string;
  let livePump: SocketSessionPump | undefined;
  let requestLog: Array<{ url: string; step: string | null; kind: string | null }>;

  beforeEach(async () => {
    foundationRoot = findRepoRoot();
    simFlowBin = path.join(foundationRoot, "target", "debug", "sim-flow");
    if (!fs.existsSync(simFlowBin)) {
      throw new Error(
        `sim-flow binary not found at ${simFlowBin}. Build with: ` +
          `cargo build -p sim-flow --bin sim-flow`,
      );
    }
    // Spin up the embedded mock server on an OS-assigned port.
    requestLog = [];
    writtenArtifacts.clear();
    server = buildMockServer(requestLog);
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    const addr = server.address() as AddressInfo;
    serverUrl = `http://127.0.0.1:${addr.port}/v1`;
    tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), "sim-flow-e2e-mock-"));
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
    if (server) {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
    if (tmpRoot && fs.existsSync(tmpRoot)) {
      fs.rmSync(tmpRoot, { recursive: true, force: true });
    }
  });

  it(
    "drives DM0 auto-mode work + critique against the mock server",
    async () => {
      const sessionId = `e2e-mock-${Date.now()}`;
      // Unix domain sockets on macOS cap path length at ~104 bytes.
      // Same `/tmp` trick as the live tests.
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
        "--llm-base-url",
        serverUrl,
        "--max-auto-iters",
        "6",
        "--max-critique-iters",
        "3",
        "--step-mode",
        "auto",
      ];
      const llm: PumpLlmConfig = {
        source: "openai-compat",
        model: MODEL,
        lmstudioBaseUrl: serverUrl,
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
      // Run settle() in the background so we can shut the run
      // down ourselves once DM0 passes -- otherwise the
      // orchestrator continues to DM1 where the mock returns
      // no-ops and the auto pump loops until max_auto_iters parks
      // it. Once DM0 is on disk we've proven what this smoke is
      // for; bail out cleanly.
      let settled: import("./session/pump").PumpSettleResult | null = null;
      const settlePromise = pump.settle(renderer).then((r) => {
        settled = r;
      });
      settlePromise.catch(() => undefined);

      const dm0Deadline = Date.now() + 30 * 1000;
      const statePath = path.join(projectDir, ".sim-flow", "state.toml");
      while (Date.now() < dm0Deadline) {
        if (fs.existsSync(statePath)) {
          const body = fs.readFileSync(statePath, "utf8");
          if (body.includes("[gates.DM0]")) {
            break;
          }
        }
        await new Promise((r) => setTimeout(r, 200));
      }
      pump.shutdown();
      await Promise.race([
        settlePromise,
        new Promise((r) => setTimeout(r, 10_000)),
      ]);
      // eslint-disable-next-line no-console
      console.log(
        `[e2e-mock] settle: ${JSON.stringify(settled)}; ` +
          `requests=${requestLog.length}; ` +
          `first=${JSON.stringify(requestLog[0])}; ` +
          `last=${JSON.stringify(requestLog[requestLog.length - 1])}`,
      );

      // Always preserve project state so failures can diagnose.
      const debugCopy = path.join(
        os.tmpdir(),
        `e2e-mock-debug-${Date.now()}`,
      );
      try {
        fs.cpSync(projectDir, debugCopy, { recursive: true });
        // eslint-disable-next-line no-console
        console.log(`[e2e-mock] preserved at: ${debugCopy}`);
      } catch {
        /* best-effort */
      }
      if (fs.existsSync(statePath)) {
        // eslint-disable-next-line no-console
        console.log("[e2e-mock] state.toml:\n" + fs.readFileSync(statePath, "utf8"));
      }
      expect(fs.existsSync(statePath)).toBe(true);
      const stateBody = fs.readFileSync(statePath, "utf8");
      const passedSteps = stateBody
        .split("\n")
        .filter((l) => l.trim().startsWith("[gates.DM"))
        .map((l) => l.replace(/\[gates\.|]$/g, "").trim());
      expect(passedSteps).toContain("DM0");
      // Mock dispatch produced a write_file native tool call -- the
      // primary value of this smoke is proving the openai-compat
      // streaming -> native tool_calls -> orchestrator artifact
      // write -> structural-gate -> advance loop closes without
      // an external LLM. The settle status reflects how we ended
      // (explicit shutdown, awaiting-input park, or natural end).
    },
    // The mock has zero LLM compute, but state.toml writes and
    // sub-session brackets still take time. 60 s is comfortable.
    60 * 1000,
  );
});
