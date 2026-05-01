// Append-only markdown debug log for the extension's view of a
// session. Mirrors `tools/sim-flow/src/session/debug_log.rs`: same
// env var (`SIM_FOUNDATION_DEBUG`), same comma-separated tokens
// (`events`, `raw`, `llm`; shortcuts `1`/`true` -> events+llm, `all`
// -> all three). Disabled when no token is selected; the methods
// short-circuit so calls in hot paths cost a single boolean check.

import * as fs from "node:fs";
import * as path from "node:path";

import type { Event as ProtocolEvent, HostEvent, LlmMessage } from "./protocol-types";

export interface CategorySet {
  events: boolean;
  raw: boolean;
  llm: boolean;
}

export function parseCategories(raw: string | undefined): CategorySet {
  const out: CategorySet = { events: false, raw: false, llm: false };
  if (!raw) {
    return out;
  }
  for (const token of raw
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0)) {
    switch (token) {
      case "events":
        out.events = true;
        break;
      case "raw":
        out.raw = true;
        break;
      case "llm":
        out.llm = true;
        break;
      case "1":
      case "true":
        out.events = true;
        out.llm = true;
        break;
      case "all":
        out.events = true;
        out.raw = true;
        out.llm = true;
        break;
      default:
        // Match the Rust side's tolerance: warn and ignore.
        console.warn(`sim-flow: ignoring unknown SIM_FOUNDATION_DEBUG token \`${token}\``);
    }
  }
  return out;
}

export function categoriesAny(c: CategorySet): boolean {
  return c.events || c.raw || c.llm;
}

export class DebugLog {
  private readonly start = Date.now();
  private fd: number | null = null;

  constructor(
    private readonly cats: CategorySet,
    projectDir: string,
  ) {
    if (!categoriesAny(cats)) {
      return;
    }
    const dir = path.join(projectDir, ".sim-flow", "logs");
    try {
      fs.mkdirSync(dir, { recursive: true });
      this.fd = fs.openSync(path.join(dir, "extension-chat.log"), "a");
      const banner = `\n## Session started at ${new Date().toISOString()}\n\n`;
      fs.writeSync(this.fd, banner);
    } catch (err) {
      console.warn(
        `sim-flow: cannot open extension debug log: ${(err as Error).message ?? String(err)}`,
      );
      this.fd = null;
    }
  }

  /**
   * Open from a comma-joined token string (e.g. `"events,llm"`). Empty
   * string -> no-op log. Used by the SessionPump after resolving the
   * `sim-flow.debug` setting (or its SIM_FOUNDATION_DEBUG fallback).
   */
  static fromTokens(tokens: string, projectDir: string): DebugLog {
    return new DebugLog(parseCategories(tokens), projectDir);
  }

  dispose(): void {
    if (this.fd !== null) {
      try {
        fs.closeSync(this.fd);
      } catch {
        // ignore
      }
      this.fd = null;
    }
  }

  logEventIn(event: ProtocolEvent): void {
    if (!this.cats.events || this.fd === null) {return;}
    this.writeSection("←", event.event, event);
  }

  logEventOut(event: HostEvent): void {
    if (!this.cats.events || this.fd === null) {return;}
    this.writeSection("→", event.event, event);
  }

  logRawIn(line: string): void {
    if (!this.cats.raw || this.fd === null) {return;}
    this.writeRaw("←", line);
  }

  logRawOut(line: string): void {
    if (!this.cats.raw || this.fd === null) {return;}
    this.writeRaw("→", line);
  }

  logLlmDispatch(messages: readonly LlmMessage[]): void {
    if (!this.cats.llm || this.fd === null) {return;}
    let body = `### ${this.elapsed()} llm→ dispatch (${messages.length} message(s))\n`;
    for (let i = 0; i < messages.length; i++) {
      const m = messages[i]!;
      body += `\n#### [${i}] ${m.role}\n\`\`\`\n${m.content}\n\`\`\`\n`;
    }
    body += "\n";
    fs.writeSync(this.fd, body);
  }

  logLlmChunk(text: string): void {
    if (!this.cats.llm || this.fd === null) {return;}
    fs.writeSync(this.fd, `### ${this.elapsed()} llm← chunk (${text.length} chars)\n${text}\n\n`);
  }

  logLlmEnd(totalChars: number, chunkCount: number): void {
    if (!this.cats.llm || this.fd === null) {return;}
    fs.writeSync(
      this.fd,
      `### ${this.elapsed()} llm← end (${chunkCount} chunk(s), ${totalChars} total chars)\n\n`,
    );
  }

  logLlmError(err: unknown): void {
    if (!this.cats.llm || this.fd === null) {return;}
    const msg = (err as Error)?.message ?? String(err);
    fs.writeSync(this.fd, `### ${this.elapsed()} llm← error\n\`\`\`\n${msg}\n\`\`\`\n\n`);
  }

  /**
   * Process lifecycle markers. Always written when the log is open
   * regardless of category — without these, a sim-flow subprocess that
   * exits silently leaves no breadcrumb in the log and the user has
   * no way to tell whether it crashed, was killed, or returned cleanly.
   */
  logProcessSpawn(binary: string, args: readonly string[], pid: number | undefined): void {
    if (this.fd === null) {return;}
    const argv = [binary, ...args].map((a) => JSON.stringify(a)).join(" ");
    fs.writeSync(
      this.fd,
      `### ${this.elapsed()} process spawned (pid=${pid ?? "?"})\n\`\`\`\n${argv}\n\`\`\`\n\n`,
    );
  }

  logSpawnError(message: string): void {
    if (this.fd === null) {return;}
    fs.writeSync(this.fd, `### ${this.elapsed()} process spawn error\n\`\`\`\n${message}\n\`\`\`\n\n`);
  }

  logProcessExit(code: number | null, signal: NodeJS.Signals | null, stderrTail: string): void {
    if (this.fd === null) {return;}
    const tail = stderrTail.trim();
    const lines = [
      `### ${this.elapsed()} process exited`,
      "```",
      `code: ${code ?? "(null)"}`,
      `signal: ${signal ?? "(none)"}`,
      tail.length > 0 ? `stderr tail (last ${tail.length} chars):\n${tail}` : "stderr: (empty)",
      "```",
      "",
    ];
    fs.writeSync(this.fd, `${lines.join("\n")}\n`);
  }

  private elapsed(): string {
    const ms = Date.now() - this.start;
    const s = Math.floor(ms / 1000);
    const r = ms % 1000;
    return `[+${s.toString().padStart(3, " ")}.${r.toString().padStart(3, "0")}s]`;
  }

  private writeSection(direction: "→" | "←", kind: string, payload: unknown): void {
    if (this.fd === null) {return;}
    const json = JSON.stringify(payload, null, 2);
    fs.writeSync(this.fd, `### ${this.elapsed()} ${direction} ${kind}\n\`\`\`json\n${json}\n\`\`\`\n\n`);
  }

  private writeRaw(direction: "→" | "←", line: string): void {
    if (this.fd === null) {return;}
    fs.writeSync(this.fd, `${this.elapsed()} raw${direction} \`${line.trimEnd()}\`\n`);
  }
}
