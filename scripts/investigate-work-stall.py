#!/usr/bin/env python3
"""
Investigate the `work-no-artifact` stall pattern observed in Phase
0c (and earlier) of the model-robustness study.

For every captured trial under <study-root>/*/*/trial-*/protocol.jsonl
that terminated on `auto: ... exceeded max_auto_iters (N) without
producing an artifact`, find the work sub-session immediately before
the terminator and dump:

  - per-turn classification: read-only, prose-only, partial-write,
    mixed-tools, empty
  - the assistant text snippet (first 400 chars) per turn
  - the tool-call name + status per turn

Output: a markdown report at <out>. Goal is to see what the model is
actually doing in the turns it burns without committing a fenced
write.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Tuple


def load_events(path: Path) -> List[Dict[str, Any]]:
    out: List[Dict[str, Any]] = []
    for line in path.read_text(errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            out.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return out


def event_kind(record: Dict[str, Any]) -> str:
    return record.get("event", {}).get("event") or record.get("event", {}).get("kind") or ""


def event_path_field(record: Dict[str, Any], field: str) -> Optional[Any]:
    return record.get("event", {}).get(field)


def is_terminator_work_no_artifact(records: Iterable[Dict[str, Any]]) -> Optional[str]:
    """Return the step id that the work-no-artifact terminator fired on,
    or None if this capture didn't end that way."""
    for r in records:
        if r.get("dir") != "out":
            continue
        ev = r.get("event", {})
        if ev.get("event") != "diagnostic":
            continue
        if ev.get("level") != "error":
            continue
        msg = ev.get("message", "")
        if "without producing an artifact" in msg and "max_auto_iters" in msg:
            # Format: "auto: <STEP> exceeded max_auto_iters ..."
            try:
                step = msg.split()[1]
            except (IndexError, AttributeError):
                step = "?"
            return step
    return None


def find_stalling_work_subsession(records: List[Dict[str, Any]], step: str) -> Tuple[int, int]:
    """Return (start_idx, end_idx) of the work sub-session for `step`
    that ran most recently before the terminator. Heuristic: scan
    backwards from the end, find the last `sub-session-ended` for
    `step.Work` (if explicit), or the last `request-llm-response`
    span between `sub-session-started` markers."""
    starts: List[int] = []
    ends: List[int] = []
    for i, r in enumerate(records):
        if r.get("dir") != "out":
            continue
        ev = r.get("event", {})
        if ev.get("event") == "sub-session-started" and ev.get("step") == step:
            kind = ev.get("kind")
            if kind in ("Work", "work"):
                starts.append(i)
        elif ev.get("event") == "sub-session-ended" and ev.get("step") == step:
            kind = ev.get("kind")
            if kind in ("Work", "work"):
                ends.append(i)
    if starts and ends and ends[-1] >= starts[-1]:
        return starts[-1], ends[-1]
    if starts:
        return starts[-1], len(records) - 1
    return 0, len(records) - 1


def split_into_turns(events: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Group events into per-turn buckets. A turn = one RequestLlmResponse
    plus the LlmChunk/End coming back, plus any tool-invoked events that
    immediately follow (orchestrator-side tool dispatch from the assistant's
    fenced calls or write attempts)."""
    turns: List[Dict[str, Any]] = []
    current: Optional[Dict[str, Any]] = None
    for r in events:
        dir_ = r.get("dir")
        kind = event_kind(r)
        if dir_ == "out" and kind == "request-llm-response":
            if current is not None:
                turns.append(current)
            current = {
                "request_id": r.get("event", {}).get("request_id"),
                "assistant_text": "",
                "tool_invocations": [],
                "artifact_writes": [],
                "diagnostics": [],
                "had_llm_chunk": False,
                "had_llm_end": False,
                "llm_error": None,
            }
        elif dir_ == "in" and kind == "llm-chunk":
            if current is not None:
                current["assistant_text"] += r.get("event", {}).get("text", "")
                current["had_llm_chunk"] = True
        elif dir_ == "in" and kind == "llm-end":
            if current is not None:
                current["had_llm_end"] = True
        elif dir_ == "in" and kind == "llm-error":
            if current is not None:
                current["llm_error"] = r.get("event", {}).get("message", "")
        elif dir_ == "out" and kind == "tool-invoked":
            if current is not None:
                current["tool_invocations"].append(
                    {
                        "name": r.get("event", {}).get("name"),
                        "status": r.get("event", {}).get("status"),
                        "args_summary": (r.get("event", {}).get("args_summary") or "")[:120],
                    }
                )
        elif dir_ == "out" and kind == "artifact-written":
            if current is not None:
                current["artifact_writes"].append(r.get("event", {}).get("path"))
        elif dir_ == "out" and kind == "diagnostic":
            if current is not None:
                current["diagnostics"].append(
                    {
                        "level": r.get("event", {}).get("level"),
                        "msg": (r.get("event", {}).get("message") or "")[:200],
                    }
                )
    if current is not None:
        turns.append(current)
    return turns


def classify_turn(turn: Dict[str, Any]) -> str:
    """Bucket each turn into a coarse classification of what the model did."""
    if turn["llm_error"]:
        return "llm-error"
    if not turn["assistant_text"].strip() and not turn["tool_invocations"]:
        return "empty"
    if turn["artifact_writes"]:
        return "wrote-artifact"
    tool_names = {ti["name"] for ti in turn["tool_invocations"]}
    if tool_names and tool_names.issubset({"read_file", "list_dir", "search"}):
        return "read-only"
    if tool_names and any(
        ti["name"] in ("write_file", "edit_file") and ti["status"] == "error"
        for ti in turn["tool_invocations"]
    ):
        return "rejected-write"
    if turn["tool_invocations"]:
        return "mixed-tools"
    if turn["assistant_text"].strip():
        return "prose-only"
    return "empty"


def summarize_capture(path: Path) -> Optional[Dict[str, Any]]:
    records = load_events(path)
    if not records:
        return None
    meta = next(
        (r for r in records if r.get("dir") == "meta" and event_kind(r) == "run-start"),
        None,
    )
    model = meta.get("event", {}).get("model") if meta else "?"
    terminator_step = is_terminator_work_no_artifact(records)
    if not terminator_step:
        return None
    start, end = find_stalling_work_subsession(records, terminator_step)
    work_events = records[start : end + 1]
    turns = split_into_turns(work_events)
    classifications = [classify_turn(t) for t in turns]
    return {
        "capture": str(path),
        "model": model,
        "step": terminator_step,
        "n_turns": len(turns),
        "classifications": classifications,
        "turns_brief": [
            {
                "n": i + 1,
                "class": classifications[i],
                "tools": [
                    f"{ti['name']}:{ti['status']}" for ti in t["tool_invocations"]
                ],
                "artifacts": t["artifact_writes"],
                "text_head": t["assistant_text"][:220].replace("\n", "  "),
            }
            for i, t in enumerate(turns)
        ],
    }


def main(argv: List[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("study_roots", nargs="+", help="Paths to study roots (or globs).")
    ap.add_argument(
        "--out", default="-", help="Output markdown path; `-` for stdout."
    )
    args = ap.parse_args(argv)

    captures: List[Path] = []
    for root in args.study_roots:
        root_path = Path(root)
        if not root_path.exists():
            continue
        # Walk one or two levels of model-slug directories.
        for proto in root_path.glob("*/trial-*/protocol.jsonl"):
            captures.append(proto)
        for proto in root_path.glob("*/*/trial-*/protocol.jsonl"):
            captures.append(proto)
    captures = sorted(set(captures))

    findings: List[Dict[str, Any]] = []
    for cap in captures:
        s = summarize_capture(cap)
        if s is not None:
            findings.append(s)

    out_lines: List[str] = []
    out_lines.append("# work-no-artifact investigation")
    out_lines.append("")
    out_lines.append(
        f"Scanned {len(captures)} capture(s); {len(findings)} terminated on "
        "`work-no-artifact`. For each, the stalling work sub-session's turn "
        "classifications and a per-turn snippet are listed below."
    )
    out_lines.append("")
    out_lines.append("## Aggregate classification (last-work-sub-session per affected trial)")
    out_lines.append("")
    bucket_totals: Dict[str, int] = {}
    for f in findings:
        for c in f["classifications"]:
            bucket_totals[c] = bucket_totals.get(c, 0) + 1
    out_lines.append("| classification | turn count |")
    out_lines.append("|---|---|")
    for k, v in sorted(bucket_totals.items(), key=lambda kv: -kv[1]):
        out_lines.append(f"| {k} | {v} |")
    out_lines.append("")
    out_lines.append("## Per-trial detail")
    out_lines.append("")
    for f in findings:
        out_lines.append(
            f"### {Path(f['capture']).parent.parent.parent.name}/"
            f"{Path(f['capture']).parent.parent.name}/"
            f"{Path(f['capture']).parent.name}"
        )
        out_lines.append("")
        out_lines.append(f"- model: `{f['model']}`")
        out_lines.append(f"- terminator step: `{f['step']}`")
        out_lines.append(
            f"- turns in stalling work sub-session: {f['n_turns']} "
            f"(classifications: {f['classifications']})"
        )
        out_lines.append("")
        out_lines.append("| turn | class | tools | artifacts | text head |")
        out_lines.append("|---|---|---|---|---|")
        for t in f["turns_brief"]:
            tools = ", ".join(t["tools"]) or "-"
            arts = ", ".join(t["artifacts"]) or "-"
            text = (
                t["text_head"]
                .replace("|", "\\|")
                .replace("`", "\\`")
            )
            if len(text) > 180:
                text = text[:180] + "..."
            out_lines.append(
                f"| {t['n']} | {t['class']} | {tools} | {arts} | {text} |"
            )
        out_lines.append("")

    body = "\n".join(out_lines)
    if args.out == "-":
        sys.stdout.write(body)
    else:
        Path(args.out).write_text(body)
        print(f"wrote {args.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
