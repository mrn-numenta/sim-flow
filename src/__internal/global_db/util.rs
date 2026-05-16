//! Small, shared helpers used by every submodule of `global_db`.
//!
//! Kept here so `schema.rs`, `writers.rs`, and `queries.rs` don't
//! each re-declare them (or worse, drift in shape).

use std::path::Path;

use crate::Error;

use super::project_dir_key;

/// Wrap a `rusqlite::Error` as `Error::State`. Every connection /
/// statement call routes through this so the error surface stays
/// uniform across writer modules.
pub(super) fn wrap_sqlite(source: rusqlite::Error) -> Error {
    Error::State(format!("global-db sqlite error: {source}"))
}

/// Minimal base64 encoder for BLOB cells in `query_read_only`. Keeps
/// the dependency surface unchanged -- pulling in a base64 crate just
/// for this tiny case isn't worth it.
pub(super) fn base64_encode_bytes(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let b0 = bytes[i] as u32;
        let b1 = bytes[i + 1] as u32;
        let b2 = bytes[i + 2] as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        out.push(ALPHABET[(triple & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let b0 = bytes[i] as u32;
        out.push(ALPHABET[((b0 >> 2) & 0x3f) as usize] as char);
        out.push(ALPHABET[((b0 << 4) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let b0 = bytes[i] as u32;
        let b1 = bytes[i + 1] as u32;
        out.push(ALPHABET[((b0 >> 2) & 0x3f) as usize] as char);
        out.push(ALPHABET[(((b0 << 4) | (b1 >> 4)) & 0x3f) as usize] as char);
        out.push(ALPHABET[((b1 << 2) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

/// Compose the `meta` key for a per-source backfill offset.
pub(super) fn backfill_offset_key(project_dir: &Path, source: &str) -> String {
    format!(
        "backfill_offset:{}:{}",
        project_dir_key(project_dir),
        source
    )
}

/// Defensive guard for the table / column names interpolated into
/// `count()` / `latest_timestamp()` queries. Rusqlite's parameter
/// binding doesn't cover identifiers, and the CLI passes them in from
/// a closed allowlist anyway -- this is belt-and-suspenders so a typo
/// in the allowlist can't produce a query-injection vector.
pub(super) fn is_safe_table_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}
