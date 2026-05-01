// Stage the sim-flow binary + libpdfium for the current platform
// under <ext>/bin/<platform-arch>/ so vsce includes them in the
// VSIX. Cross-compiling is out of scope here; CI / release tooling
// can stamp additional platforms by running this script with the
// right target binary and a populated vendor/pdfium tree.
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

// platform/arch keys must match `cli/bundled.ts::platformDir`.
const PLATFORM_DIR = (() => {
  const platform = process.platform;
  const arch = process.arch;
  if (platform === "darwin" && arch === "arm64") return "darwin-arm64";
  if (platform === "darwin" && arch === "x64") return "darwin-x64";
  if (platform === "linux" && arch === "x64") return "linux-x64";
  if (platform === "linux" && arch === "arm64") return "linux-arm64";
  if (platform === "win32" && arch === "x64") return "win32-x64";
  return null;
})();

const PDFIUM_DIR = (() => {
  // Mirrors the keys that `tools/sim-flow/scripts/fetch-pdfium.mjs`
  // writes into. Different naming from the bin/ tree because that's
  // what each toolchain uses.
  const platform = process.platform;
  const arch = process.arch;
  if (platform === "darwin" && arch === "arm64") return "macos-arm64";
  if (platform === "darwin" && arch === "x64") return "macos-x64";
  if (platform === "linux" && arch === "x64") return "linux-x64";
  if (platform === "linux" && arch === "arm64") return "linux-arm64";
  if (platform === "win32" && arch === "x64") return "windows-x64";
  return null;
})();

const PDFIUM_LIBNAME =
  process.platform === "win32"
    ? "pdfium.dll"
    : process.platform === "darwin"
      ? "libpdfium.dylib"
      : "libpdfium.so";

if (!PLATFORM_DIR || !PDFIUM_DIR) {
  console.warn(
    `bundle-bin: skipping; unsupported (platform, arch) = (${process.platform}, ${process.arch})`,
  );
  process.exit(0);
}

const exe = process.platform === "win32" ? "sim-flow.exe" : "sim-flow";
const sourceBin = join(repoRoot, "target", "release", exe);
const sourcePdfium = join(
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
