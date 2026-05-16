# Chat-panel bug audit — 2026-05-16

Findings from a read-only audit of the sim-flow VS Code extension's chat
panel code (`tools/sim-flow/extensions/sim-flow-vscode/src/chatPanel/**`
+ `media/chat-panel-experimental.css`). Scope: bugs and correctness
risks only — style / refactor suggestions are out.

Findings are ranked by severity. The top three to fix first are **#1**
(functional regression), **#3** (reproducible composer bug), **#2**
(listener leak with a misleading comment).

---

## Definite bugs

### #1 — Experimental panel drops orchestrator prompts + followups

**File:** [`src/chatPanel/panelExperimental.ts`](../../extensions/sim-flow-vscode/src/chatPanel/panelExperimental.ts)
**Confidence:** definitely a bug

The experimental UI consumes only `state.currentPlaceholder` (around
the textarea placeholder, line ~1404). It never surfaces:

- `state.currentPrompt` — the orchestrator's parked question
- `state.pendingFollowups` — clickable quick-reply chips
- `state.notice` — banner explanation
- `state.idleQaHint` — side-conversation hint
- `state.awaitingUserInput` — "waiting on user" status

The host correctly wires `onRequestUserInput` + `onFollowup` listeners
and populates the state; the webview just throws everything except the
placeholder on the floor. The standard `panel.ts` does render the full
set (lines 256-278, 488-504), so this is an experimental-UI-only
regression.

**Symptom:** When the orchestrator parks at `request-user-input` (DM0
clarification questions, refused-advance prompts, LlmError offering
`/retry` vs `/end-session`), the user sees only the textarea
placeholder — no banner with the question, no chips for the followups.

---

### #2 — `onActiveSessionChanged` subscription is leaked

**File:** [`src/chatPanel/host.ts:238-254`](../../extensions/sim-flow-vscode/src/chatPanel/host.ts#L238-L254)
**Confidence:** definitely a bug

The constructor calls `this.autoSessions.onActiveSessionChanged(...)`,
which returns a disposer. The return value is discarded. The comment
on line 236 says "The disposer is stored so the previous pump's
listener doesn't outlive its session" — but that refers to the inner
sub-session / followup / request-user-input listeners attached inside
the callback, *not* this outer registration.

**Symptom:** After `ChatPanelProvider.dispose()` the closure on lines
238-254 keeps firing on every active-session change, calling
`this.attachSubSessionListener(...)` etc. on the disposed provider.
The disposed provider stays GC-pinned via the closure. In typical
extension lifecycles `autoSessions` is disposed at the same time, so
the leak doesn't cascade, but it is a textbook leak with a misleading
comment.

---

### #3 — Composer textarea doesn't clear after Enter-to-send

**File:** [`src/chatPanel/panelExperimental.ts`](../../extensions/sim-flow-vscode/src/chatPanel/panelExperimental.ts) (`submitPrompt()` at ~1615-1625, morphdom hook at ~445-449)
**Confidence:** definitely a bug

When the user presses Enter to submit (the most common send gesture),
the textarea has focus. `submitPrompt()` sets `ui.draft = ""` and
calls `render()`. Render builds a new textarea with `area.value = ""`,
but the morphdom `onBeforeElUpdated` hook returns `false` for focused
textareas — skipping the update entirely. The live textarea keeps the
just-submitted text visible.

**Symptom chain:**

1. User types a prompt and presses Enter. Text is sent.
2. `ui.draft` becomes `""` but the textarea still visibly shows the
   sent text (focus skip).
3. Pressing Enter again: nothing sends (`canSend` returns false because
   `ui.draft.trim().length === 0`).
4. Typing more characters: the `input` handler reads `area.value` (still
   carrying the old prompt + the new characters) into `ui.draft`.
5. The next send ships the OLD prompt concatenated with the new content.

**Fix:** explicitly clear `area.value = ""` after `send()` regardless of
focus, or update the live textarea directly before calling `render()`.

---

## Likely bugs

### #4 — DOMPurify keeps `style` attribute on assistant output

**Files:** [`src/chatPanel/host.ts:2654-2663`](../../extensions/sim-flow-vscode/src/chatPanel/host.ts#L2654-L2663), [`src/chatPanel/renderMarkdown.ts:67`](../../extensions/sim-flow-vscode/src/chatPanel/renderMarkdown.ts#L67)
**Confidence:** likely a bug (security)

CSP includes `style-src ${webview.cspSource} 'unsafe-inline'` and
DOMPurify's `ALLOWED_ATTR` includes `"style"`. Adversarial LLM
markdown can inject arbitrary CSS — e.g.
`position: fixed; top: 0; left: 0; width: 100%; height: 100%; background: black;`
to overlay the panel, or repaint disabled buttons as enabled. DOMPurify
scrubs `javascript:` and `expression()` but permits everything else.

CSS-only clickjacking is constrained inside the webview iframe (no
auth tokens to steal) but can still occlude the composer or mislead
the user into actions.

The comment claims `style` is needed for Shiki spans, but Shiki output
is grafted in *after* DOMPurify by `applyShikiHighlight`
(`renderMarkdown.ts:188-211`), so the markdown→sanitize pipeline does
not actually need `style` for assistant text. Recommend dropping
`style` from `ALLOWED_ATTR`.

---

### #5 — Shiki HTML bypasses sanitization

**File:** [`src/chatPanel/renderMarkdown.ts:188-211`](../../extensions/sim-flow-vscode/src/chatPanel/renderMarkdown.ts#L188-L211)
**Confidence:** likely a bug (security)

Shiki's `codeToHtml(src, ...)` output is fed into `tpl.innerHTML = html`
and then `pre.replaceWith(replacement)` — bypassing DOMPurify entirely.
Shiki itself escapes its inputs, so a properly-functioning Shiki is
safe. But the post-sanitization injection point means any future
Shiki regression (or a bundled-language plugin with HTML injection)
would propagate straight into the live DOM. The order is "sanitize
first, then re-inject untrusted Shiki output", which is an anti-pattern.

**Severity:** lower than #4 because Shiki has a clean track record,
but tightening this would close the loop.

---

### #6 — Custom-palette pickers can fight themselves on rapid drag

**File:** [`src/chatPanel/panelExperimental.ts:231-251`](../../extensions/sim-flow-vscode/src/chatPanel/panelExperimental.ts#L231-L251)
**Confidence:** likely a bug

The picker `input` handler calls `pushPaletteToHost()` on every
change. The host persists asynchronously via `workspaceState.update`.
A `refresh()` that fires for unrelated reasons (config change,
file-watcher tick) calls `readSavedCustomPalette()` which reads
workspaceState — which may not yet contain the latest picker value
if the prior `update` is still pending. The state-update arrives at
the webview with the OLD palette; `syncPaletteFromHost` sees a "diff"
and overwrites the user's most recent picker selection.

**Symptom:** Dragging a colour picker rapidly, a colour briefly snaps
back to a previous value or the value before the user's most recent
drag. The picker "fights" itself.

---

### #9 — Path traversal via transcript-supplied paths

**File:** [`src/chatPanel/host.ts:591-620`](../../extensions/sim-flow-vscode/src/chatPanel/host.ts#L591-L620)
**Confidence:** likely a bug (security)

`openFileInEditor` accepts any path the webview sends. Paths come
from the file-path linkifier scanning untrusted LLM output. For
relative paths the host builds `${anchor}/${path}` with no
normalization and no traversal guard. An LLM emitting
`` `../../../etc/passwd` `` makes a clickable link that opens that
file on click.

Limited harm (no write, no exec, user-initiated click), but a
deliberately hostile transcript can deceive the user into opening
sensitive files. On Windows the unconditional `${anchor}/${path}`
also misses UNC paths (`\\server\share`); `vscode.Uri.file` handles
forward slashes but the regex on line 599 doesn't gate on UNC.

---

## Suspicious — needs verification

### #7 — Two concurrent pumps possible on cold start

**File:** [`src/chatPanel/host.ts:467-475, 1215-1232, 2288-2345`](../../extensions/sim-flow-vscode/src/chatPanel/host.ts)
**Confidence:** suspicious

On the webview's first `"ready"` message: `await this.refresh()` runs
(which internally calls `restoreActiveAutoSessionIfNeeded`). Then
`void this.tryAutoResume()` fires-and-forgets a *second* launch path.
If the stored auto-session record exists (restore takes effect) AND
the `LAST_PROJECT_KEY` workspaceState entry points to the same project
(auto-resume condition), both paths can be in flight concurrently.
Restore's `await pump.ready()` is asynchronous; tryAutoResume's
`await launchAutoSession()` is asynchronous. The bail-out checks
(`if (this.activePump || this.pendingAutoLaunch) return`) only fire at
function entry — not after every `await`.

**Symptom:** two concurrent socket pumps for the same `state.toml`;
subsequent dashboard/chat events fight each other. Hard to reproduce
without exact timing but reachable on cold start.

---

### #8 — File-link click delegation might double-register

**File:** [`src/chatPanel/panelExperimental.ts:281-312`](../../extensions/sim-flow-vscode/src/chatPanel/panelExperimental.ts#L281-L312)
**Confidence:** suspicious

`installFileLinkDelegation()` adds two `document`-scoped listeners
(`click`, `keydown`) at module init (line 221). No guard against
double-install. When the user toggles `sim-flow.dashboard.experimentalUi`,
host.ts (line ~184) reassigns `webview.html`. *Usually* a `.html`
reassignment fully reloads the iframe with a fresh document and
listeners reset — but worth confirming with the VS Code webview spec
for this specific surface.

**Symptom (if the iframe doesn't reset):** clicking a file path in
the transcript opens the file twice.

---

### #10 — `convert-to-sv` reconnect doesn't force manual mode

**File:** [`src/chatPanel/host.ts:629-676`](../../extensions/sim-flow-vscode/src/chatPanel/host.ts#L629-L676) (`reconnectActivePump` at 2597-2616)
**Confidence:** suspicious

After `cli.convertSv()` flips the project, `reconnectActivePump` →
`launchAutoSession` runs *without* `forceStepMode: "manual"`. It
reads the current `sim-flow.flow.stepMode` config. If the user has
set this to "auto", the freshly-flipped SV0 session immediately runs
forward unattended.

**Symptom:** auto-mode users lose a beat of control after
convert-to-sv; the orchestrator powers ahead into SV0 work without a
pause.

---

### #11 — Token estimates double-count prompt-stack messages

**File:** [`src/chatPanel/state.ts`](../../extensions/sim-flow-vscode/src/chatPanel/state.ts) (`appendOrchestratorUserEntry` ~120-150)
**Confidence:** suspicious

Every orchestrator-emitted `llm-request` message is recorded as a
user-bubble with `requestTokensEstimate = estimateTextTokens(body)`.
`summarizeTokenEstimates` sums all `requestTokensEstimate` values
into the toolbar's "↑" total. The orchestrator re-emits the same
prompt-stack messages across turns (the prompt stack carries history
forward), so the same content is counted in the running total once
per turn it appears.

**Symptom:** the toolbar's ↑ counter overstates input tokens by a
factor proportional to conversation length. Per-bubble badges are
fine; the running total drifts up faster than actual LLM cost.

---

### #12 — `pendingConversationWrites` is module-scope shared

**File:** [`src/chatPanel/host.ts:79`](../../extensions/sim-flow-vscode/src/chatPanel/host.ts#L79) (`queueConversationWrite` at ~2847-2851)
**Confidence:** suspicious

`pendingConversationWrites` is a module-scope `Promise<void>` chain.
Multiple `ChatPanelProvider` instances (test scenarios, plugin host
scenarios) serialize through the same global; `waitForPendingConversationWrites()`
waits for unrelated writes from anywhere.

No user-visible bug today (only one provider in practice). Latent
foot-gun. Easy fix: instance field.

---

### #13 — Viewer-mode click sends `switch-project`

**File:** [`src/chatPanel/host.ts:465-549`](../../extensions/sim-flow-vscode/src/chatPanel/host.ts#L465-L549), [`src/chatPanel/panelExperimental.ts:583-590`](../../extensions/sim-flow-vscode/src/chatPanel/panelExperimental.ts#L583-L590)
**Confidence:** suspicious (UX)

When attached as a viewer (`state.isViewer === true`), the project
button click handler checks `state.sessionActive` to decide between
`switch-project` and `start-session`. A viewer IS `sessionActive`,
so the click sends `switch-project`, which tears down the viewer pump
and tries to launch a fresh driving session.

Probably what the user wants, but worth confirming against the
intended viewer-mode UX.

---

## Reviewed and cleared

### `<details>` preservation hook scope

The morphdom hook preserves open state on every `<details>` including
the settings popover. I traced this carefully and the interactions
with `ui.openPopup` (help, critique) are fine — no fight between the
hook and the explicit open/close state.

### Streaming-disabled textarea

`area.disabled = state.isStreaming || ...` locks the user out of
pre-typing while the LLM streams. This is intentional design, not a
bug. Reasonable people could argue otherwise but it's deliberate.

---

## Suggested fix order

1. **#1** — restore the prompt banner + followup chips in the
   experimental panel. Largest functional regression.
2. **#3** — clear `area.value` directly after `send()` so the
   morphdom focus-skip doesn't strand the sent text.
3. **#2** — capture the `onActiveSessionChanged` disposer in
   `disposables`. Trivial.
4. **#4 + #5** — drop `style` from `ALLOWED_ATTR` and move Shiki
   rendering ahead of DOMPurify (or inside it). Closes the
   adversarial-CSS surface.
5. **#9** — `path.normalize` + traversal guard in `openFileInEditor`.
6. **#6 + #11 + #10** — UX-grade fixes; pick up as the affected
   features become friction in real use.
