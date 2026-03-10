//! Generic type for content that can be inline or linked to a file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

/// Content that can be inline or linked to a file.
///
/// In config files:
/// - `{"inline": <value>}` → inline content
/// - `{"link": "path"}` → link to file
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MaybeLinked<T> {
    /// Inline content.
    Inline {
        /// The inline content.
        inline: T,
    },
    /// Link to a file.
    Link {
        /// Path to the file.
        link: String,
    },
}

impl<T: Default> Default for MaybeLinked<T> {
    fn default() -> Self {
        Self::Inline {
            inline: T::default(),
        }
    }
}

impl<T> MaybeLinked<T> {
    /// Get the inline value if this is inline content.
    #[must_use]
    pub const fn as_inline(&self) -> Option<&T> {
        match self {
            Self::Inline { inline } => Some(inline),
            Self::Link { .. } => None,
        }
    }

    /// Get the link path if this is a link.
    #[must_use]
    pub fn as_link(&self) -> Option<&str> {
        match self {
            Self::Inline { .. } => None,
            Self::Link { link } => Some(link),
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
            Self::Inline { inline } => Ok(inline.into()),
            Self::Link { link } => {
                let path = base_path.join(&link);
                read_file(&path).map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("failed to read '{}': {e}", path.display()),
                    )
                })
            }
        }
    }
}
