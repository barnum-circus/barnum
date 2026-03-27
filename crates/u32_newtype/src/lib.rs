//! Macro for creating typed `u32` newtype wrappers.

/// Define a newtype wrapper around `u32` with standard derives.
#[macro_export]
macro_rules! u32_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(pub u32);
    };
}
