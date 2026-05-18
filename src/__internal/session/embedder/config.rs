//! Placeholder for the embedder config types. Filled in by
//! milestone 3.2.

use std::fmt;

/// Configuration error raised by the loader. Concrete shape lands in
/// milestone 3.2.
#[derive(Debug)]
pub enum ConfigError {
    /// Unused placeholder; replaced with the real variants in
    /// milestone 3.2.
    Placeholder,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Placeholder => write!(f, "embedder config: placeholder"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Embedder configuration record. Concrete fields land in
/// milestone 3.2.
#[derive(Debug, Clone)]
pub struct EmbedderConfig;

/// Optional `[auth]` block. Concrete fields land in milestone 3.2.
#[derive(Debug, Clone)]
pub struct AuthConfig;

/// `[performance]` block. Concrete fields land in milestone 3.2.
#[derive(Debug, Clone)]
pub struct PerformanceConfig;

/// `[retry]` block. Concrete fields land in milestone 3.2.
#[derive(Debug, Clone)]
pub struct RetryConfig;
