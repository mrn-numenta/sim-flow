// Typed error surface for the CLI wrapper so the extension's UI can
// render actionable messages without parsing free-form strings.

export type SimFlowCliErrorKind =
  | "binary-not-found"
  | "spawn-failed"
  | "non-zero-exit"
  | "json-parse-failed"
  | "unexpected-stdout"
  | "not-implemented";

export interface SimFlowCliErrorDetails {
  kind: SimFlowCliErrorKind;
  exitCode?: number | null;
  stdout?: string;
  stderr?: string;
  command?: string;
  args?: string[];
  cause?: unknown;
}

export class SimFlowCliError extends Error {
  readonly kind: SimFlowCliErrorKind;
  readonly exitCode?: number | null;
  readonly stdout?: string;
  readonly stderr?: string;
  readonly command?: string;
  readonly args?: string[];

  constructor(message: string, details: SimFlowCliErrorDetails) {
    super(message, { cause: details.cause });
    this.name = "SimFlowCliError";
    this.kind = details.kind;
    this.exitCode = details.exitCode;
    this.stdout = details.stdout;
    this.stderr = details.stderr;
    this.command = details.command;
    this.args = details.args;
  }
}
