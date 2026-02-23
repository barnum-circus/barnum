//! Agent pool daemon for managing workers.
//!
//! The daemon communicates with:
//! - **Submitters** via Unix socket or file-based submission
//! - **Agents** via filesystem polling (`task.json`, `response.json`)
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
//! # Architecture
//!
//! The daemon separates pure logic from I/O:
//! - **core**: Pure state machine - `step(state, event) -> (state, effects)`
//! - **io**: Filesystem, timers, effect execution
//!
//! # Response Protocol
//!
//! The daemon returns structured JSON responses (keys lowercase, values `UpperCamelCase`):
//! ```json
//! {"kind": "Processed", "stdout": "..."}
//! {"kind": "NotProcessed", "reason": "shutdown"}
//! ```

// Shared modules
mod constants;
mod lock;
mod pool;
mod response;
mod types;

// Grouped modules
mod cli;
mod daemon;

pub use constants::{AGENTS_DIR, PENDING_DIR, RESPONSE_FILE, TASK_FILE};
pub use daemon::{DaemonConfig, DaemonHandle, run, run_with_config, spawn, spawn_with_config};
pub use pool::{PoolInfo, cleanup_stopped, generate_id, id_to_path, list_pools, resolve_pool};
pub use response::{NotProcessedReason, Response, ResponseKind};
pub use cli::{cleanup_submission, stop, submit, submit_file};
pub use types::{AgentName, PoolId};
