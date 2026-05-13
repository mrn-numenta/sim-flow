# SV1 Critique - RTL Emission (critique session)

You are critiquing one rtl-milestone that SV1's work session just
emitted. The orchestrator has scoped you to a single milestone
file under `generated/plan/`; the critique evaluates the `.sv`
files it produced under `generated/rtl/`.

## Inputs

- The CURRENT milestone file (the orchestrator inlines its TOC).
- The `.sv` files this milestone produced (named in the task
  list).
- `src/model/<file>.rs` -- the Foundation construct each `.sv`
  derives from.
- `docs/analysis/decomposition.md`, `pipeline-mapping.md`,
  `data-movement.md` -- behavioral contract.
- `lib:docs/modeling-guide/06-design-patterns.md` -- HW pattern
  guide.

## Evaluation criteria

Emit findings as `RESOLVED:`, `UNRESOLVED:`, or `BLOCKER:`.

1. **All tasks resolved**: every `- [ ]` from the prior pass is
   now `- [x]` (or `- [-]` with a `defer reason:` -- SV1 forbids
   deferrals on the gate side, so any `- [-]` row that escapes is
   `BLOCKER:`). Missing tasks = `BLOCKER:`.
2. **All listed files exist**: each task's named `.sv` file is on
   disk under `generated/rtl/`. Missing = `BLOCKER:`.
3. **Source-trace comment**: top of each `.sv` includes
   `// generated from src/model/<file>.rs::<symbol>`. Missing =
   `UNRESOLVED:`.
4. **Functional correctness**: the RTL preserves the Foundation
   module's externally visible behavior. Compare the SV's
   evaluate/settle/update structure against the Rust `HasLogic`
   impl:
   - Inputs registered before influencing downstream registered
     state (Foundation invariant). Missing register stage =
     `BLOCKER:`.
   - Multi-input / multi-output / lane-parallel preserved.
     Collapsed to single-input shell when the Rust iterates lanes
     = `BLOCKER:`.
   - Sideband / feedback / kill / redirect ports / signals
     present when the Rust uses them. Silently dropped =
     `BLOCKER:`.
5. **Numeric semantics**: signedness, wrap, saturate, truncation
   preserved. SV defaults vs Rust semantics conflict (e.g.
   relying on accidental tool behavior for sign-extension) =
   `BLOCKER:`.
6. **Synthesizable subset**: no `initial` blocks, `#delays`,
   `force/release`, debug-only system tasks in RTL. Violation =
   `BLOCKER:`.
7. **No inferred latches**: every combinational `always_comb`
   fully assigns its outputs (default-assignment idiom). A
   conditional with no else / no default = `UNRESOLVED:`
   (latches may be intended by the synthesis tool but the agent
   should call it out).
8. **No combinational loops**: ready/valid feedback paths break
   the loop with at least one register. Combinational loop =
   `BLOCKER:`.
9. **payloads.sv first / top.sv last**: when this milestone is
   the first RTL slot, `generated/rtl/payloads.sv` exists and
   contains every payload struct. When this is the final RTL
   milestone, `generated/rtl/top.sv` exists and wires the model
   topology.
10. **Naming / structure**: module instance names match the
    Foundation model where practical. Inter-stage nets follow
    stage / operation naming. Drift from the model's structural
    layout without justification in the milestone's
    `## Auto-decisions` = `UNRESOLVED:`.
11. **Source-side untouched**: no files under `src/`, `tests/`,
    or `docs/` have been modified by SV1. Touched =
    `BLOCKER:` (the conversion must not edit the Rust model).

## Output

Write `docs/critiques/SV1-critique.json` per the standard critique
JSON schema. The orchestrator renders the `.md` sibling.

Stop after writing the JSON. Do not `/exit`.
