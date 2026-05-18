# Chat Panel History: Windowing & Render Strategy — Brainstorm

Status: brainstorm (no decision yet).
Context: 2026-05-17 — a 20-minute manual session accumulated ~2k
`llm-request` bubbles + ~3k `assistant-text` chunk events. The
experimental chat panel rendered for a while, then froze ("all
light gray, no console errors"). Recovery required reloading the
webview.

## 1. What the chat panel does today

### Data shape

The orchestrator owns the truth (`state.toml`, persisted artifacts).
The chat panel keeps its own **transcript** — a flat array of
`ChatTranscriptEntry` rows held in `ChatConversationState`:

```ts
interface ChatConversationState {
  transcript: ChatTranscriptEntry[];
  nextId: number;
}
```

One entry per visible bubble. Entries come from:

- User prompts the human typed (one entry per send).
- Assistant turns (one entry per turn; chunks accumulate into
  `body`; reasoning chunks accumulate into `reasoning`).
- `llm-request` events — one entry per prompt-stack message the
  orchestrator added that turn (system, user, tool). **This is the
  dominant source of bubble count: the standing system prompt
  re-appears every turn × every sub-session.**
- Notes (session-info banners, eviction notices).
- Tool / artifact activity markdown (collapsed into prior bubbles
  in some cases, separate notes in others).

State lives in three places:

1. `ChatPanelProvider.conversations: Map<key, ChatConversationState>` —
   in-memory cache per project.
2. `vscode.Memento` (`workspaceState`) — debounced 250ms persist
   so a reload survives mid-stream.
3. Webview DOM — built by `panelExperimental.ts`'s `buildShell()` →
   `buildTranscript()` → one DOM row per entry.

### State delivery

Every change calls `postState(context, conversation)` which:

1. Calls `buildState()` to assemble a full `ChatPanelState`
   (including the **entire** `transcript: ChatTranscriptEntry[]`).
2. Sends a single `{ type: "state-update", state }` message to the
   webview.
3. Webview replaces `ui.state` and calls `render()`.
4. `render()` builds a fresh `<main id="app">` off-screen, then
   `morphdom`-patches it onto the live DOM.

This works fine at ~10–50 entries. It collapses at thousands.

### Where the cliff is

For a transcript of `N` entries, every chunk that arrives during
streaming triggers:

- Host: full conversation copy → state object → `postMessage`
  IPC. JSON-serialized whole transcript over the IPC channel on
  every token-sized delta. For a 3k-entry transcript with multi-KB
  bubble bodies (system prompt repetitions), each delta ships
  **megabytes** over IPC.
- Webview: rebuild `N` DOM rows from scratch, then morphdom
  walks the existing tree comparing nodes. The
  `onBeforeElUpdated` hook calls `fromEl.isEqualNode(toEl)` to
  skip unchanged subtrees; `isEqualNode` is a deep structural
  walk per node. Net cost is **superlinear** in `N`.
- Markdown rendering inside `bubbleDetails(...)` runs per entry
  per render. No memoization keyed by `(id, body)`.

The user sees the panel freeze hard enough that VS Code's
webview watchdog stops painting; no JS exception fires, so
DevTools is clean.

## 2. What "fix this" really means

Two distinct problems:

- **A. Render cost per state-update.** Even if the user only
  cares about the most recent turn, we redraw everything.
- **B. Memory + IPC cost of "keep everything forever."** The
  transcript is unbounded; system prompts and large tool outputs
  pile up in workspaceState too.

Solving A without B still leaves us writing megabytes per token
across the IPC channel. Solving B without A still freezes when
the user scrolls up.

Best approach probably tackles both via the same lever:
**decouple "what's in memory" from "what's rendered now."**

## 3. Approaches considered

### 3.1 Virtual scrolling (windowed DOM)

**Idea.** The DOM only ever holds the rows that fit in the
viewport plus a small overscan buffer (say, 20 above and below).
A scroll listener swaps rows in and out as the user scrolls.
Row heights are measured lazily; total scroll-height comes from
a sum estimate.

**Pros.**
- Constant-time render regardless of transcript size. Standard
  pattern for chat UIs (Slack, Discord, ChatGPT).
- The scrollbar still represents the full history; the user
  doesn't lose access to older content, just doesn't pay to
  draw it.

**Cons.**
- Variable-height rows (markdown bubbles + code blocks + collapsed
  `<details>`) make height measurement messy. Naive estimates
  cause scrollbar jitter when the user lands on a region with
  taller rows.
- Browser-level "Find in page" (Cmd-F) only searches what's in
  the DOM. Power users expect to grep the transcript visually;
  this breaks that. Mitigation: forward Cmd-F to a custom search
  that scrolls to matches.
- morphdom doesn't help here — virtualization needs explicit
  row recycling. Implementation is a few hundred lines.

**Libraries:** none built into VS Code for webview-side
virtualization. Common JS options: `@tanstack/virtual`,
`react-window`, or hand-rolled (~200 LOC). We don't use React,
so `@tanstack/virtual`'s framework-agnostic core is the closest
fit.

### 3.2 Host-side windowing only

**Idea.** Keep the full transcript in `ChatConversationState` and
`workspaceState`, but only ship a **window** of it to the
webview. The window is, e.g., "the last 200 entries" or "every
entry tagged with `step === currentStep` plus the last N from
prior steps."

The webview never sees more than `window.length` entries, so
DOM size is bounded, morphdom is fast, IPC payload is small.

`ChatPanelState.transcript` grows a sibling field:

```ts
transcriptWindow: ChatTranscriptEntry[]; // bounded
omittedBefore: number;                   // count of older
omittedFrom: string | null;              // earliest id we sent
```

The webview shows a "Show earlier..." stub at the top when
`omittedBefore > 0`. Clicking it asks the host to widen the
window (pull older entries from `conversations`).

**Pros.**
- Minimal webview changes — same render path, just shorter
  transcript array.
- Solves both A and B in one shot: IPC payload shrinks AND
  morphdom has less work.
- Plays well with our existing persistence (workspaceState
  still has the full record).

**Cons.**
- Loses true "scroll up to see history" UX — the user has to
  click "Show earlier..." each time. Mitigation: auto-widen
  when scroll position reaches the top.
- Need to think about how `step` grouping interacts with the
  window boundary (drop into the middle of a step's bubbles
  looks odd).
- Streaming a fresh chunk into an old turn that scrolled out of
  the window is a non-issue (chunks always land on the most
  recent assistant entry), but we should assert it.

### 3.3 Incremental state-update (diff over the wire)

**Idea.** Instead of "every state-update carries the whole
transcript," the host sends deltas:

```ts
| { type: "transcript-append"; entry: ChatTranscriptEntry }
| { type: "transcript-update"; id: string; patch: Partial<Entry> }
| { type: "transcript-replace"; entries: ChatTranscriptEntry[] } // initial load only
```

A streaming chunk becomes ONE `transcript-update` with
`{ body: entry.body + delta }`. The webview keeps its own
mirror of the transcript and applies the patch in place — no
need to rebuild the whole tree.

**Pros.**
- Per-chunk cost is O(1). Even at 10k entries the IPC channel
  carries ~50 bytes per delta.
- Lets the webview's DOM-update path be surgical: find the row
  for `id`, mutate its `body` text node directly. No morphdom
  involved.
- Composable with windowing (3.2): widening the window can be
  a `transcript-replace` for the older slice.

**Cons.**
- More moving parts. Host has to track "what did I last send"
  per session to compute the diff. Webview needs an
  apply-patch routine.
- Easy to drift: if a patch is dropped or applied out of order,
  the two views diverge silently. Need a sequence counter and a
  resync path ("if I see a gap, ask for a `transcript-replace`").
- Doesn't solve B by itself — the host's in-memory transcript
  and workspaceState are still unbounded.

### 3.4 Bubble-body memoization in the renderer

**Idea.** `bubbleDetails(...)` parses markdown every render.
Cache by `(entry.id, entry.body.length, entry.streaming)` so
unchanged bubbles return their previously-built DOM subtree
verbatim. morphdom would then skip the entire subtree via the
existing `isEqualNode` short-circuit (which currently still
walks because the subtree was freshly built).

**Pros.**
- Targeted fix; small change.
- Doesn't require any protocol changes.

**Cons.**
- Doesn't solve IPC cost (host still ships the full transcript).
- `isEqualNode` cost stays — it short-circuits faster on a
  reused subtree, but still O(N) walk to discover that.
- Only addresses markdown parsing, which may not be the
  dominant cost (DOM construction itself is).

### 3.5 Rope buffer

**Not a fit.** Rope buffers help when you have **one** big string
that needs efficient mid-insertion and slicing (text editors).
The chat panel has a list of independent bubbles, not one big
string. The bubble list itself is already cheap to append to
(array push). The bottleneck is the DOM render, not the data
structure.

A rope would help if we instead modeled the transcript as one
serialized markdown buffer that the webview parses on every
update — but that's a worse design than the current bubble list.
Mentioning here so it's explicitly off the table.

### 3.6 VS Code-provided alternatives

Surveyed for completeness:

- `vscode.window.createWebviewView` — what we already use; no
  built-in transcript widget.
- `vscode.window.createOutputChannel` — append-only plain text;
  no rich rendering, no interaction. Not a fit for the chat
  panel UX.
- `vscode.window.createTreeView` — built-in virtualization but
  tree-shaped, not bubble-shaped. Would let us bolt a
  collapsible "by step / by turn" hierarchy on top of the
  transcript without writing virtualization ourselves, but
  we'd lose the rich-markdown body rendering.
- VS Code's own Copilot chat panel is an internal surface, not
  exposed to extensions.
- The `vscode-elements` library wraps the design system but
  doesn't ship a virtualized list.

Verdict: **no built-in solves this for our shape**. We'd have to
ship our own virtualization OR window the data we send.

### 3.7 Trim the orchestrator's chatter

Orthogonal angle: do we actually need to render every
`llm-request` bubble?

Today the panel surfaces every prompt-stack message the
orchestrator added per turn — system prompt, write-scope rules,
framework API TOC, step inputs, the user-role prompt for the
turn. Most of this is **the same content turn after turn**. A
20-minute session re-rendered the standing system prompt ~2k
times.

Options:
- Only emit `llm-request` for messages that **changed since the
  last turn** (orchestrator-side compaction signal). The chat
  panel still shows it once per turn-where-it-changed; the
  default case becomes "no llm-request bubbles."
- Collapse repeat system messages into a single "system stack
  (unchanged)" badge at the top of each turn, expandable.
- Add a panel-side filter that hides system/tool role bubbles
  by default (the user can toggle them on).

This is the cheapest fix (no virtualization needed) but it's a
behavior change the user needs to opt into.

## 4. Recommendation

Tentative ranking, biggest-bang-per-effort first. **Not a
decided plan** — picking which to ship needs a sit-down.

1. **(3.7) Trim the orchestrator's prompt-stack repetition.**
   The 2k×system-prompt situation is the root cause of the
   blow-up. Even crude dedup ("only emit `llm-request` for
   messages whose `messageId` is new this session") cuts the
   bubble count by 10×–50×. No webview changes needed.

2. **(3.2) Host-side windowing of `transcript`.** Ship the last
   N=200 entries by default, with a "Show earlier" stub.
   Solves both A and B in a single PR. Webview render path is
   unchanged. Combine with (3.7) and most users never hit the
   window boundary.

3. **(3.3) Incremental state-update protocol.** Once
   `transcript` is windowed, the per-chunk re-send of N entries
   is still wasteful. A surgical `transcript-update` for chunk
   deltas drops per-token IPC cost to near-zero. Build this on
   top of (3.2) so the initial-load message can still be a
   `transcript-replace`.

4. **(3.1) True virtualization.** Only worth doing if (2)+(3)
   aren't enough — i.e., if users routinely want to scroll
   through thousands of turns of history. For our flow this is
   unlikely; turns are bounded by the step count and the
   in-step iteration cap.

5. **(3.4) Bubble memoization.** Last-resort optimization;
   only matters if markdown parsing turns out to be a hot
   spot inside (2)+(3). Easy to measure with the perf panel
   once we have it.

## 5. Open questions

- **What window size?** 200 entries feels right for our shape
  (one step ≈ 20–40 entries × 5–10 visible recent steps), but
  needs measuring. Should it be configurable?
- **Step-boundary alignment.** Should the window snap to step
  boundaries (always include all bubbles for the current step,
  even if that pushes the total over the cap)?
- **What does "Show earlier" do mechanically?** Widen the window
  in-place vs. open a separate read-only history view?
- **Persistence cap.** Do we also trim `workspaceState`? If a
  user opens the panel after a 4-hour run, do we load 4 hours of
  history or just the last N? (workspaceState write cost itself
  may be a separate issue — the debounced persist writes the
  whole conversation on each 250ms tick.)
- **Search.** If we window, Cmd-F only finds what's in the DOM.
  Do we need a panel-side "search transcript" affordance?
- **Eviction badges.** `ContextEvictedReason` markers paint on
  bubbles that scrolled out of the orchestrator's prompt stack.
  These should still render correctly when their bubble is
  outside the window — host needs to either retain them in the
  window or drop the badge silently. Pick one.

## 6. Non-goals for this round

- Cross-session search (a transcript browser across many
  projects). Out of scope.
- Replacing markdown rendering. Current `markdown-it`-based
  path is fine; the issue isn't parsing speed at small N.
- Reworking persistence to SQLite / on-disk JSONL. Memento +
  windowing is enough; on-disk only matters if we hit
  Memento size limits.

## 7. Next step

Pick one of {3.7, 3.2, 3.3} as the first concrete piece of
work, write a plan doc with the protocol-level details, and
prototype on a branch. My pick would be **(3.7) first** (cheapest,
hits the actual root cause), then **(3.2)** as the structural
fix, then **(3.3)** if measurement shows we still need it.
