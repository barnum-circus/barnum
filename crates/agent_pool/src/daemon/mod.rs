//! Daemon implementation.
//!
//! - **core**: Pure state machine - no I/O, fully testable
//! - **io**: I/O operations - filesystem, timers, effect execution
//! - **wiring**: Spawns threads, creates channels, runs the main loop

mod core;
mod io;
mod wiring;

pub use wiring::{DaemonConfig, DaemonHandle, run, run_with_config, spawn, spawn_with_config};
