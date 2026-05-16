# sim-flow orchestrator bug audit — 2026-05-16

Findings from a read-only audit of the sim-flow Rust orchestrator
(~51k LOC under `tools/sim-flow/src/`). Scope: bugs and correctness
risks only — style / refactor suggestions are out.

Findings are ranked by severity. The top three load-bearing risks:

1. **#1** — non-fsynced `state.toml` writes can lose flow state on
   crash / power loss.
2. **#2** — zero file locking means concurrent `sim-flow` invocations
   silently corrupt project state.
3. **#3** — `runners::spawn` deadlocks every chatty cargo build that
   emits more than the OS pipe buffer (~64 KiB).

All three are reachable from the main `sim-flow auto` path.

---

## Definite bugs

### #1 — `state.toml` write is not crash-safe (no fsync)

**File:** [`src/__internal/state.rs:190-206`](../../src/__internal/state.rs#L190-L206)
**Confidence:** definitely a bug

`write_atomic` does `fs::write(tmp, bytes)` then `fs::rename(tmp, path)`.
There is **no `sync_all()` on the tempfile** before rename and **no
`fsync` on the parent directory** after rename. A kernel/host crash or
hard power loss between rename and disk flush can leave `state.toml`
truncated, empty, or missing on next boot — and the orchestrator then
exits on next start because the gate map / `current_step` is lost.
Compare to [`keys.rs:370`](../../src/__internal/keys.rs#L370)
(`file.sync_all()`) which gets this right. Affects every `state.save`
call site (mark_passed, advance, flip_to_sv_convert, reset).

---

### #2 — No file locking around `.sim-flow/` — concurrent writers clobber state

**Files:** [`state.rs::save`](../../src/__internal/state.rs#L190),
[`auto.rs::try_advance`](../../src/__internal/session/auto.rs#L1650),
[`runner.rs`](../../src/__internal/runner.rs)
**Confidence:** definitely a bug

`grep -rn "flock|fs2|advisory_lock|file_guard"` returns no hits across
the crate. Two terminals running `sim-flow auto` (or `sim-flow auto`
+ a dashboard issuing `Advance`) will both `State::load`, mutate in
memory, and `state.save` — last writer wins, the other process's
gate-passes vanish. `try_advance` reads state, runs `gate::evaluate`,
runs git commit, then `state.save` — a wide TOCTOU window where any
other writer's changes are lost.

---

### #3 — `runners::spawn` deadlocks on chatty cargo output (>~64 KiB)

**File:** [`src/__internal/session/runners.rs:592-658`](../../src/__internal/session/runners.rs#L592-L658)
**Confidence:** definitely a bug

stdout/stderr are piped (`Stdio::piped()`) but `read_to_end` is called
only AFTER `child.try_wait()` returns `Ok(Some(_))` (lines 641-647).
Cargo on a real project emits well over the OS pipe buffer (~64 KiB)
of stderr during a failing build. The child blocks on
`write(stderr)`, never exits, `try_wait()` stays `Ok(None)` forever
— the 300 s timeout fires, the child is killed, the "runner timed
out" branch returns (lines 620-628) with empty stdout/stderr. Every
`run_cargo` call against a build that prints >64 KiB is silently a
5-minute hang followed by a timeout error with no diagnostic. Fix:
per-stream reader thread, or `Command::output()` after the
timeout-based kill.

---

### #4 — `Critique::parse` matches `BLOCKER:` inside fenced code blocks

**File:** [`src/__internal/critique.rs:188-215`](../../src/__internal/critique.rs#L188-L215) (legacy markdown scanner at lines 455-485)
**Confidence:** definitely a bug

The markdown fallback iterates `text.lines()` and applies
`FINDING_MARKER_RE` to every line. There is no `in_fence` tracking
(no `grep -n fence` hits in the file). A code sample inside a
critique like:

```
\`\`\`text
- BLOCKER: example placeholder
\`\`\`
```

is parsed as a real finding and blocks the gate. Same bug exists in
`parse_with_lines` (the dashboard variant).

---

### #5 — `experiments.db` opens with no WAL, no `busy_timeout`, no transaction around `next_sequence`

**Files:** [`src/__internal/tracking/index.rs:78-102, 185-197`](../../src/__internal/tracking/index.rs#L78-L102)
**Confidence:** definitely a bug

`Connection::open(path)` with no pragmas. Compare
[`global_db.rs:167`](../../src/__internal/global_db.rs#L167) which
DOES enable WAL.

- (a) Any concurrent writer hits `SQLITE_BUSY` immediately (no
  retry backoff).
- (b) `next_sequence` (`SELECT run_id … ORDER BY id DESC LIMIT 1` +1)
  is racy — two simultaneous `record_run`s read the same max, build
  the same `run_id`, the second `INSERT` fails the UNIQUE constraint
  and the run is dropped on the floor. The artifact directory is
  created at
  [`record_run.rs:58-62`](../../src/__internal/tracking/run_recording.rs#L58-L62)
  BEFORE the insert, so the FS keeps an orphan `.experiments/NNN-…/`
  dir. (See also finding #17.)

---

### #6 — Tool `write_file` / `edit_file` / `delete_file` follow symlinks

**Files:** [`tools/write_file.rs:148-160`](../../src/__internal/session/tools/write_file.rs#L148-L160),
[`tools/delete_file.rs:90-119`](../../src/__internal/session/tools/delete_file.rs#L90-L119),
[`session/tools/mod.rs:399-428`](../../src/__internal/session/tools/mod.rs#L399-L428)
**Confidence:** definitely a bug (security)

`is_safe_relative_path` rejects `..`, absolute paths, and weird
chars in the **string**, but `resolve_safe_path` then just does
`project_dir.join(rel)` and `fs::write` — which follows existing
symlinks. A pre-existing symlink anywhere under the project dir
(e.g. `node_modules/foo -> /etc/hosts`, or a malicious file the LLM
was asked to read first and then re-creates) lets `write_file`
overwrite arbitrary files the user has write access to. No call to
`symlink_metadata` or `O_NOFOLLOW` anywhere in the write path. Only
`delete_file` uses `symlink_metadata` (line 98), and even there it's
only to detect directories.

---

### #7 — `openai_compat::tail` panics on non-ASCII UTF-8 boundary

**File:** [`src/__internal/session/agent/openai_compat/transport.rs:604-610`](../../src/__internal/session/agent/openai_compat/transport.rs#L604-L610)
**Confidence:** definitely a bug

```rust
fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[s.len() - max..] }
}
```

Slices by byte index. Called at lines 495 and 519 inside
error-formatting paths for server-returned bodies (LLM-controlled
UTF-8). If the truncation point lands in the middle of a multi-byte
char, this panics. A non-ASCII error body from vLLM / LM Studio / a
stray emoji crashes the orchestrator from the error path itself.
The sister function in
[`runners.rs:669-676`](../../src/__internal/session/runners.rs#L669-L676)
uses `s.get(start..)` and is safe.

---

### #8 — `tick_resolved_milestone_tasks` non-atomic write + silent CRLF stripping

**File:** [`src/__internal/steps/mod.rs:660-717`](../../src/__internal/steps/mod.rs#L660-L717)
**Confidence:** definitely a bug (atomicity); likely a bug (CRLF stripping)

Line 713 does a bare `std::fs::write(&path, new_body)` — no
tempfile/rename — so a crash mid-write leaves a milestone file
truncated and the run unable to resume cleanly. Separately,
`body.lines().collect::<Vec<_>>().join("\n")` (lines 689-707) strips
the `\r` from any `\r\n` line endings — milestone files written on
Windows or by an editor that uses CRLF get silently re-written LF-only.

---

### #14 — `try_advance` write-after-load TOCTOU (instance of #2)

**File:** [`src/__internal/session/auto.rs:1643-1718`](../../src/__internal/session/auto.rs#L1643-L1718)
**Confidence:** definitely a bug (given #2)

`State::load` (line 1650), `gate::evaluate` (1655),
`commit_step_advance` (which shells out to git and can take many
seconds — line 1706), then `state.mark_passed` + `state.save`
(1714-1718). Anything else that wrote `state.toml` during the git
commit window — a dashboard `Advance` from another panel, a parallel
`sim-flow status --json`, a user manually editing — gets silently
overwritten by the in-memory `state` loaded before the git call. The
lack of locking (finding #2) is the root cause; this is the worst
exposed instance because the git step gives it real latency.

---

## Likely bugs

### #9 — `flip_to_sv_convert` / `flip_to_dmf` overwrite prior archive on re-flip

**File:** [`src/__internal/state.rs:163-187`](../../src/__internal/state.rs#L163-L187)
**Confidence:** likely a bug

```rust
self.archived_gates.insert("dm".to_string(), prior);
```

`insert` REPLACES — there is no check that `"dm"` (or `"ds"`) is
already present. A user who runs `convert-sv --force` from a
DSF-archived state, or who flips DSF → DMF and somehow back, loses
the first archive silently. The current `convert-sv` precondition
gates this in normal flow, but `--force` bypasses it
([`commands.rs:1749, 1757`](../../src/commands.rs#L1749)). The
audit-trail promise in the doc comment is silently violated.

---

### #10 — Mutex `lock().unwrap()` in parallel walk panics on poisoning

**File:** [`src/__internal/session/auto.rs:1342`](../../src/__internal/session/auto.rs#L1342)
**Confidence:** likely a bug

In the scoped parallel-milestone walker, every worker does
`queue_w.lock().unwrap().pop()`. If any single worker panics while
holding the Mutex (the tool stack has plenty of `unwrap`s in
`write_file` / LSP / shell calls), the mutex is poisoned and every
other worker's `lock().unwrap()` panics on the next iteration. The
work-result `?` upstream propagates the first `Err` but the partial
state on disk (some milestones written, others not, manifest
half-recorded) is recoverable only by `sim-flow reset`.

---

### #11 — Unix socket reads use `read_line` with unbounded line length

**Files:** [`src/__internal/session/socket_host.rs:238-290`](../../src/__internal/session/socket_host.rs#L238-L290),
[`session/control_socket.rs:241-265`](../../src/__internal/session/control_socket.rs#L241-L265)
**Confidence:** likely a bug

The wire framing is newline-delimited JSON, parsed via
`BufReader::read_line` with no max-length cap. A misbehaving (or
malicious) host that connects and sends an unterminated line streams
data into the buffer forever — `read_line` keeps growing the `String`
until OOM. The orchestrator's UDS lives at `.sim-flow/control.sock`;
any local process on the host can connect. Same shape in
`control_socket::handle_connection` (line 244 `reader.lines()` — the
underlying `read_line` is identically unbounded).

---

### #13 — Critique write succeeds, markdown render fails — split state

**File:** [`src/__internal/session/tools/write_file.rs:160-187`](../../src/__internal/session/tools/write_file.rs#L160-L187)
**Confidence:** likely a bug

`fs::write` of the critique JSON succeeds (lines 160-161), then
`render_critique_markdown_to_disk` is called (line 167). If the
render fails, the tool returns `ToolResult::err(...)` — but the JSON
has already been committed to disk and the next gate evaluation will
load it. The agent sees an error result and may re-emit the critique
with different content, then write it again. The first failed render
is not retried, so the displayed dashboard view diverges from the
gate's source of truth. (Also: JSON-then-render is two non-atomic
writes; a crash between them leaves a JSON with no `.md` sibling.)

---

### #15 — `sim-flow reset` has no confirmation, no `--force` flag — deletes immediately

**File:** [`src/commands.rs:1886-1924`](../../src/commands.rs#L1886-L1924)
**Confidence:** likely a bug (design oversight)

Calling `sim-flow reset DM2a` on a project at DM4b silently deletes
every artifact and critique for DM2a..DM4b (via
`clear_step_collateral_forward` on line 1903) BEFORE any chance to
abort. The CLI prints what was deleted afterward. The dashboard's
`Reset` button hits the same path. Other destructive commands
(`convert-sv`) gate behind `--force`; `reset` does not.

---

### #17 — `next_sequence` orphans run directory on UNIQUE-constraint collision

**File:** [`src/__internal/tracking/run_recording.rs:44-80`](../../src/__internal/tracking/run_recording.rs#L44-L80)
**Confidence:** likely a bug

`record_run` creates `.experiments/<run-id>/` (lines 58-62) and writes
config/metrics snapshots (64-65) BEFORE the SQL `INSERT`. If the
insert fails (most likely on `UNIQUE(run_id)` from the race in #5,
or any other SQL error), the per-run directory and its snapshot files
are left behind with no row pointing at them. No cleanup is
attempted. Over time this accumulates ghost run dirs that
`runs list` doesn't show.

---

### #18 — Divergent semantics: gate refuses malformed-JSON critiques, dashboard silently falls back

**Files:** [`src/__internal/critique.rs:222-237` vs `510-555`](../../src/__internal/critique.rs#L222-L237)
**Confidence:** likely a bug

The gate path (`Critique::load` → `from_json` → `Err`) refuses to
advance on malformed JSON — correct. The dashboard path
(`read_critique_entry`, lines 519-536; comment line 537: "Malformed
JSON falls through to the markdown parse below") silently parses the
markdown body and computes `has_blocking` from that. Result: the
dashboard shows "Findings: 0, gate clean" while the gate refuses to
advance and the user can't tell where the disagreement comes from.

---

## Suspicious — needs verification

### #12 — Milestone files sorted lex, not by numeric prefix

**File:** [`src/__internal/steps/mod.rs:430-461`](../../src/__internal/steps/mod.rs#L430-L461)
**Confidence:** suspicious

`list_milestone_files` sorts by filename string. The numeric prefix
is NOT parsed. A milestone naming of `milestone-9-foo.md`,
`milestone-10-bar.md` orders 10 before 9. `find_current_milestone`
then picks `milestone-10` as "first pending" before `milestone-9` —
milestones run out of order. No test exercises >9 milestones;
convention assumes zero-padding but nothing enforces it.

---

### #16 — `KeyResolution` derives `Debug` and exposes raw `key: String`

**File:** [`src/__internal/keys.rs:228-232`](../../src/__internal/keys.rs#L228-L232)
**Confidence:** suspicious (no current leaker; trap is loaded)

```rust
#[derive(Debug, Clone)]
pub struct KeyResolution { pub key: String, pub source: KeySource }
```

The struct is returned from `resolve_api_key` and held by callers
including [`AnthropicAgent::new`](../../src/__internal/session/agent/anthropic/mod.rs#L92-L95).
Any future `tracing::debug!("{:?}", resolution)`, `panic!("{:?}", …)`,
or accidental `dbg!()` dumps the raw API key. Defense in depth: a
custom `Debug` that prints `"<redacted>"` and a wrapper that doesn't
expose `key` as `pub String`. Nothing currently logs it.

---

### #19 — `GateCheck::Shell` does not allowlist `cmd`

**File:** [`src/__internal/gate.rs:239-298`](../../src/__internal/gate.rs#L239-L298)
**Confidence:** suspicious

`GateCheck::Shell { cmd, args }` uses `Command::new(cmd).args(args)`
— no allowlist on `cmd`. The gate-check list is fixed in the step
registry (`steps/dm.rs`, `ds.rs`, `sv.rs`) so this is normally
tight, but the same `evaluate` is used in code paths that load gate
checks from config / step descriptors that LLM-authored milestone
files may influence. Worth confirming the gate-check list is truly
never read from a project-controlled file. Command injection via
`sh -c` is correctly avoided; the concern is `cmd` itself being
unvalidated.

---

### #20 — `expand_candidate_files` fragile filename exclusion list

**File:** [`src/__internal/gate.rs:484-524`](../../src/__internal/gate.rs#L484-L524)
**Confidence:** suspicious (low-impact correctness drift)

The "skip filenames" list at line 513 matches exact `.gitkeep`,
`README.md`, `_toc.md`, `index.md`. A section file literally named
`Readme.md` (different case) is included, while `README.md` is not.
Spec-section naming drift across projects would cause divergence.
Net: `AnyExists` still requires non-empty content
(`meta.len() > 0`), so a stubbed empty file doesn't short-circuit;
the directory-fallback path is mostly OK but the exclusion logic
is fragile.

---

## Suggested fix order

1. **#1** — switch `write_atomic` to `tempfile + write + sync_all +
   rename + parent fsync`. One-day fix; closes the worst data-loss
   risk.
2. **#2 + #14** — add advisory file lock around `.sim-flow/` for any
   process about to mutate state. Eliminates the TOCTOU and the
   silent-overwrite class.
3. **#3** — per-stream reader threads in `runners::spawn`, or use
   `Command::output()` once timing is settled. Restores cargo
   diagnostics; ends the silent 5-min hangs.
4. **#7** — `s.get(s.len().saturating_sub(max)..)` instead of byte
   slicing in `openai_compat::tail`. One-line fix; eliminates
   crash-on-error-body.
5. **#4** — track fenced-code state in `Critique::parse` so
   `BLOCKER:` inside fences doesn't fire.
6. **#5 + #17** — WAL + `busy_timeout` on `experiments.db`, wrap
   `next_sequence` + `insert` in a transaction, defer directory
   creation until after the insert succeeds (or roll back the dir on
   insert failure).
7. **#6** — `O_NOFOLLOW` (or `symlink_metadata` precheck) in the tool
   write/edit paths.
8. **#8** — `write_atomic` in `tick_resolved_milestone_tasks`; preserve
   the file's existing line ending.
9. **#15** — gate `sim-flow reset` behind a confirmation prompt or
   `--force`, mirroring `convert-sv`.
10. **#9, #10, #11, #13, #18** — pick up as load-bearing features
    surface real symptoms.
