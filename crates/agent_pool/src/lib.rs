//! Agent pool daemon for managing workers.
//!
//! The daemon communicates with:
//! - **Submitters** via Unix socket or file-based submission
//! - **Agents** via filesystem polling (`task.json`, `response.json`)
//!
//! See `protocols/AGENT_PROTOCOL.md` for details on the agent file protocol.
//!
//! # Usage
//!
//! ```ignore
//! agent_pool::run_with_config(&root, config)?;  // Never returns on success
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
mod stop;
mod transport;
mod verified_watcher;
mod worker;

// Grouped modules
mod daemon;
mod submit;

pub use constants::{
    AGENTS_DIR, RESPONSE_FILE, STATUS_FILE, STATUS_READY, STATUS_STOP, TASK_FILE, response_path,
};
pub use daemon::{DaemonConfig, run_with_config};
pub use lock::is_daemon_running;
pub use pool::{default_pool_root, generate_id, id_to_path, list_pools, resolve_pool};
pub use response::Response;
pub use stop::stop;
pub use submit::{Payload, submit, submit_file, submit_file_with_timeout, wait_for_pool_ready};
pub(crate) use transport::Transport;
pub use verified_watcher::VerifiedWatcher;
pub use worker::{TaskAssignment, wait_for_task, write_response};
