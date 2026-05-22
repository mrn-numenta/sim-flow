import { describe, expect, test } from "vitest";

import { CHAT_PALETTE_NAMES, DEFAULT_CUSTOM_PALETTE } from "./messages";

describe("chatPanel/messages constants", () => {
  test("CHAT_PALETTE_NAMES enumerates exactly the built-ins plus custom", () => {
    // Order matters -- the chat panel iterates this list to render
    // the picker so a reorder would visibly shuffle the UI.
    expect([...CHAT_PALETTE_NAMES]).toEqual(["default", "autumn", "olive", "sage", "custom"]);
  });

  test("DEFAULT_CUSTOM_PALETTE seeds the custom picker with hex colors", () => {
    expect(DEFAULT_CUSTOM_PALETTE).toMatchObject({
      input: expect.stringMatching(/^#[0-9a-fA-F]{6}$/),
      tool: expect.stringMatching(/^#[0-9a-fA-F]{6}$/),
      output: expect.stringMatching(/^#[0-9a-fA-F]{6}$/),
      accent: expect.stringMatching(/^#[0-9a-fA-F]{6}$/),
    });
  });
});
