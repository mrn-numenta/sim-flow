/**
 * Markdown renderer for `build-output` protocol events shared by
 * `SessionPump` and `SocketSessionPump`. The orchestrator emits
 * `BuildOutput { command, exit_code, stdout_tail, stderr_tail }`
 * after every build/test/clippy phase; both stdout and stderr are
 * already truncated to a tail on the orchestrator side, so we don't
 * need to clip again here.
 *
 * On exit 0 we keep the line terse -- showing the successful tail
 * is noise. On non-zero exit, we surface BOTH tails as fenced
 * blocks so the agent (and the human reader) sees the actual
 * failure reason without having to open the .sim-flow log file.
 */

export interface BuildOutputLike {
  command: string;
  exit_code: number;
  stdout_tail: string;
  stderr_tail: string;
}

export function renderBuildOutput(event: BuildOutputLike): string {
  const lines: string[] = [];
  lines.push(`\n**\`${event.command}\`** exited with status \`${event.exit_code}\`.`);
  if (event.exit_code === 0) {
    lines.push("");
    return lines.join("\n");
  }
  const stdoutTail = event.stdout_tail.trim();
  const stderrTail = event.stderr_tail.trim();
  if (stdoutTail.length > 0) {
    lines.push("");
    lines.push("stdout (tail):");
    lines.push("```text");
    lines.push(stdoutTail);
    lines.push("```");
  }
  if (stderrTail.length > 0) {
    lines.push("");
    lines.push("stderr (tail):");
    lines.push("```text");
    lines.push(stderrTail);
    lines.push("```");
  }
  if (stdoutTail.length === 0 && stderrTail.length === 0) {
    lines.push("");
    lines.push("_(no output captured)_");
  }
  lines.push("");
  return lines.join("\n");
}
