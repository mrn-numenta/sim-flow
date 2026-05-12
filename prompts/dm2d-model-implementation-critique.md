# DM2d - Model Implementation (critique session)

You are reviewing the DM2d work artifacts (the model
implementation under `src/`). {{ third_party_reviewer_note }} Do not
modify the implementation; evaluate it and write the critique file.

This critique runs more than once:

- after each milestone-complete checkpoint, to validate the newly
  landed implementation slice before the next milestone begins
- once after the final milestone, as a lighter end-to-end
  integration/regression check

Determine which milestone was just completed from the plan files,
review that milestone in detail, and also sanity-check that the new
work did not regress earlier milestones.

## Inputs

Project artifacts:

- `docs/impl-plan/plan.md`
- `docs/impl-plan/milestone-*.md`
- `docs/targets.md`
- `docs/testbench.md`
- `docs/analysis/pipeline-mapping.md`
- `docs/analysis/decomposition.md`
- `docs/analysis/data-movement.md`
- `src/model/` source tree
- `tests/` source tree

Framework / pattern references (consult these BEFORE flagging a
"deviates from Foundation patterns" or "violates conventions"
BLOCKER -- the references are the canonical patterns, not your
internal priors):

- **Primary** (curated and small enough to read end-to-end):
  - `lib:docs/modeling-guide/06-design-patterns.md` -- the
    Foundation design-pattern reference.
  - `lib:docs/modeling-guide/03-building-models.md` and
    `lib:docs/modeling-guide/04-testing-models.md` -- structural
    conventions for model code and tests.
  - `lib:examples/<topology-match>/` -- start with
    `lib:examples/README.md` and read the example whose topology
    most closely matches the design under review (typically
    `lib:examples/01-three-stage-pipeline/` for staged datapaths,
    `lib:examples/04-combinatorial-logic/` for purely combinational
    work). Reference the example's module / port / test layout when
    judging "Foundation conventions".
- **Secondary** (large; consult on demand for exact API signatures
  only -- do NOT bulk-read):
  - `fw:api/toc.md` to find a specific page, then
    `fw:api/pages/...` for the one signature you need.
  - `cargo doc --workspace --no-deps` rendered output is also
    available but is verbose; prefer the curated `fw:api/pages/...`
    files unless a signature is genuinely missing there.

## Evaluation

{{ critique_kinds }}

1. Does the `ConnectivityPlan` topology match
   `docs/analysis/pipeline-mapping.md`?
2. Does each module's `evaluate()` implement the operation(s)
   assigned to that stage in `decomposition.md`?
3. Are payload types consistent with the data widths, types, and
   fanouts in `data-movement.md`?
4. Are there any custom implementations that deviate from
   Foundation patterns (bypassing the port system, manual
   scheduling, violating the evaluate / settle / update phase
   order)? When raising a BLOCKER here, **cite the specific
   `lib:docs/modeling-guide/...` line or `lib:examples/<example>/...`
   file that defines the canonical pattern**. Un-cited "pattern"
   BLOCKERs based on internal priors are not valid -- downgrade to
   `UNRESOLVED:` if you cannot point at the canonical reference.
5. Do all smoke tests pass? Are they meaningful (elaboration, data
   flow, plus any flow-control / idle-cycle tests that
   `docs/testbench.md` explicitly required for this design) or
   trivial? Purely combinational designs need only elaboration +
   data-flow smoke tests; do not flag missing backpressure / idle
   tests when the design has no flow-control surface.
6. Is the code organized per Foundation conventions (model / sim /
   test split)? When the layout under review differs from a
   canonical `lib:examples/...` example, **cite which example you
   compared against** and quote the differing structure. Un-cited
   "convention" BLOCKERs are not valid.
7. Are operation names from the decomposition reflected in module
   or type names?
8. Does the implementation preserve target-sensitive structural choices
   implied by `docs/targets.md` and encoded in the plan / mapping
   (for example stage boundaries, buffering, or other gate-budget-driven
   decisions) rather than drifting away from them?
9. Does the implementation provide the structural support needed for the
   smoke-test and observability intent captured in `docs/testbench.md`
   where that support had to be designed in during implementation?
10. Does the implementation match the plan? Every milestone in
   `docs/impl-plan/` should have its tasks all `[x]`. Flag tasks still
   `[ ]` (incomplete) and code that doesn't trace back to a plan
   task (out-of-scope drift).
11. Did the implementation introduce major architectural structures or
    boundaries that are not reflected in DM2c's plan or the DM2a/DM2b
    artifacts?
12. If this is a milestone critique rather than the final DM2d review,
    is the just-completed milestone solid enough that the next
    milestone can safely build on it? If this is the final review,
    do the milestone-local decisions still compose cleanly
    end-to-end without regression?

13. **Coding Requirements (per the work prompt)**. Inspect every
    Rust source file under `src/` landed or modified in this
    milestone:
    - **Idiomatic Rust**: any non-idiomatic patterns
      (manual loops where iterators fit, `unwrap()` in non-test
      paths, nested `if let` where `match` would read better,
      `Box<dyn _>` where a concrete type fits) -> `BLOCKER:`.
    - **Magic numbers / strings**: inlined port names, payload
      widths, stage indices, threshold values, etc. -> `BLOCKER:`.
      Reject "well, it's only used once" exceptions.
    - **Emojis**: any non-ASCII decorative glyph in code,
      comments, doc strings, error messages, or string literals
      -> `BLOCKER:`.
    - **File size cap**: line-count every Rust file authored
      or modified this milestone. Any file at or above 400
      lines -> `BLOCKER:` with a suggested split axis (typically
      one module / one component class per file).

## Output

{{ output_intro }}

Write the critique as JSON to
`docs/critiques/DM2d-critique.json`. The orchestrator renders a
human-readable `docs/critiques/DM2d-critique.md` from that JSON
automatically; do NOT write the markdown yourself.

{{ critique_json_schema }}