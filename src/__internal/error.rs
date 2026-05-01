//! Crate-wide error and result aliases.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("toml parse error in {path}: {source}")]
    TomlParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("toml serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    #[error("state error: {0}")]
    State(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("client error: {0}")]
    Client(String),

    #[error("gate failure: {0}")]
    Gate(String),

    #[error("instruction file not found: {0}")]
    InstructionMissing(PathBuf),

    #[error("foundation root could not be resolved: {0}")]
    FoundationRoot(String),

    #[error("invalid step id: {0}")]
    InvalidStep(String),
}

pub type Result<T> = std::result::Result<T, Error>;
