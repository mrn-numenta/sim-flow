# DM3b - Testbench Implementation (critique session)

You are reviewing the DM3b testbench scaffolding. Treat it as
work produced by a third party even if you produced it yourself
earlier in this conversation -- the independent-review property
depends on you bracketing any prior reasoning rather than leaning
on it. Do not modify the testbench; evaluate it and write the
critique file.

This critique runs more than once:

- after each `tb-milestone-NN-*.md` is checked off, to validate
  the just-landed slice before the next milestone starts
- once after the FINAL tb milestone, as the lighter end-to-end
  integration / regression check across the full testbench

Determine which milestone was just completed by walking
`docs/test-plan/tb-milestone-NN-*.md` files in numeric order and
finding the highest-numbered one whose rows are all resolved
(`- [x]` or `- [-]` with a defer-reason). Review that milestone in
detail, and also sanity-check that the new work didn't regress
earlier milestones.

## Inputs

- `docs/impl-plan/plan-management.md` -- plan-file conventions
  including the 10-task cap.
- `docs/testbench.md` -- DM1's verification strategy. Specifically
  `## Implementation Baseline` names the
  `lib:examples/<NN-name>/test/` directory DM3b is mirroring.
- `docs/test-plan/test-plan.md` -- index. Testbench architecture +
  the contract DM3b is implementing.
- `docs/test-plan/tb-milestone-NN-<name>.md` -- per-milestone task
  lists. Verify every row in the just-completed milestone is
  `- [x]` or `- [-]` with a `defer reason:`.
- `tests/` source tree (or the test module DM3b is building).
- `src/` -- model under test, for confirming Monitors observe the
  right ports.
- Reference material on demand:
  - The named baseline `lib:examples/<NN-name>/test/`. Read it to
    compare structure when judging baseline fidelity.
  - `lib:docs/modeling-guide/04-testing-models.md` for canonical
    UVM-lite patterns.
  - `fw:api/toc.md` -> the specific `fw:api/pages/...` pages for
    exact API signatures.

## Evaluation

Record findings in the critique JSON (see "Output" below for the
schema). Use `kind: "blocker"` for gate-blocking issues (DM3c
cannot proceed until fixed), `"unresolved"` for informational
notes, `"resolved"` for historical / retry-mode acknowledgements.
The orchestrator fails the DM3b gate on `"blocker"` findings
only.

1. **Milestone completeness**. Identify the just-completed
   milestone (the highest-numbered `tb-milestone-NN-*.md` whose
   rows are all resolved). Is every row in that file `- [x]` or
   `- [-]` with a specific `- defer reason:` sub-bullet? Reject
   silently-skipped rows. On the FINAL critique, all
   tb-milestone files must be fully resolved.

2. **Task fidelity**. For each `- [x]` row in the just-completed
   milestone, does the named artifact actually exist in `tests/`
   with the symbol the task said it would? Quote the row and
   the implementing source location. Reject rows ticked off
   without the artifact landing.

3. **Baseline fidelity**. Quote the
   `lib:examples/<NN-name>/test/` baseline named in
   `docs/testbench.md`'s `## Implementation Baseline`. Does the
   `tests/` file structure mirror that baseline (file layout,
   module split, `SimEnvBuilder` call site)? Silent
   substitution of a different baseline is a `BLOCKER:`. A
   genuine reason to deviate is also a `BLOCKER:` -- surface it
   so DM1 can be revisited rather than fixed forward.

4. **UVM-lite topology**. Sequencer -> Driver -> DUT -> Monitor
   -> Scoreboard intact in the artifacts that landed this
   milestone? Do any components reach into internal model
   state they should observe via Monitors?

5. **Payload / port fidelity**. Do new Drivers and Monitors use
   payload types and port names that match `src/`,
   `docs/spec.md`, and `docs/analysis/data-movement.md`? Flag
   mismatches explicitly.

6. **Build state**. Does `cargo build` succeed? On the smoke
   milestone (typically the last tb-milestone), does
   `cargo test` also succeed for the basic data-flow smoke
   test? Confirm via the `run_cargo` tool; don't infer from
   source.

7. **Public API discipline**. Does the new code stay within the
   public framework surface reachable from `fw:`? Reject reliance
   on internal helper modules or non-curated framework internals
   when a public API page does not justify it.

8. **Scope discipline**. Does DM3b stay out of DM3c territory?
   The boundary is "categories named in the test plan", not
   "count of `#[test]` functions":
   - **In scope for DM3b**: Sequencer / Driver / Monitor /
     Scoreboard bodies (their internal assertion / comparison
     logic is scaffolding, not tests); the `SimEnvBuilder`
     helper; the basic data-flow smoke test from the final
     tb-milestone; small scaffolding-verification tests that
     prove the scoreboard / wiring fire correctly.
   - **Out of scope for DM3b** (`BLOCKER:` if newly-authored
     code does this): any `#[test]` that maps to a row in a
     `test-milestone-NN-*.md` file (smoke beyond the basic
     data-flow entry, edge, stress, random). Those belong to
     DM3c.
   - **Pre-existing tests from earlier steps are NOT DM3b's
     responsibility**: DM2d may legitimately have left smoke
     tests under `tests/`. Do not flag DM2d-era tests as DM3b
     scope violations. Only `BLOCKER:` tests that DM3b itself
     authored this step in scope-violating categories.

9. **Plan fidelity**. If DM3b deviated from the milestone task
   text (renamed components, added rows not in the plan, skipped
   rows), flag every deviation. The `tb-milestone-NN-*.md` rows
   are the contract. Genuine plan errors should produce a
   `BLOCKER:` so DM3a can be revisited.

10. **Milestone composability**. If this is a milestone
    checkpoint critique rather than the final DM3b review, is
    the just-completed milestone solid enough that the next
    tb-milestone can safely build on it? If this is the final
    review, do the milestone-local artifacts compose cleanly
    into a working testbench end-to-end without regression?

11. **Coding Requirements (per the work prompt)**. Inspect every
    Rust source file landed or modified in this milestone:
    - **Idiomatic Rust**: any non-idiomatic patterns
      (manual loops over iterators, `unwrap()` in non-test paths,
      nested `if let` instead of `match`, `Box<dyn _>` where a
      concrete type fits) -> `BLOCKER:` with the file/line.
    - **Magic numbers / strings**: any inlined literal that
      represents a port name, payload width, threshold, or
      run-id pattern -> `BLOCKER:`. Reject "well, it's only
      used once" exceptions.
    - **Emojis**: any non-ASCII decorative glyph in code,
      comments, doc strings, error messages, or string literals
      -> `BLOCKER:`. Quote the offending line.
    - **File size cap**: run a line count on every Rust file
      authored or modified this milestone. Any file at or above
      400 lines -> `BLOCKER:` with the line count and a
      suggested split axis.

12. **File Layout (per the work prompt)**. Verify the
    `tests/testbench/` subdirectory split:
    - Testbench scaffolding lives under `tests/testbench/<file>.rs`
      (one file per concern: `payloads.rs`, `sequencers.rs`,
      `drivers.rs`, `monitors.rs`, `scoreboards.rs`, `env.rs`,
      `mod.rs` as module root).
    - The basic data-flow smoke test lives at
      `tests/smoke/basic_data_flow.rs` (NOT inside
      `tests/testbench/`).
    - Any monolithic `tests/testbench.rs` file -> `BLOCKER:`.
      Any testbench code dumped into `tests/tests.rs` ->
      `BLOCKER:`. The split-by-concern is what makes per-
      milestone review tractable.

## Output

Write the critique as JSON to
`docs/critiques/DM3b-critique.json`. The orchestrator renders a
human-readable `docs/critiques/DM3b-critique.md` from that JSON
automatically; do NOT write the markdown yourself.

### JSON schema

```json
{
  "step": "DM3b",
  "summary": "1-paragraph summary of the critique outcome.",
  "findings": [
    {
      "kind": "blocker",
      "section": "free-form section name",
      "title": "one-line summary of the finding",
      "body": "multi-line markdown explanation"
    }
  ],
  "notes": "optional free-form trailing prose"
}
```

`kind` values: `"blocker"`, `"unresolved"`, `"resolved"`. Schema
is strict (`deny_unknown_fields`); typos fail the parse.
