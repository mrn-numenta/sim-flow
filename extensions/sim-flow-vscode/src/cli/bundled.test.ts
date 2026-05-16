import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  bundledCandidates,
  bundledFrameworkDocsRoot,
  bundledPdfiumLibPath,
  platformDir,
  setBundledRoot,
} from "./bundled";

afterEach(() => {
  // Reset between tests so they stay independent.
  setBundledRoot("");
});

describe("platformDir", () => {
  it("maps supported (platform, arch) combinations", () => {
    expect(platformDir("darwin", "arm64")).toBe("darwin-arm64");
    expect(platformDir("darwin", "x64")).toBe("darwin-x64");
    expect(platformDir("linux", "x64")).toBe("linux-x64");
    expect(platformDir("win32", "x64")).toBe("win32-x64");
  });

  it("returns null for platforms we do not ship a binary for", () => {
    expect(platformDir("linux", "arm64")).toBeNull();
    expect(platformDir("freebsd", "x64")).toBeNull();
    expect(platformDir("win32", "arm64")).toBeNull();
  });
});

describe("bundledCandidates", () => {
  it("returns an empty array when setBundledRoot has not been called", () => {
    setBundledRoot("");
    expect(bundledCandidates()).toEqual([]);
  });

  it("produces a single path under <root>/bin/<platform-dir>/ when the platform is supported", () => {
    setBundledRoot("/tmp/ext-root");
    const candidates = bundledCandidates();
    const mapped = platformDir(process.platform, process.arch);
    if (mapped === null) {
      expect(candidates).toEqual([]);
      return;
    }
    const exe = process.platform === "win32" ? "sim-flow.exe" : "sim-flow";
    expect(candidates).toEqual([`/tmp/ext-root/bin/${mapped}/${exe}`]);
  });
});

describe("bundledPdfiumLibPath / bundledFrameworkDocsRoot", () => {
  let tmpRoot: string;

  beforeEach(() => {
    tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), "sim-flow-bundled-"));
  });

  afterEach(() => {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
    setBundledRoot("");
  });

  it("bundledPdfiumLibPath returns undefined when the bundled root is unset", () => {
    setBundledRoot("");
    expect(bundledPdfiumLibPath()).toBeUndefined();
  });

  it("bundledPdfiumLibPath returns undefined when the library file is missing", () => {
    setBundledRoot(tmpRoot); // dir exists, but no bin/<dir>/libpdfium.*
    expect(bundledPdfiumLibPath()).toBeUndefined();
  });

  it("bundledPdfiumLibPath finds the platform-correct lib when present", () => {
    const mapped = platformDir(process.platform, process.arch);
    if (!mapped) {
      // Unsupported platform -- helper must short-circuit to undefined.
      setBundledRoot(tmpRoot);
      expect(bundledPdfiumLibPath()).toBeUndefined();
      return;
    }
    const libname =
      process.platform === "win32"
        ? "pdfium.dll"
        : process.platform === "darwin"
          ? "libpdfium.dylib"
          : "libpdfium.so";
    const dir = path.join(tmpRoot, "bin", mapped);
    fs.mkdirSync(dir, { recursive: true });
    const lib = path.join(dir, libname);
    fs.writeFileSync(lib, "");
    setBundledRoot(tmpRoot);
    expect(bundledPdfiumLibPath()).toBe(lib);
  });

  it("bundledFrameworkDocsRoot returns undefined when the root is unset", () => {
    setBundledRoot("");
    expect(bundledFrameworkDocsRoot()).toBeUndefined();
  });

  it("bundledFrameworkDocsRoot returns undefined when foundation-docs/api/toc.md is missing", () => {
    setBundledRoot(tmpRoot);
    expect(bundledFrameworkDocsRoot()).toBeUndefined();
  });

  it("bundledFrameworkDocsRoot returns the api dir when toc.md is present", () => {
    const apiDir = path.join(tmpRoot, "foundation-docs", "api");
    fs.mkdirSync(apiDir, { recursive: true });
    fs.writeFileSync(path.join(apiDir, "toc.md"), "# TOC\n");
    setBundledRoot(tmpRoot);
    expect(bundledFrameworkDocsRoot()).toBe(apiDir);
  });
});
