// Tiny Unix-domain-socket client for sim-flow's interactive single-
// session driver. The driver listens at `<project>/.sim-flow/control.sock`
// when started with `--session-mode single`; this module sends JSONL
// commands (one per line) and resolves once the line is on the wire.
//
// Wire format mirrors `tools/sim-flow/src/session/control_socket.rs`:
//
//   {"command":"inject","text":"explain what you just changed"}
//   {"command":"run-gate","step":"DM2c"}
//   {"command":"advance","step":"DM2c"}
//   {"command":"reset","step":"DM2c"}
//   {"command":"shutdown"}
//
// Used by the dashboard so single-session mode's buttons send commands
// to the running sim-flow + claude pair instead of opening fresh chat
// tabs. When the socket isn't reachable (per-step mode, sim-flow not
// running, etc.), the call rejects and the host's caller falls back to
// the existing chat-pane / terminal-launch behavior.

import * as fs from "node:fs";
import * as net from "node:net";
import * as path from "node:path";

/** Discriminated union of commands the control socket understands. */
export type ControlCommand =
  | { command: "inject"; text: string }
  | { command: "run-gate"; step?: string }
  | { command: "advance"; step?: string }
  | { command: "reset"; step: string }
  | { command: "shutdown" };

/** Default control-socket path for a given project directory. */
export function defaultSocketPath(projectDir: string): string {
  return path.join(projectDir, ".sim-flow", "control.sock");
}

/**
 * True iff a socket file is present at the project's default path.
 * Doesn't prove the driver is alive (the socket file may be stale
 * after a hard kill), just that one was bound. `sendCommand` does the
 * actual liveness check by attempting a connection.
 */
export function controlSocketLikelyPresent(projectDir: string): boolean {
  try {
    return fs.statSync(defaultSocketPath(projectDir)).isSocket();
  } catch {
    return false;
  }
}

/**
 * Connect, send one JSONL command, close. Resolves on successful
 * write; rejects with a wrapped error when the socket is missing,
 * connection-refused, or the write fails.
 *
 * The driver doesn't currently send synchronous replies (commands fan
 * out as broadcast events to ALL connected listeners), so this
 * function intentionally doesn't wait for a response. Connection-back
 * events are surfaced through a longer-lived consumer; that's a
 * follow-up.
 */
export async function sendCommand(
  projectDir: string,
  command: ControlCommand,
  timeoutMs: number = 2000,
): Promise<void> {
  const socketPath = defaultSocketPath(projectDir);
  if (!controlSocketLikelyPresent(projectDir)) {
    throw new ControlSocketError(
      "missing-socket",
      `No control socket at ${socketPath}. Start sim-flow with \`--session-mode single\` first.`,
    );
  }
  return await new Promise<void>((resolve, reject) => {
    const socket = net.createConnection(socketPath);
    let settled = false;
    const finish = (err?: Error): void => {
      if (settled) {
        return;
      }
      settled = true;
      try {
        socket.end();
      } catch {
        // ignore
      }
      if (err) {
        reject(err);
      } else {
        resolve();
      }
    };
    const timer = setTimeout(() => {
      finish(
        new ControlSocketError(
          "timeout",
          `Connection to ${socketPath} timed out after ${timeoutMs} ms.`,
        ),
      );
    }, timeoutMs);

    socket.once("error", (err) => {
      clearTimeout(timer);
      // ECONNREFUSED means the socket FILE exists but no listener is
      // bound to it -- typically a leftover from a sim-flow that
      // exited without running its Drop cleanup (Ctrl-C, terminal
      // closed, panic). Treat as `missing-socket` so the caller's
      // "start the agent first" guidance fires, and best-effort
      // remove the stale file so the next sim-flow startup binds
      // cleanly (it removes-then-binds, so a stale file there isn't
      // fatal -- but cleaning up here gives the user a tidy state).
      const code = (err as NodeJS.ErrnoException).code;
      if (code === "ECONNREFUSED" || code === "ENOENT") {
        try {
          // ENOENT path is a no-op; ECONNREFUSED leaves the stale.
          fs.unlinkSync(socketPath);
        } catch {
          // ignore
        }
        finish(
          new ControlSocketError(
            "missing-socket",
            `No agent listening at ${socketPath} (stale socket cleaned up).`,
            err,
          ),
        );
        return;
      }
      finish(
        new ControlSocketError(
          "connect-failed",
          `Connection to ${socketPath} failed: ${err.message ?? String(err)}`,
          err,
        ),
      );
    });
    socket.once("connect", () => {
      const line = `${JSON.stringify(command)}\n`;
      socket.write(line, "utf8", (err) => {
        clearTimeout(timer);
        if (err) {
          finish(
            new ControlSocketError(
              "write-failed",
              `Write to ${socketPath} failed: ${err.message ?? String(err)}`,
              err,
            ),
          );
        } else {
          finish();
        }
      });
    });
  });
}

export class ControlSocketError extends Error {
  readonly kind: "missing-socket" | "connect-failed" | "write-failed" | "timeout";

  constructor(
    kind: "missing-socket" | "connect-failed" | "write-failed" | "timeout",
    message: string,
    cause?: unknown,
  ) {
    super(message, { cause });
    this.name = "ControlSocketError";
    this.kind = kind;
  }
}
