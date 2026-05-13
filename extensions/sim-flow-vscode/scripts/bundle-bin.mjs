// Stage the sim-flow binary + libpdfium for one platform under
// <ext>/bin/<platform-arch>/ so vsce includes them in the VSIX.
//
// Single-platform mode (the dev / `npm run package` default): pick
// the platform from `process.platform`+`process.arch`, source the
// binary from `target/release/sim-flow[.exe]`, source pdfium from
// `tools/sim-flow/vendor/pdfium/<map>/...`. This is what runs when
// you invoke the script with no env overrides.
//
// CI / cross-platform mode: the GitHub Actions release workflow
// builds sim-flow on each target runner (macOS arm64, ubuntu x64),
// uploads the artifacts, then runs THIS script repeatedly on the
// packaging job -- once per target -- with these env overrides:
//
//   SIM_FLOW_BUNDLE_TARGET   The platform-arch key whose bin/<key>/
//                            directory we're producing (e.g.
//                            `darwin-arm64`, `linux-x64`). When set,
//                            it overrides the current-process
//                            detection so a Linux runner can stage
//                            the macOS binary into bin/darwin-arm64/
//                            and vice versa.
//   SIM_FLOW_BUNDLE_BINARY   Absolute path to the sim-flow binary
//                            for that target. When unset we fall
//                            back to <repoRoot>/target/release/sim-flow[.exe]
//                            (the local-dev path).
//   SIM_FLOW_BUNDLE_PDFIUM   Absolute path to the libpdfium library
//                            for that target. When unset we fall
//                            back to the vendored copy under
//                            tools/sim-flow/vendor/pdfium/<map>/<libname>.
//
// The script is idempotent across invocations: each call writes to
// its own bin/<platform-arch>/ subdir, so running it twice with
// different SIM_FLOW_BUNDLE_TARGET values produces a multi-platform
// staging tree. API-doc staging is repeated each call but lands on
// the same dest, so it's a no-op after the first.
//
// Layout produced (must match `cli/bundled.ts::platformDir` and
// `bundledPdfiumLibPath`):
//
//   bin/<platform-arch>/sim-flow[.exe]
//   bin/<platform-arch>/libpdfium.{dylib,so} | pdfium.dll

import { copyFileSync, cpSync, existsSync, mkdirSync, statSync, chmodSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { extDir, repoRoot } from "./paths.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const packageRoot = process.argv[2] ? resolve(process.argv[2]) : extDir;
const apiDocsSourceDir = join(repoRoot, "target", "sim-flow-vscode-api-docs");
const apiDocsDestDir = join(packageRoot, "foundation-docs", "api");

// Resolve the staging-target platform. Priority:
//   1. `SIM_FLOW_BUNDLE_TARGET` env (CI cross-platform staging)
//   2. `process.platform` + `process.arch` (local dev)
function detectCurrentPlatformDir() {
  const platform = process.platform;
  const arch = process.arch;
  if (platform === "darwin" && arch === "arm64") return "darwin-arm64";
  if (platform === "darwin" && arch === "x64") return "darwin-x64";
  if (platform === "linux" && arch === "x64") return "linux-x64";
  if (platform === "linux" && arch === "arm64") return "linux-arm64";
  if (platform === "win32" && arch === "x64") return "win32-x64";
  return null;
}

const VALID_TARGETS = new Set([
  "darwin-arm64",
  "darwin-x64",
  "linux-x64",
  "linux-arm64",
  "win32-x64",
]);

const targetOverride = process.env.SIM_FLOW_BUNDLE_TARGET?.trim();
if (targetOverride && !VALID_TARGETS.has(targetOverride)) {
  console.error(
    `bundle-bin: SIM_FLOW_BUNDLE_TARGET=${targetOverride} is not in the supported list: ${[...VALID_TARGETS].join(", ")}.`,
  );
  process.exit(1);
}

const PLATFORM_DIR = targetOverride ?? detectCurrentPlatformDir();

// Map platform-arch -> pdfium-vendor key. These names differ because
// `tools/sim-flow/scripts/fetch-pdfium.mjs` uses the upstream pdfium
// distribution's naming convention, not Node's.
const PDFIUM_DIR_FOR = {
  "darwin-arm64": "macos-arm64",
  "darwin-x64": "macos-x64",
  "linux-x64": "linux-x64",
  "linux-arm64": "linux-arm64",
  "win32-x64": "windows-x64",
};
const PDFIUM_DIR = PLATFORM_DIR ? PDFIUM_DIR_FOR[PLATFORM_DIR] : null;

// Library filename also depends on the target platform, not the host.
const PDFIUM_LIBNAME_FOR = {
  "darwin-arm64": "libpdfium.dylib",
  "darwin-x64": "libpdfium.dylib",
  "linux-x64": "libpdfium.so",
  "linux-arm64": "libpdfium.so",
  "win32-x64": "pdfium.dll",
};
const PDFIUM_LIBNAME = PLATFORM_DIR ? PDFIUM_LIBNAME_FOR[PLATFORM_DIR] : null;

if (!PLATFORM_DIR || !PDFIUM_DIR) {
  console.warn(
    `bundle-bin: skipping; unsupported (platform, arch) = (${process.platform}, ${process.arch}). Set SIM_FLOW_BUNDLE_TARGET to one of: ${[...VALID_TARGETS].join(", ")}.`,
  );
  process.exit(0);
}

// Pick the exe name from the TARGET platform (the staging target's
// extension), not the host runner -- a Linux runner staging the
// Windows binary should name it `sim-flow.exe`.
const exe = PLATFORM_DIR === "win32-x64" ? "sim-flow.exe" : "sim-flow";
const sourceBin = process.env.SIM_FLOW_BUNDLE_BINARY
  ? resolve(process.env.SIM_FLOW_BUNDLE_BINARY)
  : join(repoRoot, "target", "release", exe);
const sourcePdfium = process.env.SIM_FLOW_BUNDLE_PDFIUM
  ? resolve(process.env.SIM_FLOW_BUNDLE_PDFIUM)
  : join(
      repoRoot,
      "tools",
      "sim-flow",
      "vendor",
      "pdfium",
      PDFIUM_DIR,
      PDFIUM_LIBNAME,
    );
const destDir = join(packageRoot, "bin", PLATFORM_DIR);

mkdirSync(destDir, { recursive: true });

if (!existsSync(sourceBin)) {
  console.error(
    `bundle-bin: ${sourceBin} not found.\n` +
      `  Build with \`npm run compile:cargo\` first.`,
  );
  process.exit(1);
}
const destBin = join(destDir, exe);
copyFileSync(sourceBin, destBin);
// Preserve executable permissions (copyFileSync drops them on some
// node versions on Linux/macOS).
if (process.platform !== "win32") {
  chmodSync(destBin, 0o755);
}
console.log(`copied ${sourceBin} -> ${destBin} (${prettyBytes(destBin)})`);

if (!existsSync(sourcePdfium)) {
  console.warn(
    `bundle-bin: ${sourcePdfium} not found.\n` +
      `  PDF spec ingestion will fall back to system libpdfium at runtime.\n` +
      `  Run \`node tools/sim-flow/scripts/fetch-pdfium.mjs --only ${PDFIUM_DIR}\` to populate.`,
  );
} else {
  const destPdfium = join(destDir, PDFIUM_LIBNAME);
  copyFileSync(sourcePdfium, destPdfium);
  console.log(`copied ${sourcePdfium} -> ${destPdfium} (${prettyBytes(destPdfium)})`);
}

if (!existsSync(join(apiDocsSourceDir, "toc.md"))) {
  console.error(
    `bundle-bin: ${apiDocsSourceDir} is missing normalized foundation API docs.\n` +
      `  Build with \`npm run compile:cargo\` first.`,
  );
  process.exit(1);
}
mkdirSync(join(packageRoot, "foundation-docs"), { recursive: true });
cpSync(apiDocsSourceDir, apiDocsDestDir, { recursive: true });
console.log(`copied ${apiDocsSourceDir} -> ${apiDocsDestDir}`);

function prettyBytes(path) {
  const size = statSync(path).size;
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}
