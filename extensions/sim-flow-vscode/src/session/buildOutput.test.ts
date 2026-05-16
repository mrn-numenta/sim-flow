import { describe, expect, test } from "vitest";

import { renderBuildOutput } from "./buildOutput";

describe("renderBuildOutput", () => {
  test("exit 0 renders a terse one-line summary", () => {
    const out = renderBuildOutput({
      command: "cargo check",
      exit_code: 0,
      stdout_tail: "Finished dev profile",
      stderr_tail: "",
    });
    // Should mention the command and the exit status.
    expect(out).toContain("`cargo check`");
    expect(out).toContain("status `0`");
    // On success the tails are NOT echoed -- they're noise.
    expect(out).not.toContain("Finished dev profile");
    expect(out).not.toContain("```text");
  });

  test("non-zero exit shows stdout tail in a fenced block", () => {
    const out = renderBuildOutput({
      command: "cargo test",
      exit_code: 101,
      stdout_tail: "test result: FAILED",
      stderr_tail: "",
    });
    expect(out).toContain("status `101`");
    expect(out).toContain("stdout (tail):");
    expect(out).toContain("```text");
    expect(out).toContain("test result: FAILED");
  });

  test("non-zero exit shows stderr tail in a fenced block", () => {
    const out = renderBuildOutput({
      command: "cargo build",
      exit_code: 1,
      stdout_tail: "",
      stderr_tail: "error[E0433]: failed to resolve",
    });
    expect(out).toContain("stderr (tail):");
    expect(out).toContain("error[E0433]");
  });

  test("non-zero exit with both tails surfaces both fenced blocks", () => {
    const out = renderBuildOutput({
      command: "cargo clippy",
      exit_code: 1,
      stdout_tail: "Finished dev profile",
      stderr_tail: "error: unused variable",
    });
    expect(out).toContain("stdout (tail):");
    expect(out).toContain("Finished dev profile");
    expect(out).toContain("stderr (tail):");
    expect(out).toContain("error: unused variable");
  });

  test("non-zero exit with no captured output reports it explicitly", () => {
    const out = renderBuildOutput({
      command: "cargo run",
      exit_code: -1,
      stdout_tail: "",
      stderr_tail: "",
    });
    expect(out).toContain("status `-1`");
    expect(out).toContain("_(no output captured)_");
  });

  test("trims whitespace-only tails so they're treated as empty", () => {
    // Whitespace-only tails should hit the "no output captured"
    // branch on non-zero exits, not surface an empty fenced block.
    const out = renderBuildOutput({
      command: "cargo test",
      exit_code: 1,
      stdout_tail: "   \n  ",
      stderr_tail: "\t\n",
    });
    expect(out).toContain("_(no output captured)_");
    expect(out).not.toContain("```text");
  });

  test("exit-0 path always starts with a leading blank line", () => {
    // The orchestrator inlines this output between other markdown;
    // a leading newline keeps the block from running into the
    // previous content. Same shape for both branches.
    const ok = renderBuildOutput({
      command: "cargo check",
      exit_code: 0,
      stdout_tail: "",
      stderr_tail: "",
    });
    expect(ok.startsWith("\n")).toBe(true);
    const fail = renderBuildOutput({
      command: "cargo check",
      exit_code: 1,
      stdout_tail: "",
      stderr_tail: "",
    });
    expect(fail.startsWith("\n")).toBe(true);
  });
});
