//! Session and learned-memory persistence boundary.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Stored session summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session identifier.
    pub id: String,
    /// Human-readable summary.
    pub summary: String,
}

/// Memory store skeleton.
#[derive(Clone, Debug)]
pub struct MemoryStore {
    path: PathBuf,
}

impl MemoryStore {
    /// Creates a memory store pointing at a SQLite database path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the configured database path.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}
