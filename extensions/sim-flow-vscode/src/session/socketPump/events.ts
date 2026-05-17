/**
 * Protocol-event dispatcher for `SocketSessionPump`. The pump's
 * `handleEvent` switch-case lives here so the main class file stays
 * under the 1000-line refactor threshold without altering behavior:
 * `handleEvent(ctx, event)` reads/mutates the pump through the
 * `EventDispatchContext` interface, which the class implements
 * structurally.
 */
import type EventEmitter from "node:events";
import type { Event as ProtocolEvent } from "../protocol-types";
import type { PumpLlmConfig, PumpRenderer, PumpSettleResult } from "../pump";
import type { SessionTag, StepDescriptorOut, StepMode } from "../protocol-types";
import { renderBuildOutput } from "../buildOutput";

/**
 * Internal bus-event shape; emitted from `handleEvent` and consumed by
 * the pump's `onXxx` subscriber methods. Mirrored across both
 * `socketPump.ts` and this module so the union stays a single
 * authority -- the main class re-exports it via type-only alias.
 */
export type SocketPumpBusEvent =
  | { type: "settled"; result: PumpSettleResult }
  | { type: "step-mode"; mode: StepMode }
  | { type: "in-sub-session"; inSubSession: boolean }
  | {
      type: "gate-result";
      step: string;
      clean: boolean;
      failures: { description: string; reason: string }[];
    }
  | {
      // Orchestrator parked the sub-session and is asking for human
      // guidance. The `prompt` field is the text to show above the
      // composer (the question itself, or operator instructions
      // like "/retry or /end-session"). `placeholder` hints what
      // shape of reply is expected and goes inside the textarea.
      // Both are optional in the wire format; if absent, the chat
      // panel falls back to its generic "awaiting input" notice.
      type: "request-user-input";
      prompt: string | null;
      placeholder: string | null;
    }
  | {
      // Orchestrator suggested an actionable quick-reply. `label`
      // is the button text; `action` is the literal string the
      // host should ship as a `UserMessage` when the user clicks.
      // Typically emitted in clusters just before a
      // `request-user-input` (the orchestrator emits one per
      // action, then parks). The chat panel accumulates these
      // until a UserMessage ships or a new sub-session opens.
      type: "followup";
      label: string;
      action: string;
    }
  | {
      // Orchestrator's hint about what `ContinueFlow` would do
      // next. `label` is pre-rendered for the chat panel's
      // Continue button ("Run critique on DM2d"); null means "no
      // action available, render Continue as disabled."
      type: "next-action-hint";
      label: string | null;
    }
  | {
      // Prompt-stack compaction signal: the listed message ids
      // are no longer carried in the orchestrator's working set.
      // The chat panel marks the matching transcript rows with a
      // "no longer in context" indicator; transcript content
      // itself is preserved verbatim.
      type: "context-evicted";
      ids: string[];
      reason: import("../protocol-types").ContextEvictionReason;
    };

/**
 * Mutable state + collaborators `handleEvent` needs. `SocketSessionPump`
 * implements this structurally (its private fields are visible to its
 * own methods); we shape it as an interface so the dispatcher stays
 * decoupled from the class.
 */
export interface EventDispatchContext {
  readonly bus: EventEmitter;
  readonly llm: PumpLlmConfig;
  currentRenderer: PumpRenderer | null;
  sessionTag: SessionTag | null;
  stepDescriptor: StepDescriptorOut | null;
  currentStepMode: StepMode | null;
  inSubSessionFlag: boolean;
  awaitingUserInputFlag: boolean;
  /** Derived view: `inSubSessionFlag && !awaitingUserInputFlag`. */
  readonly inSubSession: boolean;
  markTerminated(result: PumpSettleResult): void;
}

export function handleEvent(ctx: EventDispatchContext, event: ProtocolEvent): void {
  const wasBusy = ctx.inSubSession;
  // Update the parking flag BEFORE dispatching the per-event
  // case. The bracket flag (`inSubSessionFlag`) is the orchestrator's
  // server-side truth; the parking flag (`awaitingUserInputFlag`)
  // is our derivation: `request-user-input` parks, any other
  // active event resumes. Bracket transitions and pure-state
  // events (step-mode echo, hello-ack) don't change the parking
  // signal; they're handled inline in their cases below.
  if (event.event === "request-user-input") {
    ctx.awaitingUserInputFlag = true;
  } else if (
    event.event !== "step-mode-changed" &&
    event.event !== "hello-ack" &&
    event.event !== "sub-session-started" &&
    event.event !== "sub-session-ended" &&
    event.event !== "session-end"
  ) {
    ctx.awaitingUserInputFlag = false;
  }
  switch (event.event) {
    case "hello-ack":
      ctx.sessionTag = event.session;
      ctx.stepDescriptor = event.step_descriptor;
      renderHelloAck(ctx, event);
      break;
    case "assistant-text": {
      const toolCalls = (event.tool_calls ?? []).map((c) => ({
        id: c.id ?? undefined,
        name: c.name,
        argumentsJson: c.arguments_json,
      }));
      // Prefer the structured `assistantTurn` renderer hook when
      // available -- it carries both the prose AND the tool calls
      // the LLM emitted, so experimental hosts can render a
      // complete record even on tool-only turns (text=""). Fall
      // back to `markdown(text)` for the legacy chat-participant
      // path, which has no concept of tool calls.
      if (ctx.currentRenderer?.assistantTurn) {
        ctx.currentRenderer.assistantTurn({
          text: event.text,
          finalChunk: event.final_chunk,
          toolCalls,
        });
      } else if (event.text.length > 0) {
        ctx.currentRenderer?.markdown(event.text);
      }
      break;
    }
    case "llm-request":
      // Experimental: surface every non-Assistant message the
      // orchestrator added to the prompt stack so hosts can render
      // the running prompt+response transcript. Renderers that
      // don't implement `llmRequest` (e.g. VS Code chat
      // participant) silently ignore.
      ctx.currentRenderer?.llmRequest?.({
        role: event.role,
        content: event.content,
        turnIndex: event.turn_index,
        requestId: event.request_id,
      });
      break;
    case "request-user-input":
      // Surface prompt + placeholder to subscribers before the
      // settle so the chat panel can paint the banner BEFORE
      // flipping into the awaiting-input state. Both fields are
      // optional in the wire format; the chat panel renders the
      // generic "Waiting on user" notice when prompt is null.
      ctx.bus.emit("msg", {
        type: "request-user-input",
        prompt: event.prompt ?? null,
        placeholder: event.placeholder ?? null,
      } as SocketPumpBusEvent);
      ctx.bus.emit("msg", {
        type: "settled",
        result: { status: "awaiting-input" },
      } as SocketPumpBusEvent);
      break;
    case "artifact-written":
      ctx.currentRenderer?.markdown(`\n_Wrote \`${event.path}\` (${event.bytes} bytes)._\n`);
      break;
    case "tool-invoked":
      ctx.currentRenderer?.markdown(
        `\n_Tool \`${event.name}\` ${event.args_summary ? `(${event.args_summary}) ` : ""}-> ${event.status} (${event.duration_ms} ms)._\n`,
      );
      break;
    case "phase-changed":
      ctx.currentRenderer?.markdown(`\n**Phase:** \`${event.phase}\`\n`);
      break;
    case "build-output":
      ctx.currentRenderer?.markdown(renderBuildOutput(event));
      break;
    case "gate-result":
      if (event.clean) {
        ctx.currentRenderer?.markdown(`\n**Gate \`${event.step}\`: clean.**\n`);
      } else {
        const lines = event.failures.map((f) => `- ${f.description}: ${f.reason}`).join("\n");
        ctx.currentRenderer?.markdown(
          `\n**Gate \`${event.step}\`: ${event.failures.length} failure(s).**\n\n${lines}\n`,
        );
      }
      // Bus event so the dashboard's `gate-result` HostMessage path
      // gets the structured result. Without this, manual-mode JSONL
      // gate clicks only land in the chat panel as markdown -- the
      // per-step "Run Gate" pending action stays "..." until the 5s
      // failsafe, the gate cache never updates, and downstream
      // buttons (Advance) don't react.
      ctx.bus.emit("msg", {
        type: "gate-result",
        step: event.step,
        clean: event.clean,
        failures: event.failures.map((f) => ({
          description: f.description,
          reason: f.reason,
        })),
      } as SocketPumpBusEvent);
      break;
    case "state-advanced":
      ctx.currentRenderer?.markdown(
        `\n**Advanced past \`${event.from}\`${event.to ? `; current step is now \`${event.to}\`.` : ` (final step in this flow).`}**\n`,
      );
      break;
    case "followup":
      // Still surface the suggestion in the transcript for any
      // text-only renderer (the chat panel will additionally
      // surface a clickable chip via the bus event below).
      ctx.currentRenderer?.markdown(
        `\n_Suggested next: ${event.label} (\`${event.action}\`)._\n`,
      );
      ctx.bus.emit("msg", {
        type: "followup",
        label: event.label,
        action: event.action,
      } as SocketPumpBusEvent);
      break;
    case "next-action-hint":
      ctx.bus.emit("msg", {
        type: "next-action-hint",
        label: event.label ?? null,
      } as SocketPumpBusEvent);
      break;
    case "context-evicted":
      ctx.bus.emit("msg", {
        type: "context-evicted",
        ids: event.ids,
        reason: event.reason,
      } as SocketPumpBusEvent);
      break;
    case "diagnostic":
      renderDiagnostic(ctx, event.level, event.message);
      break;
    case "session-end":
      ctx.markTerminated({
        status: "ended",
        endReason: event.reason,
        endMessage: event.message ?? undefined,
      });
      break;
    case "step-mode-changed":
      // Track the orchestrator's truth and notify subscribers (the
      // dashboard's toggle UI listens via `onStepModeChanged`). The
      // event also fires at session start as the orchestrator
      // echoes the initial `--step-mode` flag, so the toggle
      // matches reality before the user touches anything.
      ctx.currentStepMode = event.mode;
      ctx.bus.emit("msg", {
        type: "step-mode",
        mode: event.mode,
      } as SocketPumpBusEvent);
      break;
    case "sub-session-started":
      // Bracket open. A fresh sub-session is by definition not
      // parked, so clear the parking flag too (covers the case
      // where the previous sub-session ended while parked and a
      // new one starts before any active-work event).
      ctx.inSubSessionFlag = true;
      ctx.awaitingUserInputFlag = false;
      break;
    case "sub-session-ended":
      // Bracket close. Dashboard listeners re-enable per-step
      // buttons (subject to disk-state preconditions).
      ctx.inSubSessionFlag = false;
      ctx.awaitingUserInputFlag = false;
      break;
    default: {
      const exhaustive: never = event;
      void exhaustive;
    }
  }
  // Single emission point for the effective busy signal. Events
  // that don't change the bracket / parking pair are no-ops here
  // (`wasBusy === isBusy`); everything that does -- bracket open
  // and close, parking on `request-user-input`, resuming on the
  // next active-work event -- fires exactly once per transition.
  const isBusy = ctx.inSubSession;
  if (isBusy !== wasBusy) {
    ctx.bus.emit("msg", {
      type: "in-sub-session",
      inSubSession: isBusy,
    } as SocketPumpBusEvent);
  }
}

function renderHelloAck(
  ctx: EventDispatchContext,
  event: ProtocolEvent & { event: "hello-ack" },
): void {
  const banner = [
    `**Step \`${event.session.step}\` ${event.session.kind} session**`,
    event.session.candidate ? `(candidate \`${event.session.candidate}\`)` : null,
    `— sim-flow ${event.sim_flow_version}; protocol v${event.protocol_version}; backend \`${ctx.llm.source}\`${ctx.llm.model ? ` (\`${ctx.llm.model}\`)` : ""}.`,
  ]
    .filter(Boolean)
    .join(" ");
  ctx.currentRenderer?.markdown(`${banner}\n\n`);
  if (event.step_descriptor.phases.length > 0) {
    ctx.currentRenderer?.markdown(
      `_Phases:_ ${event.step_descriptor.phases.map((p) => `\`${p}\``).join(" -> ")}\n\n`,
    );
  }
}

function renderDiagnostic(ctx: EventDispatchContext, level: string, message: string): void {
  const tag = level === "error" ? "**Error**" : level === "warning" ? "**Warning**" : "**Info**";
  ctx.currentRenderer?.markdown(`\n${tag}: ${message}\n`);
}
