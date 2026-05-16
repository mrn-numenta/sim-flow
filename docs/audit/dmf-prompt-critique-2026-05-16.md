# DMF Prompt Critique

Audit of the Direct Modeling Flow (DMF) prompt set under
[tools/sim-flow/prompts/](../../prompts/). Scope: every `dm*` work and
critique prompt, plus the shared `_templates/` and `_conventions/`
files they reference.

Date: 2026-05-16.

## 1. Inconsistencies and contradictions

### 1.1 `ConnectivityPlan` is forbidden in the work prompt but checked in the critique

[dm2d-model-implementation.md:145-150](../../prompts/dm2d-model-implementation.md#L145-L150)
explicitly bans `ConnectivityPlanBuilder` ("the inline `HasInstances`
+ `connect()` style is what the gate expects"). But
[dm2d-model-implementation-critique.md:62-63](../../prompts/dm2d-model-implementation-critique.md#L62-L63)
check 1 still asks "Does the `ConnectivityPlan` topology match
`docs/analysis/pipeline-mapping.md`?" The critique should be rewritten
in terms of `HasInstances` + `connect()`, or use a neutral term such
as "the elaborated topology".

### 1.2 Critique-file extension is misstated in every work prompt except DM0

[dm0-specification.md:169](../../prompts/dm0-specification.md#L169)
says `.json` ("the critique is a distinct task"), but DM1, DM2a, DM2b,
DM2c, DM2cd, DM2d, DM3a, DM3b, DM3c, DM4a, DM4ad, and DM4b all say
`docs/critiques/DMx-critique.md`. The orchestrator renders the `.md`
from the `.json`; the agent should never write either, but the
inconsistent wording invites confusion ("am I forbidden from writing
the `.md` but allowed to write the `.json`?"). Pick one phrasing
(`.json` is correct since that is the file the gate keys on).

### 1.3 Two parallel finding-classification conventions co-exist

[_templates/critique-kinds.md](../../prompts/_templates/critique-kinds.md)
defines the strict-JSON `kind` field
(`"blocker"` / `"unresolved"` / `"resolved"`, all lowercase). But
[dm2c-model-impl-plan-critique.md:28-33](../../prompts/dm2c-model-impl-plan-critique.md#L28-L33)
and
[dm4b-performance-analysis-critique.md:34-37](../../prompts/dm4b-performance-analysis-critique.md#L34-L37)
hand-roll a parallel "Prefix gate-blocking issues with `BLOCKER:`..."
paragraph that smells like the old line-prefix markdown format. Other
critique prompts inline `BLOCKER:` / `UNRESOLVED:` markers inside
check bodies (e.g. dm2c-critique checks 4-10, dm2cd-critique checks
1-10, dm3a-critique checks 2-8). The markdown prefix style and the
JSON `kind` field describe the same gate but use different
vocabularies; the work agent has to learn both. Either drop the prose
prefix convention entirely (rely solely on `kind`) or keep it
consistently. Do not mix.

### 1.4 `{{ third_party_reviewer_note }}` template usage is inconsistent

DM0, DM1, DM2a, DM2b, DM2d, DM3b, DM3c, DM4b critiques use the
template tag. DM2c, DM2cd, DM3a, DM3ad, DM4a, DM4ad **inline** the
same sentence in their own words (e.g.
[dm2c-model-impl-plan-critique.md:3-9](../../prompts/dm2c-model-impl-plan-critique.md#L3-L9),
[dm3ad-test-plan-detail-critique.md:4-8](../../prompts/dm3ad-test-plan-detail-critique.md#L4-L8)).
Same content, two different wordings depending on author/era. All
should use the tag.

### 1.5 Stub-template field names drift between DM2c and DM3a

DM2c stubs have `Scope / Dependencies / Trace / Tasks`. DM3a stubs
have `Scope / Components-Tests / Trace / Tasks`. These are
structurally the same artifact (a milestone stub awaiting
`<!-- detail-pending -->` expansion), but the predecessors-and-deps
field is named differently. DM2cd's critique check 5 looks for
"Dependencies"; DM3ad's critique check 3 looks for "Components/Tests".
A unified milestone-stub schema (with one optional field renamed per
step) would reduce surface area for the agent to misremember.

### 1.6 Coverage-threshold phrasing drifts

DM3a work: "minimum **90% line coverage** on `src/model/`".
DM3a-critique: "90% expected". DM3c-critique: "default 90%". Pick one
canonical phrasing.

### 1.7 DM2d critique requires `lib:` citations the work prompt does not impose

[dm2d-model-implementation-critique.md:71-76](../../prompts/dm2d-model-implementation-critique.md#L71-L76)
says un-cited "pattern" BLOCKERs are not valid. But the work prompt
never tells the implementer "every framework call must trace to a
`lib:` reference", so the agent legitimately writes code that the
critique then cannot cite-check. Either move the citation discipline
into the work prompt too, or soften the critique requirement to "if
you flag a deviation, name a canonical reference."

### 1.8 DM4b critique re-derives a gate rule already covered by the template

[dm4b-performance-analysis-critique.md:34-37](../../prompts/dm4b-performance-analysis-critique.md#L34-L37)
restates the prefix-style gate rule that
[_templates/critique-kinds.md](../../prompts/_templates/critique-kinds.md)
already provides. Remove the paragraph; the template covers it.

### 1.9 DM2d-critique duplicates the embedded-probe list

If
[dm2d-model-implementation.md:164-171](../../prompts/dm2d-model-implementation.md#L164-L171)'s
probe-type list changes, the parallel list in
[dm2d-model-implementation-critique.md:100-101](../../prompts/dm2d-model-implementation-critique.md#L100-L101)
will silently fall behind. Either factor the probe list into a shared
snippet or have the critique reference the work prompt rather than
redeclaring it.

## 2. Unnecessary redundancy

### 2.1 Work-session trailer duplicated 14 times

The trailing "stop, do not write the critique, do not `/exit`" block
appears verbatim in every work prompt (DM0 through DM4b), with
cosmetic variation only.

### 2.2 "Coding Requirements" block duplicated 4x in work and 4x in critique prompts

[dm2d-model-implementation.md:221-252](../../prompts/dm2d-model-implementation.md#L221-L252),
[dm3b-testbench-impl.md:138-169](../../prompts/dm3b-testbench-impl.md#L138-L169),
[dm3c-test-execution.md:153-184](../../prompts/dm3c-test-execution.md#L153-L184),
[dm4b-performance-analysis.md:147-177](../../prompts/dm4b-performance-analysis.md#L147-L177),
plus the matching critique blocks (~30 lines each). 8 near-identical
copies. High drift risk; the DM4b copy already differs slightly (adds
the "reports under `docs/analysis/`" caveat). Single source of truth,
please.

### 2.3 "Pre-stop hygiene" paragraph duplicated 4x

DM2d, DM3b, DM3c, and DM4b each state "`cargo fmt --check` AND
`cargo clippy --all-targets -- -D warnings` are run AUTOMATICALLY by
the orchestrator after you stop ... Do NOT invoke them yourself..."
in near-identical wording.

### 2.4 "Order, jumping, and deferring" paragraph duplicated 4x

DM2d, DM3b, DM3c, and DM4b each cite `plan-management.md` with the
same supporting prose. The DM4b copy has a step-specific note tacked
on, but the leading paragraph is shared.

### 2.5 "Re-entry" section duplicated 4x

DM2d, DM3b, DM3c, and DM4b share the structure "walk files in numeric
order, first one with open rows is your milestone, do not skip
checked-off milestones if build/test fails", with step-specific tail.

### 2.6 Single-file vs paginated layout paragraph duplicated 4x

DM0 (spec), DM1 (targets), DM2a (decomposition), and DM2b
(pipeline-mapping) each repeat the same warning ("Pick one layout per
project and stick with it") and the numbered-prefix convention.

### 2.7 Critique-output trailer duplicated 14 times

Every critique prompt ends with "Write the critique as JSON to
`docs/critiques/DMx-critique.json`. The orchestrator renders a
human-readable `.md`...". Identical except for the step ID.

### 2.8 Third-party-reviewer framing partly templated, partly inlined

Already a template (`{{ third_party_reviewer_note }}`), but only half
of the critique prompts use it. See 1.4.

### 2.9 `<!-- detail-pending -->` reminder duplicated 6 times

Appears in DM2c, DM2cd, DM3a, DM3ad, DM4a, DM4ad with the same
load-bearing warning each time.

### 2.10 "Stubs describe WHAT, not HOW" rule duplicated 6 times

Same six prompts as 2.9, with six different phrasings of the same
prohibition on citing specific framework APIs.

### 2.11 `BLOCKER:`/`UNRESOLVED:` prose duplicates the JSON template

DM2c-critique and DM4b-critique re-derive what critique-kinds.md
states. See 1.3.

### 2.12 `orchestrator-native-tools.md` already handles critique outputs

[_conventions/orchestrator-native-tools.md:78-80](../../prompts/_conventions/orchestrator-native-tools.md#L78-L80)
addresses critique outputs in native-tool mode. Every critique prompt
nonetheless rehashes that the JSON-vs-MD split is the orchestrator's
job. The convention covers it.

## 3. Sections that should be factored into templates

Based on the redundancies above, the following snippets are clear
template candidates:

| Proposed template | Replaces | Used by |
|---|---|---|
| `{{ work_session_trailer(step_id) }}` | "stop, do not write the critique, do not `/exit`" trailer | All 14 work prompts (2.1) |
| `{{ coding_requirements }}` | Rust coding rules (6 bullets) | DM2d, DM3b, DM3c, DM4b work (2.2) |
| `{{ coding_requirements_checks }}` | Critique-side version of the same 6 rules | DM2d, DM3b, DM3c, DM4b critique (2.2) |
| `{{ pre_stop_hygiene }}` | "fmt/clippy run automatically" paragraph | DM2d, DM3b, DM3c, DM4b work (2.3) |
| `{{ order_jumping_deferring }}` | The plan-management.md reference paragraph | DM2d, DM3b, DM3c, DM4b work (2.4) |
| `{{ re_entry(file_glob) }}` | "walk files in numeric order" re-entry block, parameterized on the milestone-file glob | DM2d, DM3b, DM3c, DM4b work (2.5) |
| `{{ dual_layout(artifact_name) }}` | The single-file-vs-paginated layout paragraph | DM0, DM1, DM2a, DM2b work (2.6) |
| `{{ critique_output_block(step_id) }}` | "Write the critique as JSON to ..." (incorporating `critique_json_schema`) | All 14 critique prompts (2.7) |
| `{{ stub_template_rules }}` | The `<!-- detail-pending -->` + "stubs describe WHAT, not HOW" framing | DM2c, DM2cd, DM3a, DM3ad, DM4a, DM4ad (2.9, 2.10) |
| `{{ third_party_reviewer_note }}` (existing) | Hand-rolled paraphrases | DM2c, DM2cd, DM3a, DM3ad, DM4a, DM4ad critique (1.4, 2.8) |

Additionally:

- **A shared stub-template definition** for milestones (covering
  DM2c / DM3a / DM4a). Today each step inlines its own version with
  subtly different field names (1.5). One canonical
  `Scope / Predecessors / Trace / Tasks` shape with per-step field
  labels would let DM2cd / DM3ad / DM4ad share a critique snippet too.
- **The embedded-probe list** (DM2d work lines 168-170; critique
  lines 100-101) should live in one place, likely a
  `_templates/dm2d-embedded-probes.md` snippet referenced from both.

## 4. Clarity and conciseness

### 4.1 DM2d's block-diagram contract is buried under `## Constraints`

It is actually a structural requirement on the source layout, equal
in weight to the procedure itself. Promote it to its own section
right after `## Procedure`. Right now the most failure-prone rule in
DM2d (renaming `Top` breaks the gate even though the code compiles)
is on line 256, after 250 lines of setup.

### 4.2 DM2d is 18 KB - by far the longest prompt

The "Inputs" / "Reference material" section (~60 lines) describing
`lib:` vs `fw:` vs `api_*` could be heavily condensed by linking to a
shared `_conventions/reference-material.md` snippet. The current
treatment repeats most of `fw:api/toc.md`'s preamble inline.

### 4.3 Section ordering varies between prompts

DM0 has "Two kinds of 'spec'" between Goal and Procedure; DM1 jumps
Goal -> Procedure; DM2c has "Inputs" between Goal and Procedure; DM2d
has Goal -> Inputs -> Reference -> Procedure -> Coding Requirements
-> Constraints -> Re-entry -> Output. A consistent canonical order
(Goal -> Inputs -> Reference Material -> Procedure -> Constraints ->
Output -> Re-entry) would make the flow predictable.

### 4.4 The auto-mode convention is doing two jobs

[_conventions/auto-mode.md](../../prompts/_conventions/auto-mode.md):
the first ~60 lines are general "you cannot ask questions, document
decisions" rules that apply to every step. The remaining ~150 lines
(investigation vs fix-attempts, `declare_fix`, bug log) only matter
for steps with a cargo-test loop (DM2d, DM3c, parts of DM4b). Mixing
them means DM0 / DM1 / DM2a / DM2b / DM2c / DM2cd / DM3a / DM3ad /
DM4a / DM4ad / DM3b prompts all carry irrelevant `declare_fix`
instructions in their system stack. Split into `auto-mode.md`
(general) and `auto-mode-test-loop.md` (DM2d / DM3c / DM4b additions).

### 4.5 DM0 vs DM1 gate-budget rule is in tension

[dm0-specification.md:106-113](../../prompts/dm0-specification.md#L106-L113)
explicitly forbids LLM-derived estimates ("do NOT silently invent a
number or derive one yourself").
[dm1-modeling-setup.md:54-66](../../prompts/dm1-modeling-setup.md#L54-L66)
then says "Otherwise, derive a reasonable gate-budget-per-cycle
estimate from the frequency and technology target." These are in
tension. DM0 says the human must provide it; DM1 says DM1 derives it
if DM0 did not. Either DM0's rule is too strict or DM1's permission
is too loose. Pick one. Today's reader has to infer the resolution
(DM0 hard-stops; DM1 only derives if DM0 already approved deriving).

### 4.6 Goal / Procedure / Inputs overlap

DM2d's Goal says "Execute the implementation plan ... to produce a
cycle-accurate sim-foundation model that elaborates and passes smoke
tests." The procedure restates this; the constraints restate it
again. A typical prompt could be 20-30% shorter without losing
information.

### 4.7 DM3a is hard to skim

The testbench-design content (step 2, lines 54-72) is mixed with the
milestone-breakdown content (steps 3-7). The two are different
artifacts (an architectural decision vs a planning decomposition).
Splitting them into two procedure sections would let DM3a's critique
key cleanly off each one.

### 4.8 DM0's section 6 conflates gate-checked and soft items

12 "must include" bullets mix gate-checked items (clock frequency,
gates-per-cycle) with soft-recommended items (open questions,
examples). The gate-checked items should be a distinct, smaller list
- easy to verify mechanically - and the rest a separate "should
include for completeness" list.

### 4.9 DM2d's `## Constraints` casing and ordering are inconsistent

8 "Do not" / "DO NOT" items (some all-caps, some not). After 250
lines of context the reader's eye glazes; the critical block-diagram
contract is the first one, followed by 7 less-critical ones. Order
by criticality and standardize the casing.

### 4.10 Some critique checks duplicate work the orchestrator already does

DM3b-critique check 6 ("Does `cargo build` succeed? ... Confirm via
the `run_cargo` tool"), DM3c-critique check 3, DM4b-critique check 7
("Is there at least one row in `experiments.db`...") are
mechanically checkable. If the orchestrator's gate already enforces
them, the critique check is redundant (and the reviewer might
double-fail the same condition).

## 5. Recommended highest-impact fixes (priority order)

1. **Fix the `ConnectivityPlan` contradiction in DM2d-critique check
   1** (1.1). The only outright-broken instruction.
2. **Standardize the `.json` extension across every "do not write the
   critique" trailer** (1.2). Easy mechanical fix.
3. **Adopt `{{ third_party_reviewer_note }}` in the 6 critique
   prompts that inline it** (1.4). Easy mechanical fix.
4. **Extract `{{ coding_requirements }}` and
   `{{ coding_requirements_checks }}` templates** (2.2). The largest
   single source of duplication (~240 lines across 8 files).
5. **Drop the markdown-prefix `BLOCKER:` / `UNRESOLVED:` vocabulary
   from prompts that produce JSON** (1.3, 1.8). Collapses two
   conventions into one. Keep the prefix convention only for inline-
   finding mentions inside check bodies, where it is useful shorthand.
6. **Split `auto-mode.md` into general + test-loop subfiles** (4.4).
   Reduces system-stack noise for ~10 of 14 steps.
7. **Promote DM2d's block-diagram contract out of `## Constraints`**
   (4.1).
8. **Resolve the DM0 vs DM1 gate-budget tension** (4.5). One sentence
   in DM1 that defers cleanly to DM0's rule.

The templating infrastructure already exists
(`{{ output_intro }}` / `{{ critique_kinds }}` /
`{{ critique_json_schema }}` / `{{ third_party_reviewer_note }}`), so
item 4 is mostly a refactor, not a redesign. The prompts have grown
organically; the bones are good.
