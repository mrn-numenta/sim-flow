// Pure parsing helpers for @sim-flow slash-command arguments. Pulled
// out of handlers.ts so they can be unit-tested without loading vscode.

/**
 * Strip `--project <path>` from a prompt before it reaches the
 * per-command parser. The chat participant accepts this flag on
 * every slash command so users can target a specific sim-flow
 * project in a multi-project workspace; handlers never see the
 * flag themselves.
 */
export function extractProjectHint(prompt: string): {
  hint?: string;
  stripped: string;
} {
  const parts = tokenize(prompt);
  const kept: string[] = [];
  let hint: string | undefined;
  for (let i = 0; i < parts.length; i++) {
    if (parts[i] === "--project" && i + 1 < parts.length) {
      hint = parts[i + 1];
      i++;
      continue;
    }
    kept.push(parts[i]);
  }
  return { hint, stripped: kept.map(quoteIfNeeded).join(" ") };
}

/** Strip `--spec <path>` from a prompt the same way `--project` is stripped. */
export function extractSpecPath(prompt: string): { specPath?: string; stripped: string } {
  const parts = tokenize(prompt);
  const kept: string[] = [];
  let specPath: string | undefined;
  for (let i = 0; i < parts.length; i++) {
    if (parts[i] === "--spec" && i + 1 < parts.length) {
      specPath = parts[i + 1];
      i++;
      continue;
    }
    kept.push(parts[i]);
  }
  return { specPath, stripped: kept.map(quoteIfNeeded).join(" ") };
}

function quoteIfNeeded(token: string): string {
  return /\s/.test(token) ? `"${token}"` : token;
}

export type StepKind = "work" | "critique";

export interface StepRef {
  step: string;
  kind: StepKind;
  /** Optional candidate scope for per-candidate DS steps. */
  candidate?: string;
}

/**
 * Parse the prompt for `/step <step>.<kind> [--candidate <name>]`.
 * Returns an error string when the prompt is malformed.
 */
export function parseStepRef(prompt: string): StepRef | { error: string } {
  const parts = tokenize(prompt);
  if (parts.length === 0) {
    return { error: "Usage: /step <step-id>.work or /step <step-id>.critique" };
  }
  const raw = parts[0];
  const dot = raw.lastIndexOf(".");
  if (dot <= 0 || dot === raw.length - 1) {
    return { error: `Expected "<step>.work" or "<step>.critique", got "${raw}".` };
  }
  const step = raw.slice(0, dot);
  const kind = raw.slice(dot + 1);
  if (kind !== "work" && kind !== "critique") {
    return { error: `Kind must be "work" or "critique", got "${kind}".` };
  }
  let candidate: string | undefined;
  for (let i = 1; i < parts.length - 1; i++) {
    if (parts[i] === "--candidate") {
      candidate = parts[i + 1];
    }
  }
  return { step, kind, candidate };
}

/**
 * Parse `/gate [step] [--candidate <name>]`. Missing step => undefined
 * so the CLI falls back to the current step.
 */
export function parseGateArgs(prompt: string): {
  step?: string;
  candidate?: string;
} {
  const parts = tokenize(prompt);
  let step: string | undefined;
  let candidate: string | undefined;
  for (let i = 0; i < parts.length; i++) {
    const token = parts[i];
    if (token === "--candidate" && i + 1 < parts.length) {
      candidate = parts[++i];
    } else if (!token.startsWith("--") && !step) {
      step = token;
    }
  }
  return { step, candidate };
}

/**
 * Parse `/runs [--workload <w>] [--candidate <c>] [--study <s>] [--sweep <id>] [--limit <n>]`.
 */
export function parseRunsArgs(prompt: string): {
  workload?: string;
  candidate?: string;
  study?: string;
  sweep?: string;
  limit?: number;
} {
  const parts = tokenize(prompt);
  const out: {
    workload?: string;
    candidate?: string;
    study?: string;
    sweep?: string;
    limit?: number;
  } = {};
  for (let i = 0; i < parts.length; i++) {
    const token = parts[i];
    const next = parts[i + 1];
    if (token === "--workload" && next) {
      out.workload = next;
      i++;
    } else if (token === "--candidate" && next) {
      out.candidate = next;
      i++;
    } else if (token === "--study" && next) {
      out.study = next;
      i++;
    } else if (token === "--sweep" && next) {
      out.sweep = next;
      i++;
    } else if (token === "--limit" && next) {
      const parsed = Number(next);
      if (Number.isFinite(parsed) && parsed > 0) {
        out.limit = Math.floor(parsed);
      }
      i++;
    }
  }
  return out;
}

/** Parse `/reset <step>`. Required argument. */
export function parseResetArgs(prompt: string): { step: string } | { error: string } {
  const parts = tokenize(prompt);
  if (parts.length === 0) {
    return { error: "Usage: /reset <step-id>" };
  }
  return { step: parts[0] };
}

/**
 * Split on whitespace. Handles quoted strings minimally -- arguments
 * with spaces can be wrapped in double quotes. No backslash escaping.
 */
export function tokenize(prompt: string): string[] {
  const out: string[] = [];
  const re = /"([^"]*)"|(\S+)/g;
  let match: RegExpExecArray | null;
  while ((match = re.exec(prompt)) !== null) {
    out.push(match[1] ?? match[2]);
  }
  return out;
}
