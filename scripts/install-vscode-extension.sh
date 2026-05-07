#!/usr/bin/env bash
# Build sim-flow + the VS Code extension and force-install the VSIX.
# Single command for "I changed Rust or extension code; install it."
#
# - Runs `cargo build --release -p sim-flow` (so the extension's bundle
#   step has the binary to embed).
# - Runs `npm run reload` inside the extension dir, which packages a
#   dev-versioned VSIX (`<base>-dev.<sha>[.dirty]`) and force-installs
#   it via `$VSCODE_BIN` (or `code` on PATH).
# - The user still needs to reload the VS Code window afterward
#   (Cmd+Shift+P → "Developer: Reload Window") for the new code to
#   take effect.
#
# Set VSCODE_BIN if `code` isn't on your PATH. On macOS:
#   export VSCODE_BIN="/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"
set -euo pipefail

export VSCODE_BIN=/Applications/Visual\ Studio\ Code.app/Contents/Resources/app/bin/code
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXT_DIR="${ROOT_DIR}/tools/sim-flow/extensions/sim-flow-vscode"

cd "${ROOT_DIR}"
exec npm --prefix "${EXT_DIR}" run reload
