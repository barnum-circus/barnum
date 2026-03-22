//! Generic type for content that can be inline or linked to a file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

/// Content that can be inline or linked to a file.
///
/// In config files:
/// - `{"kind": "Inline", "value": <content>}` → content provided directly in the config
/// - `{"kind": "Link", "path": "file.md"}` → content loaded from a file (path relative to the config file)
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum MaybeLinked<T> {
    /// Inline content.
    Inline {
        /// The content value, provided directly in the config file.
        value: T,
    },
    /// Link to a file whose contents will be loaded at runtime.
    Link {
        /// Relative path to the file (resolved relative to the config file's directory).
        path: String,
    },
}

impl<T: Default> Default for MaybeLinked<T> {
    fn default() -> Self {
        Self::Inline {
            value: T::default(),
        }
    }
}

impl<T> MaybeLinked<T> {
    /// Get the inline value if this is inline content.
    #[must_use]
    pub const fn as_inline(&self) -> Option<&T> {
        match self {
            Self::Inline { value } => Some(value),
            Self::Link { .. } => None,
        }
    }

    /// Get the link path if this is a link.
    #[must_use]
    pub fn as_link(&self) -> Option<&str> {
        match self {
            Self::Inline { .. } => None,
            Self::Link { path } => Some(path),
        }
    }

    /// Resolve to the inner value, reading from file if linked.
    ///
    /// The `read_file` function is called with the resolved path to read the file content.
    ///
    /// # Errors
    ///
    /// Returns an error if the linked file cannot be read.
    pub fn resolve<U, F>(self, base_path: &Path, read_file: F) -> io::Result<U>
    where
        F: FnOnce(&Path) -> io::Result<U>,
        T: Into<U>,
    {
        match self {
            Self::Inline { value } => Ok(value.into()),
            Self::Link { path: link_path } => {
                let resolved = base_path.join(&link_path);
                read_file(&resolved).map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("failed to read '{}': {e}", resolved.display()),
                    )
                })
            }
        }
    }
}
