//! Embedder configuration loader (Chapter 5 §5.6).
//!
//! A single TOML file describes the embedder. The orchestrator
//! looks up in priority order:
//!
//! 1. `<cwd>/.sim-flow/embedder.toml` (per-project override).
//! 2. `$SIM_FLOW_EMBEDDER_CONFIG` env var (operator override).
//! 3. `~/.sim-flow/embedder.toml` (user-default).
//!
//! The first found wins. Missing sub-sections (`[performance]`,
//! `[retry]`) inherit defaults; `[auth]` is itself optional (no
//! Authorization header is sent when absent).

use std::env;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Source of the active config. Threaded through to the CLI for
/// diagnostic output (which file actually fed the loader).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    /// `<cwd>/.sim-flow/embedder.toml`.
    ProjectFile(PathBuf),
    /// Pointed at by `$SIM_FLOW_EMBEDDER_CONFIG`.
    EnvOverride(PathBuf),
    /// `~/.sim-flow/embedder.toml`.
    UserDefault(PathBuf),
    /// Explicit `--config` path (CLI override).
    Explicit(PathBuf),
}

impl ConfigSource {
    /// Underlying path read from disk.
    pub fn path(&self) -> &Path {
        match self {
            ConfigSource::ProjectFile(p)
            | ConfigSource::EnvOverride(p)
            | ConfigSource::UserDefault(p)
            | ConfigSource::Explicit(p) => p,
        }
    }
}

/// Configuration error raised by the loader.
#[derive(Debug)]
pub enum ConfigError {
    /// None of the three priority paths existed and no explicit
    /// path was supplied.
    NotFound,
    /// File existed but could not be read.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// File parsed but had a structural problem (missing
    /// `provider`, unknown provider, negative numbers, etc.).
    Invalid { path: PathBuf, message: String },
    /// Toml-syntax-level parse failure.
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    /// `[auth]` present but the referenced env var is missing or
    /// empty.
    AuthEnvUnset { env_var: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::NotFound => write!(
                f,
                "embedder.toml not found: looked in <cwd>/.sim-flow/, \
                 $SIM_FLOW_EMBEDDER_CONFIG, and ~/.sim-flow/"
            ),
            ConfigError::Io { path, source } => write!(
                f,
                "failed to read embedder config at {}: {source}",
                path.display()
            ),
            ConfigError::Invalid { path, message } => write!(
                f,
                "invalid embedder config at {}: {message}",
                path.display()
            ),
            ConfigError::Parse { path, source } => write!(
                f,
                "failed to parse embedder config at {}: {source}",
                path.display()
            ),
            ConfigError::AuthEnvUnset { env_var } => write!(
                f,
                "embedder [auth] block references env var {env_var} \
                 but it is unset or empty"
            ),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io { source, .. } => Some(source),
            ConfigError::Parse { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Fully-resolved embedder config. Defaults have been applied; the
/// `auth_value` (if any) has been read from the environment.
#[derive(Debug, Clone)]
pub struct EmbedderConfig {
    /// Schema version, currently always `1`.
    pub schema_version: u32,
    /// Provider id; currently only `"openai-compat"` is recognized.
    pub provider: String,
    /// HTTP base URL (e.g. `http://localhost:11434/v1`).
    pub base_url: String,
    /// Model identifier passed to the embedding endpoint.
    pub model: String,
    /// Expected output vector dimension. Verified by the
    /// constructor's smoke embed.
    pub dimension: usize,
    /// Optional auth block. `None` means no `Authorization` header
    /// is sent.
    pub auth: Option<ResolvedAuth>,
    /// Performance / batching settings.
    pub performance: PerformanceConfig,
    /// Retry policy.
    pub retry: RetryConfig,
    /// Source the config was loaded from.
    pub source: ConfigSource,
}

/// Resolved auth block: shape from the file plus the env-var value
/// that was read at load time.
#[derive(Debug, Clone)]
pub struct ResolvedAuth {
    /// Header name to send (e.g. `"Authorization"`).
    pub header_name: String,
    /// Environment variable that was read.
    pub env_var: String,
    /// Optional value prefix (e.g. `"Bearer "`).
    pub value_prefix: String,
    /// The full header value: `format!("{value_prefix}{env_value}")`.
    pub header_value: String,
}

/// Optional `[auth]` block on disk. Strictly the shape the file may
/// carry; resolution into a `ResolvedAuth` happens during
/// [`EmbedderConfig::load`].
#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    /// HTTP header to send. Default: `Authorization`.
    #[serde(default = "default_auth_header")]
    pub header_name: String,
    /// Environment variable that holds the API key. Required.
    pub env_var: String,
    /// String prepended to the env-var value when building the
    /// header. Default: `"Bearer "` (with trailing space).
    #[serde(default = "default_auth_prefix")]
    pub value_prefix: String,
}

fn default_auth_header() -> String {
    "Authorization".to_string()
}

fn default_auth_prefix() -> String {
    "Bearer ".to_string()
}

/// `[performance]` block. Optional in the file; each field has a
/// default per §5.6.
#[derive(Debug, Clone)]
pub struct PerformanceConfig {
    /// Max texts per HTTP request to the provider.
    pub batch_size: usize,
    /// Max chars per input; longer inputs are truncated with a
    /// warning (truncation lands in the embedder itself).
    pub max_input_chars: usize,
    /// Per-request wall-clock timeout.
    pub timeout_secs: u64,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            batch_size: 32,
            max_input_chars: 8000,
            timeout_secs: 30,
        }
    }
}

/// `[retry]` block. Optional; defaults per §5.6.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Total attempts (including the first). `1` disables retry.
    pub max_attempts: u32,
    /// Backoff delay before the second attempt.
    pub initial_delay_ms: u64,
    /// Cap on exponential backoff between attempts.
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay_ms: 200,
            max_delay_ms: 2000,
        }
    }
}

/// Raw on-disk shape; transformed into `EmbedderConfig` by the loader.
#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    #[serde(default = "default_provider")]
    provider: String,
    base_url: String,
    model: String,
    dimension: usize,
    #[serde(default)]
    auth: Option<AuthConfig>,
    #[serde(default)]
    performance: Option<RawPerformance>,
    #[serde(default)]
    retry: Option<RawRetry>,
}

fn default_schema_version() -> u32 {
    1
}

fn default_provider() -> String {
    "openai-compat".to_string()
}

#[derive(Debug, Deserialize, Default)]
struct RawPerformance {
    batch_size: Option<usize>,
    max_input_chars: Option<usize>,
    timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
struct RawRetry {
    max_attempts: Option<u32>,
    initial_delay_ms: Option<u64>,
    max_delay_ms: Option<u64>,
}

impl EmbedderConfig {
    /// Load the embedder config from the first path that exists in
    /// the priority order documented at the module level.
    pub fn load() -> Result<EmbedderConfig, ConfigError> {
        Self::load_with(&LoadContext::from_env())
    }

    /// Load the embedder config from the supplied explicit path,
    /// bypassing the priority order. Used by `sim-flow embedder
    /// check --config <path>`.
    pub fn load_explicit(path: impl AsRef<Path>) -> Result<EmbedderConfig, ConfigError> {
        let path = path.as_ref().to_path_buf();
        let bytes = read_path(&path)?;
        parse_and_resolve(bytes, ConfigSource::Explicit(path))
    }

    /// Load with an injected context. Public-in-crate so unit tests
    /// can stub the cwd / env / home roots without poking real
    /// environment variables.
    pub(crate) fn load_with(ctx: &LoadContext) -> Result<EmbedderConfig, ConfigError> {
        for candidate in ctx.candidates() {
            if candidate.path().is_file() {
                let bytes = read_path(candidate.path())?;
                return parse_and_resolve(bytes, candidate);
            }
        }
        Err(ConfigError::NotFound)
    }
}

/// Resolver context: the three roots (cwd / env-pointed / home) the
/// loader walks in priority order. Decoupled from real `std::env`
/// so tests can drive the loader without touching process state.
#[derive(Debug, Default)]
pub(crate) struct LoadContext {
    pub project_cwd: Option<PathBuf>,
    pub env_pointer: Option<PathBuf>,
    pub home_dir: Option<PathBuf>,
}

impl LoadContext {
    /// Read the real environment.
    pub(crate) fn from_env() -> Self {
        let project_cwd = env::current_dir().ok();
        let env_pointer = env::var_os("SIM_FLOW_EMBEDDER_CONFIG").map(PathBuf::from);
        let home_dir = home_dir_real();
        Self {
            project_cwd,
            env_pointer,
            home_dir,
        }
    }

    fn candidates(&self) -> Vec<ConfigSource> {
        let mut out = Vec::with_capacity(3);
        if let Some(cwd) = &self.project_cwd {
            out.push(ConfigSource::ProjectFile(
                cwd.join(".sim-flow").join("embedder.toml"),
            ));
        }
        if let Some(envp) = &self.env_pointer {
            out.push(ConfigSource::EnvOverride(envp.clone()));
        }
        if let Some(home) = &self.home_dir {
            out.push(ConfigSource::UserDefault(
                home.join(".sim-flow").join("embedder.toml"),
            ));
        }
        out
    }
}

fn home_dir_real() -> Option<PathBuf> {
    // The `directories` crate is already a sim-flow dep; using its
    // BaseDirs keeps the resolution consistent with the rest of the
    // CLI's "where does ~/.sim-flow live" answer.
    directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf())
}

fn read_path(path: &Path) -> Result<Vec<u8>, ConfigError> {
    std::fs::read(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn parse_and_resolve(bytes: Vec<u8>, source: ConfigSource) -> Result<EmbedderConfig, ConfigError> {
    let text = String::from_utf8(bytes).map_err(|e| ConfigError::Invalid {
        path: source.path().to_path_buf(),
        message: format!("file is not valid UTF-8: {e}"),
    })?;

    let raw: RawConfig = toml::from_str(&text).map_err(|e| ConfigError::Parse {
        path: source.path().to_path_buf(),
        source: e,
    })?;

    if raw.schema_version != 1 {
        return Err(ConfigError::Invalid {
            path: source.path().to_path_buf(),
            message: format!(
                "schema_version = {} is not supported (this build expects 1)",
                raw.schema_version
            ),
        });
    }
    if raw.provider != "openai-compat" {
        return Err(ConfigError::Invalid {
            path: source.path().to_path_buf(),
            message: format!(
                "provider = \"{}\" is not supported (v1 only knows openai-compat)",
                raw.provider
            ),
        });
    }
    if raw.base_url.is_empty() {
        return Err(ConfigError::Invalid {
            path: source.path().to_path_buf(),
            message: "base_url must not be empty".into(),
        });
    }
    if raw.model.is_empty() {
        return Err(ConfigError::Invalid {
            path: source.path().to_path_buf(),
            message: "model must not be empty".into(),
        });
    }
    if raw.dimension == 0 {
        return Err(ConfigError::Invalid {
            path: source.path().to_path_buf(),
            message: "dimension must be greater than zero".into(),
        });
    }

    let auth = match raw.auth {
        Some(spec) => Some(resolve_auth(spec)?),
        None => None,
    };

    let performance = raw
        .performance
        .map(|p| PerformanceConfig {
            batch_size: p.batch_size.unwrap_or(32),
            max_input_chars: p.max_input_chars.unwrap_or(8000),
            timeout_secs: p.timeout_secs.unwrap_or(30),
        })
        .unwrap_or_default();

    if performance.batch_size == 0 {
        return Err(ConfigError::Invalid {
            path: source.path().to_path_buf(),
            message: "performance.batch_size must be greater than zero".into(),
        });
    }
    if performance.timeout_secs == 0 {
        return Err(ConfigError::Invalid {
            path: source.path().to_path_buf(),
            message: "performance.timeout_secs must be greater than zero".into(),
        });
    }

    let retry = raw
        .retry
        .map(|r| RetryConfig {
            max_attempts: r.max_attempts.unwrap_or(3),
            initial_delay_ms: r.initial_delay_ms.unwrap_or(200),
            max_delay_ms: r.max_delay_ms.unwrap_or(2000),
        })
        .unwrap_or_default();

    if retry.max_attempts == 0 {
        return Err(ConfigError::Invalid {
            path: source.path().to_path_buf(),
            message: "retry.max_attempts must be at least 1".into(),
        });
    }

    Ok(EmbedderConfig {
        schema_version: raw.schema_version,
        provider: raw.provider,
        base_url: raw.base_url,
        model: raw.model,
        dimension: raw.dimension,
        auth,
        performance,
        retry,
        source,
    })
}

fn resolve_auth(spec: AuthConfig) -> Result<ResolvedAuth, ConfigError> {
    let env_value = env::var(&spec.env_var).unwrap_or_default();
    if env_value.is_empty() {
        return Err(ConfigError::AuthEnvUnset {
            env_var: spec.env_var,
        });
    }
    let header_value = format!("{}{env_value}", spec.value_prefix);
    Ok(ResolvedAuth {
        header_name: spec.header_name,
        env_var: spec.env_var,
        value_prefix: spec.value_prefix,
        header_value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Build a tmp tree with three roots so we can drive the
    /// priority order without touching the real process env.
    struct Harness {
        _root: tempfile::TempDir,
        cwd: PathBuf,
        env_target: PathBuf,
        home: PathBuf,
    }

    impl Harness {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("tmpdir");
            let cwd = root.path().join("cwd");
            let env_target = root.path().join("envroot");
            let home = root.path().join("home");
            fs::create_dir_all(cwd.join(".sim-flow")).unwrap();
            fs::create_dir_all(&env_target).unwrap();
            fs::create_dir_all(home.join(".sim-flow")).unwrap();
            Self {
                _root: root,
                cwd,
                env_target,
                home,
            }
        }

        fn write_project(&self, body: &str) -> PathBuf {
            let p = self.cwd.join(".sim-flow").join("embedder.toml");
            fs::write(&p, body).unwrap();
            p
        }

        fn write_env_pointer(&self, body: &str) -> PathBuf {
            let p = self.env_target.join("embedder.toml");
            fs::write(&p, body).unwrap();
            p
        }

        fn write_home(&self, body: &str) -> PathBuf {
            let p = self.home.join(".sim-flow").join("embedder.toml");
            fs::write(&p, body).unwrap();
            p
        }

        fn ctx(&self) -> LoadContext {
            LoadContext {
                project_cwd: Some(self.cwd.clone()),
                env_pointer: Some(self.env_target.join("embedder.toml")),
                home_dir: Some(self.home.clone()),
            }
        }
    }

    const MINIMAL_TOML: &str = r#"
schema_version = 1
provider = "openai-compat"
base_url = "http://localhost:11434/v1"
model = "nomic-embed-text"
dimension = 768
"#;

    #[test]
    fn project_file_wins_over_env_and_home() {
        let h = Harness::new();
        let project_path = h.write_project(MINIMAL_TOML);
        h.write_env_pointer(
            r#"schema_version = 1
provider = "openai-compat"
base_url = "http://from-env/v1"
model = "from-env"
dimension = 256
"#,
        );
        h.write_home(
            r#"schema_version = 1
provider = "openai-compat"
base_url = "http://from-home/v1"
model = "from-home"
dimension = 512
"#,
        );

        let cfg = EmbedderConfig::load_with(&h.ctx()).expect("load");
        assert_eq!(cfg.model, "nomic-embed-text");
        assert_eq!(cfg.dimension, 768);
        match cfg.source {
            ConfigSource::ProjectFile(p) => assert_eq!(p, project_path),
            other => panic!("expected ProjectFile, got {other:?}"),
        }
    }

    #[test]
    fn env_pointer_wins_over_home_when_no_project_file() {
        let h = Harness::new();
        let env_path = h.write_env_pointer(MINIMAL_TOML);
        h.write_home(
            r#"schema_version = 1
provider = "openai-compat"
base_url = "http://from-home/v1"
model = "from-home"
dimension = 512
"#,
        );

        let cfg = EmbedderConfig::load_with(&h.ctx()).expect("load");
        assert_eq!(cfg.model, "nomic-embed-text");
        match cfg.source {
            ConfigSource::EnvOverride(p) => assert_eq!(p, env_path),
            other => panic!("expected EnvOverride, got {other:?}"),
        }
    }

    #[test]
    fn home_default_used_when_no_others() {
        let h = Harness::new();
        let home_path = h.write_home(MINIMAL_TOML);

        let cfg = EmbedderConfig::load_with(&h.ctx()).expect("load");
        assert_eq!(cfg.model, "nomic-embed-text");
        match cfg.source {
            ConfigSource::UserDefault(p) => assert_eq!(p, home_path),
            other => panic!("expected UserDefault, got {other:?}"),
        }
    }

    #[test]
    fn not_found_when_no_paths_exist() {
        let h = Harness::new();
        let err = EmbedderConfig::load_with(&h.ctx()).expect_err("no file");
        assert!(matches!(err, ConfigError::NotFound), "got {err:?}");
    }

    #[test]
    fn defaults_apply_when_sections_absent() {
        let h = Harness::new();
        h.write_project(MINIMAL_TOML);
        let cfg = EmbedderConfig::load_with(&h.ctx()).expect("load");
        assert_eq!(cfg.performance.batch_size, 32);
        assert_eq!(cfg.performance.max_input_chars, 8000);
        assert_eq!(cfg.performance.timeout_secs, 30);
        assert_eq!(cfg.retry.max_attempts, 3);
        assert_eq!(cfg.retry.initial_delay_ms, 200);
        assert_eq!(cfg.retry.max_delay_ms, 2000);
        assert!(cfg.auth.is_none());
    }

    #[test]
    fn auth_block_resolves_from_env() {
        // SAFETY: this test sets a process-wide env var.
        // `cargo test` runs threads in the same process so there's
        // a small risk of cross-test interference; we use a
        // uniquely-named var to keep it isolated.
        let var = "SIM_FLOW_EMBED_TEST_AUTH_TOKEN";
        // SAFETY: setting env var; only this test touches it.
        unsafe {
            std::env::set_var(var, "secret-123");
        }

        let h = Harness::new();
        h.write_project(&format!(
            r#"schema_version = 1
provider = "openai-compat"
base_url = "http://localhost:11434/v1"
model = "nomic-embed-text"
dimension = 768
[auth]
header_name = "Authorization"
env_var = "{var}"
value_prefix = "Bearer "
"#
        ));
        let cfg = EmbedderConfig::load_with(&h.ctx()).expect("load");
        let auth = cfg.auth.expect("auth present");
        assert_eq!(auth.header_name, "Authorization");
        assert_eq!(auth.header_value, "Bearer secret-123");

        // SAFETY: cleanup.
        unsafe {
            std::env::remove_var(var);
        }
    }

    #[test]
    fn auth_missing_env_is_hard_error() {
        let var = "SIM_FLOW_EMBED_TEST_MISSING_AUTH";
        // SAFETY: ensure the var is unset for the duration of the
        // test.
        unsafe {
            std::env::remove_var(var);
        }
        let h = Harness::new();
        h.write_project(&format!(
            r#"schema_version = 1
provider = "openai-compat"
base_url = "http://localhost:11434/v1"
model = "nomic-embed-text"
dimension = 768
[auth]
env_var = "{var}"
"#
        ));
        let err = EmbedderConfig::load_with(&h.ctx()).expect_err("must error");
        match err {
            ConfigError::AuthEnvUnset { env_var } => assert_eq!(env_var, var),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_provider() {
        let h = Harness::new();
        h.write_project(
            r#"schema_version = 1
provider = "made-up"
base_url = "http://localhost:11434/v1"
model = "nomic-embed-text"
dimension = 768
"#,
        );
        let err = EmbedderConfig::load_with(&h.ctx()).expect_err("unknown provider");
        match err {
            ConfigError::Invalid { message, .. } => {
                assert!(message.contains("made-up"), "msg = {message}")
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn rejects_unsupported_schema_version() {
        let h = Harness::new();
        h.write_project(
            r#"schema_version = 99
provider = "openai-compat"
base_url = "http://localhost:11434/v1"
model = "nomic-embed-text"
dimension = 768
"#,
        );
        let err = EmbedderConfig::load_with(&h.ctx()).expect_err("schema mismatch");
        assert!(matches!(err, ConfigError::Invalid { .. }));
    }

    #[test]
    fn load_explicit_bypasses_priority() {
        let h = Harness::new();
        // Project file points at one config; explicit at a different
        // file. Explicit wins.
        h.write_project(MINIMAL_TOML);
        let explicit_path = h._root.path().join("explicit.toml");
        std::fs::write(
            &explicit_path,
            r#"schema_version = 1
provider = "openai-compat"
base_url = "http://explicit/v1"
model = "explicit-model"
dimension = 1024
"#,
        )
        .unwrap();
        let cfg = EmbedderConfig::load_explicit(&explicit_path).expect("load explicit");
        assert_eq!(cfg.model, "explicit-model");
        assert_eq!(cfg.dimension, 1024);
        match cfg.source {
            ConfigSource::Explicit(p) => assert_eq!(p, explicit_path),
            other => panic!("expected Explicit, got {other:?}"),
        }
    }
}
