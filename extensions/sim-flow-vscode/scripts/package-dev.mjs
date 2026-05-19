// Package the extension into a VSIX with a dev version derived from
// git: `<base>-dev.<short-hash>[.dirty]`. The source tree stays clean
// by packaging from a generated staging directory under the
// extension-local `build/`.

import { execSync, spawnSync } from "node:child_process";
import { basename, join, relative, resolve, sep } from "node:path";
import * as fs from "node:fs";

import { buildRoot, extDir, repoRoot, stageDir, vsixPath } from "./paths.mjs";

const pkgPath = resolve(extDir, "package.json");

function readPkg() {
  return JSON.parse(fs.readFileSync(pkgPath, "utf8"));
}

function writePkg(pkgPath, pkg) {
  fs.writeFileSync(pkgPath, JSON.stringify(pkg, null, 2) + "\n");
}

function gitShortHash() {
  return execSync("git rev-parse --short HEAD", { encoding: "utf8" }).trim();
}

function gitDirty() {
  const out = execSync("git status --porcelain", { encoding: "utf8" });
  return out.trim().length > 0;
}

function stripDevSuffix(version) {
  // Strip any `-dev.<...>` already present so we don't chain.
  const i = version.indexOf("-dev.");
  return i === -1 ? version : version.slice(0, i);
}

function stageFilter(src) {
  const rel = relative(extDir, src);
  if (rel === "") {
    return true;
  }
  const topLevel = rel.split(sep)[0];
  if (
    topLevel === "build" ||
    topLevel === "node_modules" ||
    topLevel === "bin" ||
    topLevel === "out"
  ) {
    return false;
  }
  return basename(src).endsWith(".vsix") ? false : true;
}

function stageRuntimeNodeModules() {
  const runtimePackages = ["better-sqlite3", "bindings", "file-uri-to-path", "smol-toml"];
  const stageNodeModules = join(stageDir, "node_modules");
  fs.mkdirSync(stageNodeModules, { recursive: true });
  for (const pkg of runtimePackages) {
    fs.cpSync(join(extDir, "node_modules", pkg), join(stageNodeModules, pkg), {
      recursive: true,
    });
  }
  const betterSqlitePkgPath = join(stageNodeModules, "better-sqlite3", "package.json");
  const betterSqlitePkg = JSON.parse(fs.readFileSync(betterSqlitePkgPath, "utf8"));
  if (betterSqlitePkg.dependencies) {
    delete betterSqlitePkg.dependencies["prebuild-install"];
  }
  writePkg(betterSqlitePkgPath, betterSqlitePkg);
}

function prepareStage() {
  fs.rmSync(stageDir, { recursive: true, force: true });
  fs.mkdirSync(buildRoot, { recursive: true });
  fs.mkdirSync(stageDir, { recursive: true });
  for (const entry of fs.readdirSync(extDir)) {
    const src = join(extDir, entry);
    if (!stageFilter(src)) {
      continue;
    }
    fs.cpSync(src, join(stageDir, entry), { recursive: true, filter: stageFilter });
  }
  stageRuntimeNodeModules();
}

function vsceExecutable() {
  return join(
    extDir,
    "node_modules",
    ".bin",
    process.platform === "win32" ? "vsce.cmd" : "vsce",
  );
}

const sourcePkg = readPkg();
const baseVersion = stripDevSuffix(sourcePkg.version);
const hash = gitShortHash();
const dirty = gitDirty();
// `git rev-parse --short HEAD` can return an all-numeric string
// (e.g. "0680042"). Pre-release identifiers in SemVer reject
// all-digit segments with a leading zero, and vsce treats those as
// invalid versions. Prefix with `g` (same convention `git describe`
// uses) so the identifier always contains at least one non-digit
// and the version stays SemVer-valid regardless of the hash.
const devVersion = `${baseVersion}-dev.g${hash}${dirty ? ".dirty" : ""}`;
const outputPath = vsixPath(devVersion);

console.log(`Packaging sim-flow-vscode ${devVersion} (from ${sourcePkg.version})`);
console.log(`Staging extension package under ${stageDir}`);

// Stage the sim-flow binary for the current platform under
// bin/<platform-arch>/ so vsce includes it in the VSIX.
// `bundle-bin.mjs` hard-fails when the sim-flow binary is missing
// -- a VSIX without sim-flow is useless.
prepareStage();

const stagePkgPath = resolve(stageDir, "package.json");
const stagePkg = JSON.parse(fs.readFileSync(stagePkgPath, "utf8"));
stagePkg.version = devVersion;
writePkg(stagePkgPath, stagePkg);

const bundle = spawnSync(
  "node",
  [resolve(extDir, "scripts", "bundle-bin.mjs"), stageDir],
  { cwd: extDir, stdio: "inherit" },
);
if (bundle.status !== 0) {
  console.error("package: bundle-bin.mjs failed; aborting before vsce package.");
  process.exit(bundle.status ?? 1);
}

fs.rmSync(outputPath, { force: true });

const result = spawnSync(
  vsceExecutable(),
  ["package", "--follow-symlinks", "--out", outputPath],
  {
  cwd: stageDir,
  env: {
    ...process.env,
    SIM_FOUNDATION_ROOT: repoRoot,
  },
  stdio: "inherit",
  },
);
if (result.status !== 0) {
  process.exit(result.status ?? 1);
}

console.log(`Built ${outputPath}`);
