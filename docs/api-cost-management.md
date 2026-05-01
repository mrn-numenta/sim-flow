# API cost management proposal

Draft notes for reducing Anthropic API spend in the JSONL-host LLM
path. Captured for review; **not yet implemented.** Revisit after
working through the live flow and prompt set so we can validate the
ranking with empirical token telemetry.

Numbers below assume Claude Sonnet 4.6 ($3 / MTok input, $15 / MTok
output) since the dashboard's API path defaults there. Same
percentages apply for Opus 4.7 ($5 / MTok input, $25 / MTok output);
Haiku 4.5 is $1 / $5.

## Where the money goes today

For each work or critique session in JSONL-host mode (sources:
`vscode`, `anthropic`, `openai`, `ollama`, `lmstudio`), the
orchestrator inlines:

| Block | Approx tokens | Stable across turns? |
|---|---:|---|
| `_conventions/native-tools.md` | ~800 | yes |
| `_conventions/auto-mode.md` (when `auto: true`) | ~480 | yes |
| `_conventions/fenced-blocks.md` (JSONL only) | ~290 | yes |
| Step prompt (e.g. `dm2d-model-implementation.md`) | ~1,800 | yes within session |
| Spec TOC (when ingested) | 200–4,000 | yes within project |
| Predecessor TOC + tools list | 500–1,500 | yes within session |
| Conversation history (predecessors fetched, prior turns) | grows | yes (within turn N→N+1) |

A typical session is **6–12 turns** before the structural gate
clears. Today every one of those turns re-bills the entire system
stack at full input price. There is **zero prompt caching** wired in
any LLM adapter (`tools/sim-flow/extensions/sim-flow-vscode/src/llm/anthropic.ts`
joins system blocks into a plain string and passes them as
`system: <string>` — we'd need to switch to the array form with
`cache_control` markers).

Worked example, Sonnet 4.6, DM2d work session, 8 turns, ~10k system
tokens per turn, ~3k assistant output per turn:

- **No cache (today):** 80k system + 24k content @ $3 + 24k output
  @ $15 ≈ **$0.67 / session**
- **With 5-minute cache on the system stack:** 1 write @ 1.25× +
  7 reads @ 0.1× ≈ 13.5k effective input ≈ **$0.21 / session**
  (–69%)
- **With 1-hour cache (covers a whole step):** 1 write @ 2× +
  7 reads @ 0.1× ≈ 12.7k effective input ≈ **$0.20 / session**
  (–70%)

11 sessions across DM0 → DM4b: roughly **$7.40 → $2.20** for the
same flow on Sonnet, before output savings.

**Note:** PTY/CLI agents (Claude Code, codex, gh-copilot) use the
user's existing subscription auth — flat rate, not per-token. The
wins below mostly target the JSONL host path. The PTY path has its
own benefit (avoiding huge inlined pastes), addressed in B3.

## Tier A — high impact, low risk

**A1. Wire prompt caching on the system stack in
`llm/anthropic.ts`.** 5-minute ephemeral cache, marked on the last
system block. Pays back after one cache read; sim-flow's turn
cadence (always sub-5-minute) means the cache is essentially always
warm. Estimated –65 to –75% on input tokens for multi-turn sessions.
Touch points:

- Switch `system: string` → array form in the request body.
- Append `cache_control: {type: "ephemeral"}` on the final system
  block.
- Confirm `usage` parsing surfaces `cache_creation_input_tokens` and
  `cache_read_input_tokens` for cost telemetry.

**A2. 1-hour cache on the convention block specifically.** The three
`_conventions/*.md` files don't change for hours within a project. A
separate cache breakpoint on just that subblock (write = 2× base,
read = 0.1×) pays back after the second turn and survives across
sessions in the same step. Estimated extra –5 to –10% on top of A1
because conventions cover the same content in every session.

**A3. Default `verbose=false` for the dashboard.** Output tokens are
5× input. The `verbose` setting already exists and prepends "be
concise" — flipping the default would meaningfully cut output across
all the natural-language responses (the artifact-write tool calls
themselves are already terse). Conservative estimate: –20% output
≈ –15% session cost. Reversible per-user via the Settings tab.

**A4. Skip work session re-runs when the gate already passed.**
Already partially handled, but worth auditing: the dashboard's Run
Step shouldn't re-fire the prompt for a step whose gate is clean. If
it currently does (need to verify), that's free savings.

## Tier B — medium impact, requires UI/design

**B1. Per-step model routing.** Spec elicitation (DM0), critique
sessions, and the Plan steps (DM2c / DM3a / DM4a) almost certainly
run fine on Haiku 4.5 at $1 / MTok input vs $3 for Sonnet. The
implementation-heavy steps (DM2d / DM3c / DM4b) keep Sonnet or move
to Opus 4.7 ($5 input — same as Sonnet 4 was, much cheaper than
Opus 4.1's $15). Implementation options:

- Extend `sim-flow.llm.model` to accept an object/per-step map.
- Or add a "Cheaper for plans" toggle that auto-routes by step kind.

Estimated –30 to –50% across the flow if Haiku handles half the
sessions.

**B2. Trim the convention stack.** With "Session boundaries" added
to `native-tools.md`, there's now overlap between `native-tools.md`
and `auto-mode.md` (both touch on "stop and wait" and gate
semantics). Audit + merge could shave 20–30% off the convention
block (–~500 tokens × every turn × every session). Bonus: less for
the agent to reason over.

**B3. Lazy-load the step prompt for JSONL agents too.** Today the
full step prompt is inlined into every turn. Alternative: inline a
one-line bootstrap ("Read `tools/sim-flow/prompts/<step>.md` for
instructions") and let the agent fetch it once. The Read tool
result lands in conversation history, where it gets cached
automatically by A1/A2. This already exists for PTY agents; bringing
it to JSONL agents (which DO have a tools-array Read or can be given
one) is the same idea. Estimated –10 to –20% on top of A1 because
the step prompt is the largest stable chunk.

**B4. Cache predecessor artifact fetches.** When the agent fetches a
predecessor with `read_file`, the result enters conversation history.
An additional `cache_control` breakpoint after the first
user→assistant turn locks in the predecessor content for the rest of
the session. Naturally implemented via the Anthropic SDK's
auto-cache; explicit breakpoints give finer control.

## Tier C — speculative or special-case

**C1. Batch API for the fully-automated red-button flow.** 50% off
input AND output, but async (24h SLA), so only fits unattended
end-to-end runs that can wait. Probably not worth implementing for
typical use; if we ever want to run nightly cleanups across many
projects, revisit.

**C2. Conversation summarization at turn N ≥ 10.** When a session
drags long (rare, but happens on DM2d), summarize early turns rather
than re-sending them. Risky — the agent might lose context — and
only relevant for marathon sessions. Lower priority.

**C3. Cache the spec TOC at project lifetime.** The TOC doesn't
change once ingested. Could be pinned in 1-hour cache or even a
dedicated "permanent" breakpoint at the very start of the system
stack. Stacks with A2.

## Recommended phasing

1. **Land A1 + A3 first.** Roughly 1–2 days of extension work.
   Single biggest dollar impact, no UX changes for the user.
2. **Add cost telemetry** — surface `cache_creation_input_tokens` /
   `cache_read_input_tokens` from Anthropic's response in the
   dashboard's Runs table or a new "Tokens" footer. Validate the
   savings empirically before tier B.
3. **A2 + B2 + B3** as a follow-up. They reinforce each other — a
   leaner system stack with a 1-hour cache breakpoint on the truly
   stable parts.
4. **B1** last and only if telemetry shows planning steps are still
   pricier than they need to be. Adds dashboard complexity (per-step
   model picker), so worth deferring until the data justifies it.

## Out of scope for this proposal

- Per-step prompt content quality (already covered by the
  mode-neutral updates in the conventions and per-DM prompt files).
- The PTY / CLI agent flow (subscription auth, not per-token).
- The auto-iteration cap and runaway-loop guards.
- Any user-facing change in the Flow tab UI.

## Reference: 2026-04 pricing snapshot

Captured from <https://platform.claude.com/docs/en/about-claude/pricing>
for the models the dashboard currently exposes. All prices in
USD per million tokens (MTok); MTok = 10^6 tokens.

| Model | Input | 5m cache write | 1h cache write | Cache hit | Output |
|---|---:|---:|---:|---:|---:|
| Opus 4.7 | $5 | $6.25 | $10 | $0.50 | $25 |
| Opus 4.6 | $5 | $6.25 | $10 | $0.50 | $25 |
| Sonnet 4.6 | $3 | $3.75 | $6 | $0.30 | $15 |
| Haiku 4.5 | $1 | $1.25 | $2 | $0.10 | $5 |

Cache mechanics:

- Cache writes pay 1.25× base for 5-minute TTL or 2× base for
  1-hour TTL.
- Cache reads pay 0.1× base regardless of TTL.
- 5-minute cache pays back after **1** cache read; 1-hour cache
  pays back after **2**. sim-flow sessions clear both bars easily
  inside a single multi-turn session.
- Batch API: 50% off input AND output. Async (24h SLA); not usable
  for interactive flow.
