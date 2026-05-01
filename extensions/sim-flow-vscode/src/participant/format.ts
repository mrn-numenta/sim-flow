// Pure markdown formatters for CLI payloads. Unit-testable without
// vscode; the participant handler just concatenates them into the
// response stream.

import type { BaselineRecord, GateResult, RunRow, StatusResult } from "../cli/types";

export function formatStatusMarkdown(state: StatusResult): string {
  const lines: string[] = [];
  lines.push(`**flow:** \`${state.flow}\``);
  lines.push(`**current step:** \`${state.current_step}\``);
  if (state.started) {
    lines.push(`**started:** ${state.started}`);
  }
  const gateEntries = Object.entries(state.gates);
  if (gateEntries.length === 0) {
    lines.push("");
    lines.push("*(no gates recorded)*");
  } else {
    lines.push("");
    lines.push("| Step | Passed | Timestamp |");
    lines.push("| --- | --- | --- |");
    for (const [id, gate] of gateEntries) {
      const mark = gate.passed ? "`[x]`" : "`[ ]`";
      const ts = gate.timestamp ?? "-";
      lines.push(`| ${id} | ${mark} | ${ts} |`);
      for (const [candName, sub] of Object.entries(gate.candidates ?? {})) {
        const sm = sub.passed ? "`[x]`" : "`[ ]`";
        const sts = sub.timestamp ?? "-";
        lines.push(`| &nbsp;&nbsp;↳ ${candName} | ${sm} | ${sts} |`);
      }
    }
  }
  return lines.join("\n");
}

export function formatGateMarkdown(result: GateResult): string {
  if (result.clean) {
    return `**Gate for \`${result.step}\`:** ✅ clean — no failures.`;
  }
  const lines: string[] = [];
  lines.push(`**Gate for \`${result.step}\`:** ❌ ${result.failures.length} failure(s).`);
  lines.push("");
  for (const f of result.failures) {
    lines.push(`- **${f.description}** — ${f.reason}`);
  }
  return lines.join("\n");
}

export function formatRunsMarkdown(rows: RunRow[]): string {
  if (rows.length === 0) {
    return "*(no runs match the filter)*";
  }
  const lines: string[] = [];
  lines.push(`**Runs** (${rows.length} row${rows.length === 1 ? "" : "s"}):`);
  lines.push("");
  lines.push("| Run | Workload | Study / Candidate | Commit | Timestamp |");
  lines.push("| --- | --- | --- | --- | --- |");
  for (const r of rows) {
    const commit = r.git_commit.length > 8 ? r.git_commit.slice(0, 8) : r.git_commit;
    const dirty = r.git_dirty ? " (dirty)" : "";
    const sc = `${r.study ?? "-"} / ${r.candidate ?? "-"}`;
    lines.push(
      `| \`${r.run_id}\` | ${r.workload ?? "-"} | ${sc} | \`${commit}${dirty}\` | ${r.timestamp} |`,
    );
  }
  return lines.join("\n");
}

export function formatBaselinesMarkdown(records: BaselineRecord[]): string {
  if (records.length === 0) {
    return "*(no baselines defined)*";
  }
  const lines: string[] = [];
  lines.push("| Name | Run | Timestamp |");
  lines.push("| --- | --- | --- |");
  for (const b of records) {
    lines.push(`| ${b.name} | \`${b.run_id}\` | ${b.timestamp} |`);
  }
  return lines.join("\n");
}
