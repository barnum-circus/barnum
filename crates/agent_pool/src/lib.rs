//! Agent pool daemon for managing workers.
//!
//! The daemon communicates with:
//! - **Submitters** via Unix socket - connect, send task, receive result
//! - **Agents** via files - read `{id}.input`, write `{id}.output`
//!
//! See `AGENT_PROTOCOL.md` for details on the agent file protocol.
//!
//! # Usage
//!
//! For CLI tools that run forever:
//! ```ignore
//! agent_pool::run(&root)?;  // Never returns on success
//! ```
//!
//! For programmatic control with graceful shutdown:
//! ```ignore
//! let handle = agent_pool::spawn(&root)?;
//! // ... submit tasks ...
//! handle.shutdown()?;  // Gracefully stops the daemon
//! ```
//!
//! # Response Protocol
//!
//! The daemon returns structured JSON responses (keys lowercase, values `UpperCamelCase`):
//! ```json
//! {"kind": "Processed", "stdout": "..."}
//! {"kind": "NotProcessed", "reason": "shutdown"}
//! ```

mod constants;
mod daemon;
mod lock;
mod response;
mod stop;
mod submit;

pub use constants::{AGENTS_DIR, INPUT_EXT, OUTPUT_EXT};
pub use daemon::{DaemonHandle, run, spawn};
pub use response::{NotProcessedReason, Response, ResponseKind};
pub use stop::stop;
pub use submit::submit;

// Legacy exports for backwards compatibility during transition
#[doc(hidden)]
pub use constants::{IN_PROGRESS_FILE, NEXT_TASK_FILE, OUTPUT_FILE};
