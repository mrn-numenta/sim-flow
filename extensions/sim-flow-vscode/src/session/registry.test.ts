import { describe, expect, it, vi } from "vitest";

import { freshSessionKey, SessionRegistry } from "./registry";
import type { SessionPump } from "./pump";

/**
 * The registry stores `SessionPump` instances but only ever calls
 * `.dispose()` on them, so a minimal duck-typed stub is enough for
 * unit tests. Cast through `unknown` to placate the type checker.
 */
function stubPump(): SessionPump {
  const dispose = vi.fn();
  return { dispose } as unknown as SessionPump;
}

describe("SessionRegistry", () => {
  it("starts empty: has() and get() return falsy values", () => {
    const reg = new SessionRegistry();
    expect(reg.has("missing")).toBe(false);
    expect(reg.get("missing")).toBeUndefined();
  });

  it("insert -> has -> get round-trip", () => {
    const reg = new SessionRegistry();
    const pump = stubPump();
    reg.insert("k", pump);
    expect(reg.has("k")).toBe(true);
    expect(reg.get("k")).toBe(pump);
  });

  it("insert with the same key replaces the prior pump (no auto-dispose)", () => {
    // The registry only calls dispose on `remove`. An overwrite via
    // `insert` leaves the prior pump's dispose to the caller; the
    // chat participant relies on this when it deliberately swaps a
    // dead pump for a fresh one.
    const reg = new SessionRegistry();
    const a = stubPump();
    const b = stubPump();
    reg.insert("k", a);
    reg.insert("k", b);
    expect(reg.get("k")).toBe(b);
    expect(a.dispose).not.toHaveBeenCalled();
    expect(b.dispose).not.toHaveBeenCalled();
  });

  it("remove() disposes the entry and clears the slot", () => {
    const reg = new SessionRegistry();
    const pump = stubPump();
    reg.insert("k", pump);
    reg.remove("k");
    expect(pump.dispose).toHaveBeenCalledTimes(1);
    expect(reg.has("k")).toBe(false);
    expect(reg.get("k")).toBeUndefined();
  });

  it("remove() on an unknown key is a no-op", () => {
    const reg = new SessionRegistry();
    expect(() => reg.remove("nope")).not.toThrow();
  });

  it("disposeAll() disposes every entry and clears the map", () => {
    const reg = new SessionRegistry();
    const a = stubPump();
    const b = stubPump();
    const c = stubPump();
    reg.insert("a", a);
    reg.insert("b", b);
    reg.insert("c", c);
    reg.disposeAll();
    expect(a.dispose).toHaveBeenCalledTimes(1);
    expect(b.dispose).toHaveBeenCalledTimes(1);
    expect(c.dispose).toHaveBeenCalledTimes(1);
    expect(reg.has("a")).toBe(false);
    expect(reg.has("b")).toBe(false);
    expect(reg.has("c")).toBe(false);
  });
});

describe("freshSessionKey", () => {
  it("produces strings with the `s-` prefix", () => {
    const k = freshSessionKey();
    expect(k.startsWith("s-")).toBe(true);
  });

  it("produces unique values across rapid calls", () => {
    const seen = new Set<string>();
    for (let i = 0; i < 100; i++) {
      seen.add(freshSessionKey());
    }
    expect(seen.size).toBe(100);
  });
});
