//! Payload types for task submission.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How the task content is delivered.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Payload {
    /// Content is included directly in the message.
    Inline {
        /// The task content as a JSON string.
        content: String,
    },
    /// Content is at a file path; recipient reads the file.
    FileReference {
        /// Path to the file containing the task content.
        path: PathBuf,
    },
}

impl Payload {
    /// Create an inline payload with the given content.
    pub fn inline(content: impl Into<String>) -> Self {
        Self::Inline {
            content: content.into(),
        }
    }

    /// Create a file reference payload pointing to the given path.
    pub fn file_ref(path: impl Into<PathBuf>) -> Self {
        Self::FileReference { path: path.into() }
    }
}
