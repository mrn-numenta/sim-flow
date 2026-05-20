#![doc(hidden)]

#[doc(hidden)]
pub mod __internal;

#[doc(hidden)]
pub use __internal::*;

/// Build sim-flow + the VS Code extension, optionally installing the
/// VSIX in VS Code. Replaces the legacy `scripts/install-vscode-extension.sh`
/// and is the canonical entry point for consumers (sim-models' `xtask`,
/// the `sim-flow install-extension` subcommand). Lives at the top
/// level rather than under `__internal` because it IS the crate's
/// public interface; everything else stays internal until a consumer
/// needs it.
pub mod install;
