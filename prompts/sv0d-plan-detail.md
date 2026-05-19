# SV0d - SV Conversion Plan, Detail (work session)

You are executing step SV0d (Plan, DETAIL) of the SystemVerilog
Conversion Flow. Prerequisite: SV0 gate passed.

## Goal

Walk each milestone STUB written by SV0 and replace its
`<!-- detail-pending -->` placeholder with the full task list per
`docs/plan-management.md`'s conventions (10-task cap,
`- [ ]` rows naming concrete artifacts, pass criteria, trace).
The orchestrator scopes you to ONE stub per work + critique
session, walking `rtl-milestone-NN-*.md` and `uvm-milestone-NN-*.md`
files under `generated/plan/` in lexicographic order.

## Inputs

The orchestrator inlines:

- `generated/plan.md` -- the index (module table + milestone tables
  + traceability).
- The CURRENT milestone stub file you are detailing this turn.

Read on demand:

- `docs/plan-management.md` -- task / state conventions.
- `src/model/<file>.rs` -- the Foundation module referenced by the
  current stub's Trace section.
- `tests/testbench/<file>.rs` -- the Rust testbench role for UVM
  milestones.
- `lib:examples/01-three-stage-pipeline/test/` and
  `lib:examples/02-multiple-ports/test/` -- structural references
  for UVM-side roles.
- `api_semantic_search(query)` + `api_hover(symbol)` only when
  you need exact framework API signatures. Do NOT `read_file`
  paths under `fw:api/pages/*.md`; the lance API index + LSP
  queries supersede the static page mirror.

## Procedure

1. Open the current milestone stub. Read its Scope, Components /
   Files, and Trace sections -- SV0's contract for this milestone.

2. Read the Trace's predecessor sources (the Foundation module,
   the Rust testbench role, the test-plan rows).

3. Replace the `## Tasks` section's `<!-- detail-pending -->`
   comment with a real task list:

   For **rtl-milestone-NN-*.md**, each task names a concrete `.sv`
   artifact and the Foundation construct it derives from:

   ```markdown
   - [ ] `generated/rtl/<file>.sv` -- <one-sentence purpose>
     - derives from: `src/model/<file>.rs::<symbol>`
     - pass criteria: <specific, measurable>
   ```

   For **uvm-milestone-NN-*.md**, each task names a UVM `.sv`
   artifact and the role / sequence it implements:

   ```markdown
   - [ ] `generated/test/<file>.sv::<symbol>` -- <one-sentence
     purpose>
     - role: driver / monitor / scoreboard / sequence / env /
       test / top
     - derives from: `tests/testbench/<file>.rs::<symbol>` (when
       applicable) AND/OR `docs/test-plan/<file>.md::<row>`
     - pass criteria: <specific, measurable>
   ```

4. **10-task cap per milestone**. If your task list would exceed
   ~10 rows, surface a structural concern in `## Auto-decisions`
   (don't silently overflow). SV0 should have split the milestone
   if it's too big.

5. **Mandatory categorical content** (the critique enforces):
   - `rtl-milestone-01-payloads.md` MUST include every packed
     struct + typedef the model uses (each gets a row).
   - The final rtl-milestone MUST list `generated/rtl/top.sv` plus
     all internal nets it wires.
   - `uvm-milestone-01-types-and-interfaces.md` MUST list
     `uvm_types_pkg.sv` + every virtual interface the testbench
     uses.
   - The final uvm-milestone MUST list `tb_top.sv` + the per-test
     `uvm_test_<name>.sv` files.

6. Add a `## Auto-decisions` trailing section recording any
   structural choices.

7. Once the task list is in place, the `<!-- detail-pending -->`
   placeholder MUST be gone from the file.

8. Surface the canonical milestone-complete notice:

   > `<milestone-name> complete; ready for critique.`
   > `<one-line summary: count of tasks added, decisions flagged,
   > deferred items>`

   Do NOT proceed to the next milestone.

## Output

{{ output_intro }}

## Constraints

- DO NOT write any `.sv` files. Stub-detail markdown only.
- DO NOT modify other milestone stubs. Sibling stubs are
  intentionally hidden.
- DO NOT modify `generated/plan.md`. The outline step (SV0) owns
  it; if you spot a structural issue, surface it in
  `## Auto-decisions`.
- DO NOT cite internal Foundation helpers; tasks describe WHAT
  will be built, SV1/SV2 pick HOW.
- DO NOT add new milestone files; if the breakdown is wrong, flag
  it in `## Auto-decisions`.
- DO NOT remove the placeholder by leaving `## Tasks` empty --
  empty task list is a `BLOCKER:`.

When the current milestone is fully detailed, stop. Do not write
`docs/critiques/SV0d-critique.md`. Do not `/exit` on your own.
