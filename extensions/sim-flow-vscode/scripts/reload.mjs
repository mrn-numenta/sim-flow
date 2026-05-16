// Build the sim-flow Rust binary, package a fresh dev VSIX, and
// force-install it. The new dev version (hash + dirty suffix) is
// unique per build, so `--install-extension --force` replaces the
// previously-installed copy without needing a separate uninstall +
// extension-host restart. The user still needs to reload the VSCode
// window afterward (Cmd+Shift+P -> "Developer: Reload Window") for
// the new extension code to take effect.

import { execSync, spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { extDir, vsixPath } from "./paths.mjs";

// Step 1: rebuild the sim-flow Rust binary. bundle-bin.mjs (run by
// `npm run package` below) hard-fails if `target/release/sim-flow`
// doesn't exist, so doing this up front makes "npm run reload" the
// single command for "build everything and install."
const cargo = spawnSync("npm", ["run", "compile:cargo"], { cwd: extDir, stdio: "inherit" });
if (cargo.status !== 0) {
  process.exit(cargo.status ?? 1);
}

// Step 2: build a fresh dev VSIX. Reuses the existing package-dev
// pipeline (which restores package.json on success or failure).
const pkg = spawnSync("npm", ["run", "package"], { cwd: extDir, stdio: "inherit" });
if (pkg.status !== 0) {
  process.exit(pkg.status ?? 1);
}

// Step 3: read the version that was just packaged. The package-dev
// script logged the dev version to stdout; rather than parse logs,
// recompute it from git so the path is deterministic.
const baseVersion = JSON.parse(readFileSync(resolve(extDir, "package.json"), "utf8")).version;
const hash = execSync("git rev-parse --short HEAD", { encoding: "utf8" }).trim();
const dirty = execSync("git status --porcelain", { encoding: "utf8" }).trim().length > 0;
// Match the `g`-prefix from package-dev.mjs so an all-numeric git
// short-hash doesn't produce an invalid SemVer pre-release tag.
const devVersion = `${baseVersion}-dev.g${hash}${dirty ? ".dirty" : ""}`;
const vsix = vsixPath(devVersion);

console.log(`\nInstalling ${vsix}`);

// Step 4: force-install. `--force` skips the "already installed"
// short-circuit so the new dev build wins regardless of what was
// installed before.
const codeBin = process.env["VSCODE_BIN"] ?? "code";
const install = spawnSync(codeBin, ["--install-extension", vsix, "--force"], {
  cwd: extDir,
  stdio: "inherit",
});
if (install.status !== 0) {
  console.error(
    `Install failed. If the \`code\` CLI is not on your PATH, set VSCODE_BIN to the full path of the \`code\` shim (Cmd+Shift+P -> "Shell Command: Install 'code' command in PATH").`,
  );
  process.exit(install.status ?? 1);
}

console.log(
  "\nDone. Reload the VSCode window: Cmd+Shift+P -> \"Developer: Reload Window\" (or bind it to a shortcut).",
);
