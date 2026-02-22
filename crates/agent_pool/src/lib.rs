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
mod pool;
mod response;
mod stop;
mod submit;
mod submit_file;
mod types;

pub use constants::{AGENTS_DIR, PENDING_DIR, RESPONSE_FILE, TASK_FILE};
pub use daemon::{DaemonHandle, run, spawn};
pub use pool::{PoolInfo, cleanup_stopped, generate_id, id_to_path, list_pools, resolve_pool};
pub use response::{NotProcessedReason, Response, ResponseKind};
pub use stop::stop;
pub use submit::submit;
pub use submit_file::{cleanup_submission, submit_file};
pub use types::{AgentId, PoolId};
