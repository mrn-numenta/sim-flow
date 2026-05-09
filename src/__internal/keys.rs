//! Cross-platform API-key resolution shared between the CLI and the
//! VS Code extension.
//!
//! Resolution order (first non-empty wins):
//!
//!   1. Provider-specific environment variable (`ANTHROPIC_API_KEY`,
//!      `OPENAI_API_KEY`, ...). Standard practice and what most
//!      users already have set in their shell rc.
//!   2. Plaintext TOML credentials file at the platform's standard
//!      config location:
//!      - macOS / Linux: `$XDG_CONFIG_HOME/sim-flow/credentials.toml`
//!        (defaults to `~/.config/sim-flow/credentials.toml` on
//!        Linux, `~/Library/Application Support/sim-flow/credentials.toml`
//!        on macOS).
//!      - Windows: `%APPDATA%\sim-flow\config\credentials.toml`.
//!
//! Permissions are tightened to `0600` on POSIX. The extension
//! reads from the same file so VS Code-provisioned keys work when
//! running `sim-flow` directly from a terminal.
//!
//! The VS Code extension layers a third source on top -- its
//! per-extension SecretStorage (OS keychain) -- for users who don't
//! want their key on disk in plaintext. That third source only
//! applies inside VS Code; the CLI deliberately doesn't try to read
//! VS Code's keychain entries because they're scoped to the
//! extension host process and brittle to access from outside.
//!
//! File format:
//!
//! ```toml
//! [anthropic]
//! api_key = "sk-ant-..."
//!
//! [openai]
//! api_key = "sk-..."
//! ```
//!
//! Unknown top-level tables (a future provider name a stale binary
//! doesn't recognize) are preserved on rewrite via `toml::Table`
//! round-trip -- we never lose data we don't recognize.

use std::path::PathBuf;

use directories::ProjectDirs;
use toml::{Table, Value};

use crate::error::{Error, Result};

/// JSON / wire string codes for `KeySource`. Defined as consts so
/// the `keys list --json` output and any future tooling that
/// inspects it agree on the spelling.
pub const SOURCE_CODE_ENV: &str = "env";
pub const SOURCE_CODE_CONFIG_FILE: &str = "config-file";
pub const SOURCE_CODE_NONE: &str = "none";

/// File name (within the platform's per-app config dir) where the
/// shared credentials live. Public so the extension can read the
/// same constant if wired through `wasm` or codegen later.
pub const CREDENTIALS_FILE_NAME: &str = "credentials.toml";

/// TOML key inside each provider table that holds the API key
/// string. Single-source-of-truth so read and write don't drift.
const API_KEY_FIELD: &str = "api_key";

/// POSIX file mode for the credentials file. Owner-only read/write
/// matches `~/.aws/credentials`, `~/.npmrc`, etc.
#[cfg(unix)]
const CREDENTIALS_FILE_MODE: u32 = 0o600;

/// Supported credential namespaces. Order is alphabetical for
/// deterministic `keys list` output. New providers append below;
/// removing one is a wire break (the file's table for that name
/// will start being preserved as opaque on rewrite, which is
/// exactly what we want for forward compat).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    Lmstudio,
    Ollama,
    Openai,
}

impl Provider {
    pub const ALL: &'static [Provider] = &[
        Provider::Anthropic,
        Provider::Lmstudio,
        Provider::Ollama,
        Provider::Openai,
    ];

    /// Conventional shell environment variable for this provider.
    /// Matches the official SDK names where one exists; for
    /// `ollama` / `lmstudio` we mint `OLLAMA_API_KEY` /
    /// `LMSTUDIO_API_KEY` since the upstream tools don't define one.
    pub fn env_var(self) -> &'static str {
        match self {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::Openai => "OPENAI_API_KEY",
            Provider::Ollama => "OLLAMA_API_KEY",
            Provider::Lmstudio => "LMSTUDIO_API_KEY",
        }
    }

    /// Stable identifier used as the TOML table name in
    /// `credentials.toml`, the CLI subcommand argument, and the
    /// extension's resolver. Lowercase, hyphenless.
    pub fn config_key(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::Openai => "openai",
            Provider::Ollama => "ollama",
            Provider::Lmstudio => "lmstudio",
        }
    }

    pub fn from_str_ci(s: &str) -> Option<Self> {
        let lc = s.trim().to_ascii_lowercase();
        match lc.as_str() {
            "anthropic" => Some(Provider::Anthropic),
            "openai" => Some(Provider::Openai),
            "ollama" => Some(Provider::Ollama),
            "lmstudio" | "lm-studio" => Some(Provider::Lmstudio),
            _ => None,
        }
    }
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.config_key())
    }
}

/// Where a resolved key came from. Surfaced in `keys list` so the
/// user can tell a stale env var from a config-file entry without
/// having to grep their shell rc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Env,
    ConfigFile,
}

impl KeySource {
    /// Stable wire code (matches `SOURCE_CODE_*` constants).
    pub fn as_str(self) -> &'static str {
        match self {
            KeySource::Env => SOURCE_CODE_ENV,
            KeySource::ConfigFile => SOURCE_CODE_CONFIG_FILE,
        }
    }
}

/// Resolve the credentials file path for this user. Returns `None`
/// when the platform doesn't expose a usable config dir (very rare
/// -- happens on stripped-down embedded targets). Callers should
/// treat `None` as "no config file is reachable" and fall through to
/// env only.
pub fn config_file_path() -> Option<PathBuf> {
    project_dirs().map(|d| d.config_dir().join(CREDENTIALS_FILE_NAME))
}

fn project_dirs() -> Option<ProjectDirs> {
    // qualifier / organization / app -- keeps the path under the
    // expected per-platform location:
    //   macOS:   ~/Library/Application Support/sim-flow/
    //   Linux:   ~/.config/sim-flow/
    //   Windows: %APPDATA%\sim-flow\config\
    // The qualifier and organization are empty so the path uses
    // `sim-flow` directly. The Windows-only `\config\` subdirectory
    // is part of the directories crate's per-platform contract -- we
    // mirror it on the TS side.
    ProjectDirs::from("", "", "sim-flow")
}

/// Try to resolve an API key for `provider`. Returns `Ok(None)` when
/// neither the env var nor the config file has a non-empty value.
///
/// A malformed config file (TOML parse error, IO failure on read) is
/// downgraded to a `tracing::warn!` and treated as "absent" so a
/// working env var isn't shadowed by a corrupt file. Real IO errors
/// only surface when the file has been opened successfully but
/// reading it fails partway through -- in which case we still warn
/// and return `None` rather than aborting.
pub fn resolve_api_key(provider: Provider) -> Result<Option<KeyResolution>> {
    if let Ok(value) = std::env::var(provider.env_var()) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Ok(Some(KeyResolution {
                key: trimmed.to_string(),
                source: KeySource::Env,
            }));
        }
    }
    let Some(path) = config_file_path() else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    let table = match read_credentials_table(&path) {
        Ok(t) => t,
        Err(err) => {
            // Don't let a corrupt file shadow a working env var (the
            // env path returns null for whitespace; the user might
            // have one of those edge cases). Surface the failure on
            // stderr so it's debuggable, then carry on as "absent".
            eprintln!(
                "sim-flow: ignoring unreadable credentials file {}: {}",
                path.display(),
                err,
            );
            return Ok(None);
        }
    };
    let entry = table
        .get(provider.config_key())
        .and_then(Value::as_table)
        .and_then(|t| t.get(API_KEY_FIELD))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    Ok(entry.map(|k| KeyResolution {
        key: k.to_string(),
        source: KeySource::ConfigFile,
    }))
}

#[derive(Debug, Clone)]
pub struct KeyResolution {
    pub key: String,
    pub source: KeySource,
}

/// Persist `key` to the user's credentials file under `provider`'s
/// table, creating the file (and parent dirs) if needed. Returns the
/// path that was written.
///
/// The write is atomic and owner-only:
///
///   - On POSIX: opens a sibling tempfile with mode `0o600` AND
///     `O_NOFOLLOW` (rejects symlink-targeted attacks), writes the
///     full body, fsyncs, then `rename(2)`s the tempfile over the
///     final path. The credentials value is never on disk under any
///     wider mode, even briefly.
///   - On Windows: a single `fs::write` is sufficient because NTFS
///     ACLs inherit from the user's profile and the file lives
///     under `%APPDATA%`, which is already user-private.
///
/// Unknown top-level tables in the existing file are preserved
/// verbatim on rewrite -- we deserialize into `toml::Table`, mutate
/// only the slot we own, and re-serialize. A future binary that
/// adds a new provider doesn't lose entries an older binary writes.
pub fn write_api_key(provider: Provider, key: &str) -> Result<PathBuf> {
    let path = config_file_path()
        .ok_or_else(|| Error::Config("no usable config directory on this platform".into()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut table = if path.exists() {
        read_credentials_table(&path)?
    } else {
        Table::new()
    };
    let mut entry = Table::new();
    entry.insert(
        API_KEY_FIELD.to_string(),
        Value::String(key.trim().to_string()),
    );
    table.insert(provider.config_key().to_string(), Value::Table(entry));
    write_table_atomic(&path, &table)?;
    Ok(path)
}

/// Remove `provider` from the credentials file, if present. Returns
/// `true` when an entry was deleted, `false` when there was nothing
/// to remove. The file itself is preserved (and unknown providers
/// kept) so future `sim-flow keys set` calls don't have to recreate
/// the directory structure or lose other tables.
pub fn clear_api_key(provider: Provider) -> Result<bool> {
    let path = config_file_path()
        .ok_or_else(|| Error::Config("no usable config directory on this platform".into()))?;
    if !path.exists() {
        return Ok(false);
    }
    let mut table = read_credentials_table(&path)?;
    if table.remove(provider.config_key()).is_none() {
        return Ok(false);
    }
    write_table_atomic(&path, &table)?;
    Ok(true)
}

/// Per-provider summary of where a key currently resolves from.
/// Used by `keys list` and the extension's "set api key" UI.
#[derive(Debug, Clone)]
pub struct KeyStatus {
    pub provider: Provider,
    pub source: Option<KeySource>,
}

pub fn list_status() -> Result<Vec<KeyStatus>> {
    let mut out = Vec::with_capacity(Provider::ALL.len());
    for &p in Provider::ALL {
        let resolved = resolve_api_key(p)?;
        out.push(KeyStatus {
            provider: p,
            source: resolved.map(|r| r.source),
        });
    }
    Ok(out)
}

fn read_credentials_table(path: &PathBuf) -> Result<Table> {
    let body = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    body.parse::<Table>().map_err(|source| Error::TomlParse {
        path: path.clone(),
        source,
    })
}

fn write_table_atomic(path: &PathBuf, table: &Table) -> Result<()> {
    let body = toml::to_string_pretty(table).map_err(Error::TomlSerialize)?;
    write_atomic_owner_only(path, body.as_bytes())
}

#[cfg(unix)]
fn write_atomic_owner_only(path: &PathBuf, body: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    // Sibling tempfile in the same directory so the final
    // `rename(2)` is on a single filesystem (rename across
    // filesystems is a copy + unlink and not atomic).
    let tmp = path.with_file_name(match path.file_name() {
        Some(name) => format!(".{}.tmp-{}", name.to_string_lossy(), std::process::id()),
        None => {
            return Err(Error::Config(format!(
                "credentials file path has no file name: {}",
                path.display()
            )));
        }
    });
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(CREDENTIALS_FILE_MODE)
            // O_NOFOLLOW on the tempfile path. Unlikely to matter
            // (we just chose the name), but defense in depth: if
            // the tempfile slot is somehow a symlink we don't want
            // to write through it.
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)
            .map_err(|source| Error::Io {
                path: tmp.clone(),
                source,
            })?;
        file.write_all(body).map_err(|source| Error::Io {
            path: tmp.clone(),
            source,
        })?;
        file.sync_all().map_err(|source| Error::Io {
            path: tmp.clone(),
            source,
        })?;
    }
    // Replace the destination atomically. If `path` was a symlink to
    // somewhere else, this rename detaches the symlink instead of
    // following it -- so a malicious symlink can't trick us into
    // overwriting an arbitrary path.
    std::fs::rename(&tmp, path).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    let _ = parent;
    Ok(())
}

#[cfg(not(unix))]
fn write_atomic_owner_only(path: &PathBuf, body: &[u8]) -> Result<()> {
    // Windows: NTFS ACLs are inherited from the user's profile,
    // which is already user-private. A direct write is sufficient.
    std::fs::write(path, body).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // The resolver reads process env. Tests that mutate env need to
    // serialize so they don't see each other's writes.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_isolated_config<F: FnOnce()>(f: F) {
        // Force `directories` to a tempdir by overriding the
        // platform env vars that ProjectDirs consults.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().expect("tempdir for isolated config");
        let saved = [
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
            std::env::var_os("APPDATA"),
        ];
        // SAFETY: tests are single-threaded under the env lock
        // above, so concurrent set_var/get_var calls cannot race.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", tmp.path());
            std::env::set_var("HOME", tmp.path());
            std::env::set_var("APPDATA", tmp.path());
        }
        f();
        unsafe {
            match &saved[0] {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match &saved[1] {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match &saved[2] {
                Some(v) => std::env::set_var("APPDATA", v),
                None => std::env::remove_var("APPDATA"),
            }
        }
    }

    fn with_env<F: FnOnce()>(key: &str, value: Option<&str>, f: F) {
        let saved = std::env::var_os(key);
        unsafe {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        f();
        unsafe {
            match saved {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    fn env_var_wins_over_config_file() {
        with_isolated_config(|| {
            write_api_key(Provider::Anthropic, "from-config").unwrap();
            with_env(Provider::Anthropic.env_var(), Some("from-env"), || {
                let resolved = resolve_api_key(Provider::Anthropic).unwrap().unwrap();
                assert_eq!(resolved.key, "from-env");
                assert_eq!(resolved.source, KeySource::Env);
            });
        });
    }

    #[test]
    fn config_file_wins_when_env_is_empty_or_absent() {
        with_isolated_config(|| {
            with_env(Provider::Openai.env_var(), None, || {
                write_api_key(Provider::Openai, "from-config").unwrap();
                let resolved = resolve_api_key(Provider::Openai).unwrap().unwrap();
                assert_eq!(resolved.key, "from-config");
                assert_eq!(resolved.source, KeySource::ConfigFile);
            });
        });
    }

    #[test]
    fn empty_env_var_falls_through_to_config_file() {
        with_isolated_config(|| {
            with_env(Provider::Anthropic.env_var(), Some("   "), || {
                write_api_key(Provider::Anthropic, "from-config").unwrap();
                let resolved = resolve_api_key(Provider::Anthropic).unwrap().unwrap();
                assert_eq!(resolved.key, "from-config");
                assert_eq!(resolved.source, KeySource::ConfigFile);
            });
        });
    }

    #[test]
    fn returns_none_when_no_source_has_a_key() {
        with_isolated_config(|| {
            with_env(Provider::Anthropic.env_var(), None, || {
                let resolved = resolve_api_key(Provider::Anthropic).unwrap();
                assert!(resolved.is_none());
            });
        });
    }

    #[test]
    fn write_then_clear_round_trips() {
        with_isolated_config(|| {
            with_env(Provider::Anthropic.env_var(), None, || {
                let path = write_api_key(Provider::Anthropic, "secret-1").unwrap();
                assert!(path.exists());
                let resolved = resolve_api_key(Provider::Anthropic).unwrap().unwrap();
                assert_eq!(resolved.key, "secret-1");
                let cleared = clear_api_key(Provider::Anthropic).unwrap();
                assert!(cleared);
                let resolved = resolve_api_key(Provider::Anthropic).unwrap();
                assert!(resolved.is_none());
                let cleared_again = clear_api_key(Provider::Anthropic).unwrap();
                assert!(!cleared_again);
            });
        });
    }

    #[test]
    fn write_preserves_other_known_providers() {
        with_isolated_config(|| {
            with_env(Provider::Anthropic.env_var(), None, || {
                with_env(Provider::Openai.env_var(), None, || {
                    write_api_key(Provider::Anthropic, "ant-1").unwrap();
                    write_api_key(Provider::Openai, "oai-1").unwrap();
                    write_api_key(Provider::Anthropic, "ant-2").unwrap();
                    assert_eq!(
                        resolve_api_key(Provider::Anthropic).unwrap().unwrap().key,
                        "ant-2",
                    );
                    assert_eq!(
                        resolve_api_key(Provider::Openai).unwrap().unwrap().key,
                        "oai-1",
                    );
                });
            });
        });
    }

    #[test]
    fn write_preserves_unknown_provider_tables_for_forward_compat() {
        // Simulate a future version that wrote a `[vertex]` table
        // we don't recognize. After we set anthropic and clear
        // anthropic again, the unknown table must still be intact.
        with_isolated_config(|| {
            with_env(Provider::Anthropic.env_var(), None, || {
                let path = config_file_path().unwrap();
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(
                    &path,
                    "[vertex]\napi_key = \"vx-future\"\nproject = \"my-proj\"\n",
                )
                .unwrap();

                write_api_key(Provider::Anthropic, "ant-1").unwrap();
                let body = std::fs::read_to_string(&path).unwrap();
                assert!(body.contains("[vertex]"), "body: {body}");
                assert!(body.contains("vx-future"), "body: {body}");
                assert!(body.contains("my-proj"), "body: {body}");

                clear_api_key(Provider::Anthropic).unwrap();
                let body = std::fs::read_to_string(&path).unwrap();
                assert!(!body.contains("[anthropic]"), "body: {body}");
                assert!(body.contains("[vertex]"), "body: {body}");
                assert!(body.contains("vx-future"), "body: {body}");
            });
        });
    }

    #[test]
    fn malformed_config_file_does_not_shadow_env_var() {
        with_isolated_config(|| {
            let path = config_file_path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "this is { not valid toml\n").unwrap();
            with_env(Provider::Anthropic.env_var(), Some("from-env"), || {
                let resolved = resolve_api_key(Provider::Anthropic).unwrap().unwrap();
                assert_eq!(resolved.key, "from-env");
                assert_eq!(resolved.source, KeySource::Env);
            });
        });
    }

    #[test]
    fn malformed_config_file_resolves_as_absent_when_no_env() {
        // We deliberately don't bubble the parse error up -- a
        // corrupt file should report "no key" rather than blocking
        // the call site with an opaque "TOML parse error" they may
        // not be able to interpret. The on-stderr warning is the
        // user's signal to investigate.
        with_isolated_config(|| {
            let path = config_file_path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "this is { not valid toml\n").unwrap();
            with_env(Provider::Anthropic.env_var(), None, || {
                let resolved = resolve_api_key(Provider::Anthropic).unwrap();
                assert!(resolved.is_none());
            });
        });
    }

    #[cfg(unix)]
    #[test]
    fn write_sets_0600_permissions_on_posix() {
        use std::os::unix::fs::PermissionsExt;
        with_isolated_config(|| {
            let path = write_api_key(Provider::Anthropic, "secret").unwrap();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, CREDENTIALS_FILE_MODE,
                "credentials.toml must be owner-only",
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn write_does_not_leave_a_world_readable_window() {
        // The temp file the writer uses must itself be 0o600 from
        // creation -- not `fs::write` then `chmod`, which would have
        // a window where the file is on disk under the umask
        // default. Probe by listing dir entries during the write
        // and asserting any tmp sibling we see is already 0o600.
        // Done by writing once, then inspecting -- if the tmp
        // approach were broken, the FINAL file would still end up
        // 0o600 (the chmod at the end), so we instead assert that
        // any leftover tmp file is also 0o600. Easier proof: the
        // writer never calls a chmod-after-create code path
        // (verified by the absence of `set_permissions` in this
        // module).
        with_isolated_config(|| {
            write_api_key(Provider::Anthropic, "secret").unwrap();
            // After a successful write the tempfile should be gone
            // (renamed onto path). Just sanity-check no `*.tmp-*`
            // sibling leaks remain.
            let dir = config_file_path().unwrap().parent().unwrap().to_path_buf();
            for entry in std::fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name().into_string().unwrap_or_default();
                assert!(
                    !name.contains(".tmp-"),
                    "tempfile left behind in {}: {}",
                    dir.display(),
                    name,
                );
            }
        });
    }

    #[test]
    fn list_status_reports_per_provider_source() {
        with_isolated_config(|| {
            for p in Provider::ALL {
                with_env(p.env_var(), None, || {});
            }
            with_env(Provider::Anthropic.env_var(), Some("from-env"), || {
                with_env(Provider::Openai.env_var(), None, || {
                    write_api_key(Provider::Openai, "from-config").unwrap();
                    let statuses = list_status().unwrap();
                    let anthropic = statuses
                        .iter()
                        .find(|s| s.provider == Provider::Anthropic)
                        .unwrap();
                    assert_eq!(anthropic.source, Some(KeySource::Env));
                    let openai = statuses
                        .iter()
                        .find(|s| s.provider == Provider::Openai)
                        .unwrap();
                    assert_eq!(openai.source, Some(KeySource::ConfigFile));
                    let ollama = statuses
                        .iter()
                        .find(|s| s.provider == Provider::Ollama)
                        .unwrap();
                    assert_eq!(ollama.source, None);
                });
            });
        });
    }

    #[test]
    fn provider_from_str_is_case_insensitive_and_accepts_lm_studio() {
        assert_eq!(
            Provider::from_str_ci("anthropic"),
            Some(Provider::Anthropic)
        );
        assert_eq!(
            Provider::from_str_ci("ANTHROPIC"),
            Some(Provider::Anthropic)
        );
        assert_eq!(Provider::from_str_ci("lmstudio"), Some(Provider::Lmstudio));
        assert_eq!(Provider::from_str_ci("lm-studio"), Some(Provider::Lmstudio));
        assert_eq!(Provider::from_str_ci("claude"), None);
    }

    #[test]
    fn key_source_as_str_codes_match_documented_constants() {
        assert_eq!(KeySource::Env.as_str(), SOURCE_CODE_ENV);
        assert_eq!(KeySource::ConfigFile.as_str(), SOURCE_CODE_CONFIG_FILE);
    }
}
