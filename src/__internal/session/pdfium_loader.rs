//! Runtime libpdfium resolution. The crate uses pdfium-render which
//! dynamically loads `libpdfium.{dylib,so,dll}` at runtime; this
//! module decides which file to load.
//!
//! Resolution order:
//! 1. `SIM_FLOW_PDFIUM_LIB_PATH` env var (full path to the library
//!    file, not the directory). The VSCode extension sets this when
//!    spawning sim-flow so the bundled VSIX-shipped library wins
//!    over anything else.
//! 2. `vendor/pdfium/<platform>/<libname>` resolved relative to the
//!    sim-flow binary's parent directory. Lets a sim-flow that ships
//!    alongside the vendor tree work without extra configuration.
//! 3. `vendor/pdfium/<platform>/<libname>` resolved relative to
//!    `CARGO_MANIFEST_DIR` (i.e. the in-tree dev path) so
//!    `cargo run` / `cargo test` work without any setup.
//! 4. System library lookup (`libpdfium.dylib` on macOS, ...) as a
//!    last resort; no panic if it isn't installed -- we surface a
//!    clear error pointing the user at the vendor tree.

use std::env;
use std::path::{Path, PathBuf};

use pdfium_render::prelude::Pdfium;

use crate::{Error, Result};

const PLATFORM_KEY: &str = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    "macos-arm64"
} else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
    "macos-x64"
} else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    "linux-x64"
} else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
    "linux-arm64"
} else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
    "windows-x64"
} else {
    "unsupported"
};

const LIB_FILENAME: &str = if cfg!(target_os = "macos") {
    "libpdfium.dylib"
} else if cfg!(target_os = "linux") {
    "libpdfium.so"
} else if cfg!(target_os = "windows") {
    "pdfium.dll"
} else {
    "libpdfium"
};

/// Try to load PDFium from the resolution order documented above.
pub fn load() -> Result<Pdfium> {
    let candidates = candidate_paths();
    for path in &candidates {
        if path.is_file()
            && let Ok(bindings) = Pdfium::bind_to_library(path)
        {
            return Ok(Pdfium::new(bindings));
        }
    }
    if let Ok(bindings) = Pdfium::bind_to_system_library() {
        return Ok(Pdfium::new(bindings));
    }
    Err(Error::State(format!(
        "pdfium: could not load libpdfium for platform `{PLATFORM_KEY}`. \
         Tried: {}, then the system library. \
         Set `SIM_FLOW_PDFIUM_LIB_PATH` to a libpdfium file, or run \
         `node tools/sim-flow/scripts/fetch-pdfium.mjs --only {PLATFORM_KEY}` \
         to populate the vendor directory.",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("; ")
    )))
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(p) = env::var_os("SIM_FLOW_PDFIUM_LIB_PATH") {
        out.push(PathBuf::from(p));
    }
    if let Some(exe) = env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
    {
        out.push(
            exe.join("vendor")
                .join("pdfium")
                .join(PLATFORM_KEY)
                .join(LIB_FILENAME),
        );
        // Also accept the lib sitting directly next to the binary
        // (the layout the VSCode extension uses when bundling).
        out.push(exe.join(LIB_FILENAME));
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    out.push(
        manifest
            .join("vendor")
            .join("pdfium")
            .join(PLATFORM_KEY)
            .join(LIB_FILENAME),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_paths_includes_manifest_dir() {
        let paths = candidate_paths();
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let expected = manifest
            .join("vendor")
            .join("pdfium")
            .join(PLATFORM_KEY)
            .join(LIB_FILENAME);
        assert!(
            paths.contains(&expected),
            "candidate paths should include {}",
            expected.display()
        );
    }
}
