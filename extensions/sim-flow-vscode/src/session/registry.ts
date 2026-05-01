// Map<sessionKey, SessionPump> with TTL-style cleanup. The chat
// participant looks up a pump by sessionKey on every turn; pumps
// terminate themselves on `session-end` and the registry sweeps
// them out of the map.

import { SessionPump } from "./pump";

/**
 * Stable identifier for a chat session. Built from the chat tab's
 * sticky id when available, or fabricated per `/step` invocation
 * when not. Stored on `ChatResult.metadata` so subsequent turns can
 * recover it.
 */
export type SessionKey = string;

export class SessionRegistry {
  private pumps = new Map<SessionKey, SessionPump>();

  has(key: SessionKey): boolean {
    return this.pumps.has(key);
  }

  get(key: SessionKey): SessionPump | undefined {
    return this.pumps.get(key);
  }

  insert(key: SessionKey, pump: SessionPump): void {
    this.pumps.set(key, pump);
  }

  remove(key: SessionKey): void {
    const existing = this.pumps.get(key);
    this.pumps.delete(key);
    existing?.dispose();
  }

  /** Tear down every pump. Called from `deactivate()`. */
  disposeAll(): void {
    for (const pump of this.pumps.values()) {
      pump.dispose();
    }
    this.pumps.clear();
  }
}

/** Generate a fresh session key per `/step` invocation. */
export function freshSessionKey(): SessionKey {
  return `s-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}
