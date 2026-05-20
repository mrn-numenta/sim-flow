#!/usr/bin/env bash
# Build sim-flow + the VS Code extension, optionally install the VSIX.
#
# Modes:
#   default            Build the release sim-flow binary, package the
#                      extension, and force-install the VSIX in VS Code
#                      via `$VSCODE_BIN` (or `code` on PATH). You still
#                      need Cmd+Shift+P -> "Developer: Reload Window"
#                      afterward.
#   --package-only     Same build, but stop after packaging. Prints the
#                      VSIX path so you can copy it to another machine
#                      and install it there with
#                          code --install-extension <file>.vsix
#                      The VSIX already embeds the sim-flow binary for
#                      the platform/arch this script ran on (see
#                      `bundle-bin.mjs` for the supported matrix); a
#                      VSIX built on darwin-arm64 will not run on
#                      linux-x64 etc. Build on the target platform when
#                      it differs.
#
# Env:
#   VSCODE_BIN         Path to the `code` CLI. Only used in the default
#                      install mode. Hardcoded macOS default below is
#                      a convenience; override or unset for Linux.
#   CARGO_PROFILE      Cargo profile for the sim-flow build. Defaults
#                      to `release` since that's what production VSIX
#                      installs ship. Set to `dev` for a faster
#                      iteration loop where size / speed of the
#                      embedded binary don't matter.
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: install-vscode-extension.sh [--package-only|-p] [--help|-h]

  (no flags)        Build, package, AND install the VSIX in VS Code.
  --package-only    Build + package only; print the VSIX path.
  --help            Show this message.
USAGE
}

PACKAGE_ONLY=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        -p|--package-only) PACKAGE_ONLY=1 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "install-vscode-extension: unknown flag: $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXT_DIR="${ROOT_DIR}/extensions/sim-flow-vscode"
CARGO_PROFILE="${CARGO_PROFILE:-release}"

log() { echo "install-vscode-extension: $*"; }

# 1. Build the sim-flow binary the bundler embeds into the VSIX.
#    `bundle-bin.mjs` hard-fails when this binary is missing.
log "building sim-flow binary (--${CARGO_PROFILE})"
if [[ "${CARGO_PROFILE}" == "release" ]]; then
    cargo build --release -p sim-flow --manifest-path "${ROOT_DIR}/Cargo.toml"
else
    cargo build -p sim-flow --manifest-path "${ROOT_DIR}/Cargo.toml"
fi

# 2. Either reload (build + install) or package-only.
if [[ "${PACKAGE_ONLY}" -eq 1 ]]; then
    log "packaging VSIX (no install)"
    npm --prefix "${EXT_DIR}" run package
    # Locate the produced VSIX. `package-dev.mjs` writes to
    # `<ext>/build/sim-flow-vscode-<version>.vsix`. Newest wins so
    # back-to-back package runs surface the just-built one.
    VSIX="$(ls -t "${EXT_DIR}/build/"sim-flow-vscode-*.vsix 2>/dev/null | head -1 || true)"
    if [[ -z "${VSIX}" ]]; then
        echo "install-vscode-extension: package step succeeded but no .vsix found under ${EXT_DIR}/build/" >&2
        exit 1
    fi
    log "VSIX ready: ${VSIX}"
    log "install on another machine with:"
    log "  code --install-extension $(basename "${VSIX}")"
    exit 0
fi

# 3. Default: build + install via `npm run reload`. Honors VSCODE_BIN
#    if set; otherwise falls back to whatever `code` is on PATH.
# Convenience macOS default for the local install path. Skipped when
# the caller already exported VSCODE_BIN in their shell.
if [[ -z "${VSCODE_BIN:-}" ]]; then
    DEFAULT_VSCODE_BIN="/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"
    if [[ -x "${DEFAULT_VSCODE_BIN}" ]]; then
        export VSCODE_BIN="${DEFAULT_VSCODE_BIN}"
    fi
fi

log "packaging + installing via npm run reload"
exec npm --prefix "${EXT_DIR}" run reload
