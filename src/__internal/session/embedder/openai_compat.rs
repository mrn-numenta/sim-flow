//! Placeholder for the v1 `OpenAiCompatEmbedder`. Filled in by
//! milestone 3.3.

use std::fmt;

/// Constructor error. Concrete shape lands in milestone 3.3.
#[derive(Debug)]
pub enum ConstructError {
    /// Placeholder; real variants land in milestone 3.3.
    Placeholder,
}

impl fmt::Display for ConstructError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstructError::Placeholder => write!(f, "embedder construct: placeholder"),
        }
    }
}

impl std::error::Error for ConstructError {}

/// The single v1 `EmbeddingClient` implementation. Concrete fields
/// and the rig wiring land in milestone 3.3.
#[derive(Debug)]
pub struct OpenAiCompatEmbedder;
