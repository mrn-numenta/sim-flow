// Fetch prebuilt libpdfium binaries from bblanchon/pdfium-binaries
// for all platforms sim-flow ships to. Run from the repo root or
// from `tools/sim-flow/`; output lands under
// `tools/sim-flow/vendor/pdfium/<platform>/`.
//
// Usage:
//   node tools/sim-flow/scripts/fetch-pdfium.mjs
//   node tools/sim-flow/scripts/fetch-pdfium.mjs --only macos-arm64

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Default tag from bblanchon/pdfium-binaries. Bump via the
// `PDFIUM_TAG` env var to refresh; the path-style tag (`chromium/<n>`)
// is what GitHub's release-download URL expects.
const TAG = process.env.PDFIUM_TAG ?? "chromium/7811";

// (platform-key, bblanchon-asset-name, library-filename-after-extract)
const TARGETS = [
  ["macos-arm64", "pdfium-mac-arm64.tgz", "libpdfium.dylib"],
  ["macos-x64", "pdfium-mac-x64.tgz", "libpdfium.dylib"],
  ["linux-x64", "pdfium-linux-x64.tgz", "libpdfium.so"],
  ["linux-arm64", "pdfium-linux-arm64.tgz", "libpdfium.so"],
  ["windows-x64", "pdfium-win-x64.tgz", "pdfium.dll"],
];

const here = dirname(fileURLToPath(import.meta.url));
const crateRoot = resolve(here, "..");
const vendorRoot = join(crateRoot, "vendor", "pdfium");

const onlyArg = process.argv.indexOf("--only");
const only = onlyArg >= 0 ? process.argv[onlyArg + 1] : null;

mkdirSync(vendorRoot, { recursive: true });
writeFileSync(
  join(vendorRoot, "VERSION"),
  `${TAG}\n` +
    "# bblanchon/pdfium-binaries release tag this vendor dir was last\n" +
    "# fetched from. Bump and re-run scripts/fetch-pdfium.mjs to refresh.\n",
);

for (const [key, asset, libname] of TARGETS) {
  if (only && only !== key) {
    continue;
  }
  const dest = join(vendorRoot, key);
  mkdirSync(dest, { recursive: true });
  const url = `https://github.com/bblanchon/pdfium-binaries/releases/download/${TAG}/${asset}`;
  const tar = join(dest, asset);
  console.log(`fetching ${url}`);
  const curl = spawnSync(
    "curl",
    ["--fail", "--silent", "--location", "--output", tar, url],
    { stdio: "inherit" },
  );
  if (curl.status !== 0) {
    console.error(`fetch failed for ${key}`);
    process.exit(1);
  }
  // Extract just the library file from the tarball. Layout is
  // platform-specific: libpdfium.{dylib,so} live under `lib/`,
  // pdfium.dll lives under `bin/`.
  const subpath = libname.endsWith(".dll") ? `bin/${libname}` : `lib/${libname}`;
  const tarRes = spawnSync(
    "tar",
    ["-xzf", tar, "-C", dest, "--strip-components=1", subpath],
    { stdio: "inherit" },
  );
  if (tarRes.status !== 0) {
    console.error(`extract failed for ${key}`);
    process.exit(1);
  }
  rmSync(tar);
  // Move from <dest>/{lib,bin}/<libname> up to <dest>/<libname>.
  // tar's --strip-components=1 already removed the prefix on most
  // platforms; double-check and tidy if it didn't.
  const flat = join(dest, libname);
  if (!existsSync(flat)) {
    const lib = join(dest, "lib", libname);
    const bin = join(dest, "bin", libname);
    if (existsSync(lib)) {
      spawnSync("mv", [lib, flat]);
      rmSync(join(dest, "lib"), { recursive: true, force: true });
    } else if (existsSync(bin)) {
      spawnSync("mv", [bin, flat]);
      rmSync(join(dest, "bin"), { recursive: true, force: true });
    }
  }
  if (!existsSync(flat)) {
    console.error(`${key}: ${libname} not found after extraction`);
    process.exit(1);
  }
  console.log(`  -> ${flat}`);
}

const version = readFileSync(join(vendorRoot, "VERSION"), "utf8").split("\n")[0];
console.log(`done. PDFium tag: ${version}`);
