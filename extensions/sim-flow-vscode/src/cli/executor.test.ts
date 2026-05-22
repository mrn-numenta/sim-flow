import { describe, expect, test } from "vitest";

import { defaultExecute } from "./executor";

describe("defaultExecute", () => {
  test("captures stdout from a successful subprocess", async () => {
    // /bin/echo is universally available on macOS / Linux test runners.
    const { stdout, stderr } = await defaultExecute("/bin/echo", ["hello world"]);
    expect(stdout.trim()).toBe("hello world");
    expect(stderr).toBe("");
  });

  test("throws when the subprocess exits non-zero (carries the error's stderr/stdout)", async () => {
    // /bin/sh -c 'exit 1' fails with no output; the thrown error must
    // expose .code so caller wrappers (e.g. SimFlowCli.gate) can pull
    // the JSON payload from .stdout.
    await expect(defaultExecute("/bin/sh", ["-c", "exit 7"])).rejects.toMatchObject({
      code: 7,
    });
  });

  test("rejects when the binary doesn't exist", async () => {
    await expect(defaultExecute("/this/binary/does/not/exist", [])).rejects.toBeInstanceOf(Error);
  });
});
