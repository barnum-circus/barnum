//! Domain types for agent pool.
//!
//! These newtypes add semantic clarity to commonly confused string types,
//! preventing accidental parameter reordering or misuse.

use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;

/// Macro to define a newtype wrapper around String with common trait implementations.
macro_rules! define_string_id {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident;
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        $vis struct $name(String);

        impl $name {
            /// Create a new identifier.
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Get the identifier as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                self.0 == *other
            }
        }
    };
}

define_string_id! {
    /// An agent's name within a pool.
    ///
    /// Agents register with a unique name (e.g., "claude-1", "worker-a").
    /// This newtype prevents confusion with other string parameters like pool IDs.
    ///
    /// Note: This is distinct from the internal `AgentId` in `daemon::core`, which
    /// is a numeric ID assigned by the daemon. This type represents the user-facing
    /// name that agents choose when they register.
    pub struct AgentName;
}

define_string_id! {
    /// A pool identifier.
    ///
    /// Pool IDs are short, memorable strings (e.g., "abc12345") that resolve
    /// to filesystem paths like `/tmp/gsd/<id>/`.
    ///
    /// This newtype distinguishes pool IDs from agent IDs and other strings.
    pub struct PoolId;
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn agent_name_equality() {
        let name = AgentName::new("claude-1");
        assert_eq!(name, "claude-1");
        assert_eq!(name.as_str(), "claude-1");
    }

    #[test]
    fn pool_id_equality() {
        let id = PoolId::new("abc12345");
        assert_eq!(id, "abc12345");
        assert_eq!(id.as_str(), "abc12345");
    }

    #[test]
    fn types_are_distinct() {
        let name = AgentName::new("test");
        let pool = PoolId::new("test");

        // These are different types even with same value
        // This line wouldn't compile: assert_eq!(name, pool);
        assert_eq!(name.as_str(), pool.as_str());
    }

    #[test]
    fn agent_name_serializes_transparently() {
        let name = AgentName::new("worker");
        let json = serde_json::to_string(&name).unwrap();
        assert_eq!(json, "\"worker\"");

        let parsed: AgentName = serde_json::from_str("\"worker\"").unwrap();
        assert_eq!(parsed, name);
    }
}
