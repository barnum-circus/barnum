//! Domain types for GSD config.
//!
//! These newtypes add semantic clarity to commonly confused string types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;

/// Macro to define a newtype wrapper around String with common trait implementations.
///
/// This reduces boilerplate when defining multiple string-based identifiers.
macro_rules! define_string_id {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident;
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
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
    /// A step name identifier.
    ///
    /// In configs, this is the `name` field of a step.
    /// In tasks, this is the `kind` field (serialized as "kind" for compatibility).
    ///
    /// Using a newtype makes it clear that step names and arbitrary strings
    /// are different concepts, preventing accidental misuse.
    pub struct StepName;
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn step_name_equality() {
        let name = StepName::new("Analyze");
        assert_eq!(name, "Analyze");
        assert_eq!(name, "Analyze".to_string());
        assert_eq!(name.as_str(), "Analyze");
    }

    #[test]
    fn step_name_serializes_transparently() {
        let name = StepName::new("Test");
        let json = serde_json::to_string(&name).unwrap();
        assert_eq!(json, "\"Test\"");

        let parsed: StepName = serde_json::from_str("\"Test\"").unwrap();
        assert_eq!(parsed, name);
    }

    #[test]
    fn step_name_in_hashmap() {
        use std::collections::HashMap;

        let mut map: HashMap<StepName, i32> = HashMap::new();
        map.insert(StepName::new("A"), 1);

        // Can lookup with &str via Borrow
        assert_eq!(map.get("A"), Some(&1));
    }
}
