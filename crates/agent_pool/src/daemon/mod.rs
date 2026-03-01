//! Daemon implementation.
//!
//! - **`core`**: Pure state machine - no I/O, fully testable
//! - **`io`**: I/O operations - filesystem, timers, effect execution
//! - **`path_category`**: Categorizes filesystem paths for event handling
//! - **`wiring`**: Spawns threads, creates channels, runs the main loop

mod core;
mod io;
mod path_category;
mod wiring;

pub use wiring::{DaemonConfig, run_with_config};
