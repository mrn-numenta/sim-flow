# SV2 Critique - UVM Testbench Emission (critique session)

You are critiquing one uvm-milestone that SV2's work session just
emitted under `generated/test/`. The orchestrator has scoped you
to one milestone file under `generated/plan/`.

## Inputs

- The CURRENT milestone file.
- The `.sv` files this milestone produced under `generated/test/`.
- `tests/testbench/<file>.rs` -- Rust testbench role each UVM
  component derives from.
- `tests/<category>/` -- Rust tests each UVM test derives from.
- `docs/test-plan/test-plan.md` -- planned-test contract.

## Evaluation criteria

Emit findings as `RESOLVED:`, `UNRESOLVED:`, or `BLOCKER:`.

1. **All tasks resolved**: every `- [ ]` is now `- [x]`. Pending
   tasks = `BLOCKER:`. SV2 forbids deferrals on the gate.
2. **All listed files exist**: each task's named `.sv` is on disk
   under `generated/test/`. Missing = `BLOCKER:`.
3. **Source-trace comment**: each generated `.sv` carries a
   `// generated from tests/testbench/<...>` or
   `// generated from docs/test-plan/<...>` comment. Missing =
   `UNRESOLVED:`.
4. **Verification intent preserved**: the UVM env checks what the
   Rust testbench checks. Specifically:
   - Sequencer + Driver pair drives transactions to the DUT.
     Missing connect = `BLOCKER:`.
   - Monitor samples the virtual interface and fires an analysis
     port. Missing = `BLOCKER:` for non-trivial designs.
   - Scoreboard subscribes to the monitor and predicts expected
     vs observed (when the Rust side scoreboards); or explicit
     `assert` checks are present (when the Rust uses direct
     assertions). Mismatch = `BLOCKER:`.
5. **Planned-test mapping**: every row in
   `docs/test-plan/test-plan.md` traces to a UVM test/sequence,
   or is grouped under a parameterized family documented in the
   milestone's `## Auto-decisions`. Untraced row = `BLOCKER:`.
6. **Random reproducibility**: random tests pin a seed in their
   class name (`<name>_seed_<N>`); rerunnable via
   `+UVM_SEED=<N>`. Missing = `BLOCKER:` for random milestones.
7. **`uvm_types_pkg.sv` first / `tb_top.sv` last**:
   - When this milestone is the types/interfaces slot,
     `uvm_types_pkg.sv` + every required `<name>_if.sv` exist.
   - When this milestone is the per-test slot, `tb_top.sv` exists
     and instantiates the DUT, virtual interface(s), clock, and
     reset, plus calls `run_test("<default_test>")`.
8. **Synthesizable boundary**: the testbench may use behavioral
   constructs (`initial`, `#delays`, `assert`); the DUT must not.
   SV2 must NOT have modified anything under `generated/rtl/`.
   Touched = `BLOCKER:`.
9. **Source-side untouched**: no files under `src/`, `tests/`,
   `docs/`, or `generated/rtl/` have been modified. Touched =
   `BLOCKER:`.
10. **Naming**: per-test files follow `uvm_test_<name>.sv`;
    sequences follow `uvm_<name>_seq.sv`; driver / monitor / sb
    follow `uvm_<name>_drv.sv` etc. Mismatch =
    `UNRESOLVED:` (the make / verilator flow assumes the
    convention).

## Output

Write `docs/critiques/SV2-critique.json` per the standard critique
JSON schema. The orchestrator renders the `.md` sibling.

Stop after writing the JSON. Do not `/exit`.
