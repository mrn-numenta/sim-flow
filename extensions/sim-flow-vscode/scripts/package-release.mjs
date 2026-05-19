// Package a release VSIX with binaries staged for multiple
// platforms. Companion to `package-dev.mjs`, which only ever stages
// the current runner's platform and stamps a `-dev.<sha>` version.
//
// What this script does:
//   1. Stage the extension tree the same way `package-dev.mjs` does
//      (copy files honoring an allowlist, then stage the runtime
//      node_modules subset and strip `prebuild-install` from
//      better-sqlite3 so the VSIX installs offline).
//   2. Set the staged `package.json` version from
//      `SIM_FLOW_RELEASE_VERSION` (or fall back to whatever's in
//      the source package.json -- useful for local smoke runs).
//   3. For each target listed in `SIM_FLOW_BUNDLE_TARGETS`
//      (comma-separated `platform-arch` keys), invoke
//      `bundle-bin.mjs` with per-target env overrides so a single
//      VSIX gets bin/<platform-arch>/ subtrees for ALL listed
//      targets. The per-target env var is
//      `SIM_FLOW_BUNDLE_<TARGET>_BINARY` (target uppercased, `-`
//      preserved -- see normalizeTargetEnv below).
//   4. Run `vsce package` from the staging dir.
//
// CI usage (see `.github/workflows/release.yml`):
//   SIM_FLOW_RELEASE_VERSION=0.1.0 \
//   SIM_FLOW_BUNDLE_TARGETS=darwin-arm64,linux-x64 \
//   SIM_FLOW_BUNDLE_DARWIN_ARM64_BINARY=/abs/path/to/sim-flow \
//   SIM_FLOW_BUNDLE_LINUX_X64_BINARY=/abs/path/to/sim-flow \
//     node scripts/package-release.mjs
//
// Local smoke (single platform, current binary):
//   SIM_FLOW_BUNDLE_TARGETS=darwin-arm64 \
//     node scripts/package-release.mjs

import { spawnSync } from "node:child_process";
import { basename, join, relative, resolve, sep } from "node:path";
import * as fs from "node:fs";

import { buildRoot, extDir, repoRoot, stageDir, vsixPath } from "./paths.mjs";

const pkgPath = resolve(extDir, "package.json");

function readPkg(p) {
  return JSON.parse(fs.readFileSync(p, "utf8"));
}

function writePkg(p, pkg) {
  fs.writeFileSync(p, JSON.stringify(pkg, null, 2) + "\n");
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
  // Strip `prebuild-install` from better-sqlite3's deps so the
  // VSIX install doesn't try to fetch a prebuild over the network
  // on install. This matches `package-dev.mjs`'s behavior; without
  // it, install-on-target fails in sandboxed environments.
  const betterSqlitePkgPath = join(stageNodeModules, "better-sqlite3", "package.json");
  const betterSqlitePkg = readPkg(betterSqlitePkgPath);
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

// Convert `darwin-arm64` -> `DARWIN_ARM64` for env-var lookups.
function normalizeTargetEnv(target) {
  return target.replace(/-/g, "_").toUpperCase();
}

const sourcePkg = readPkg(pkgPath);
const releaseVersion = (process.env.SIM_FLOW_RELEASE_VERSION ?? "").trim() || sourcePkg.version;

const targetsRaw = (process.env.SIM_FLOW_BUNDLE_TARGETS ?? "").trim();
if (!targetsRaw) {
  console.error(
    "package-release: SIM_FLOW_BUNDLE_TARGETS is required (comma-separated platform-arch keys, e.g. `darwin-arm64,linux-x64`).",
  );
  process.exit(2);
}
const targets = targetsRaw
  .split(",")
  .map((t) => t.trim())
  .filter(Boolean);

console.log(`Packaging sim-flow-vscode ${releaseVersion} (release; targets=${targets.join(",")})`);
console.log(`Staging extension package under ${stageDir}`);

prepareStage();

const stagePkgPath = resolve(stageDir, "package.json");
const stagePkg = readPkg(stagePkgPath);
stagePkg.version = releaseVersion;
writePkg(stagePkgPath, stagePkg);

for (const target of targets) {
  const envKey = normalizeTargetEnv(target);
  const binary = process.env[`SIM_FLOW_BUNDLE_${envKey}_BINARY`];
  const env = {
    ...process.env,
    SIM_FLOW_BUNDLE_TARGET: target,
  };
  if (binary) env.SIM_FLOW_BUNDLE_BINARY = binary;
  console.log(`Staging target ${target} (binary=${binary ?? "(local)"})`);
  const bundle = spawnSync("node", [resolve(extDir, "scripts", "bundle-bin.mjs"), stageDir], {
    cwd: extDir,
    env,
    stdio: "inherit",
  });
  if (bundle.status !== 0) {
    console.error(`package-release: bundle-bin for ${target} failed; aborting.`);
    process.exit(bundle.status ?? 1);
  }
}

const outputPath = vsixPath(releaseVersion);
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
