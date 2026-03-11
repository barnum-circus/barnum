//! Troupe - agent pool daemon for managing workers.
//!
//! Troupe communicates with:
//! - **Submitters** via Unix socket or file-based submission
//! - **Agents** via filesystem watchers (`task.json`, `response.json`)
//!
//! See `protocols/AGENT_PROTOCOL.md` for details on the agent file protocol.
//!
//! # Usage
//!
//! ```ignore
//! troupe::run_with_config(&root, config)?;  // Never returns on success
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
pub use pool::{default_root, generate_id, id_to_path, list_pools, pools_dir, resolve_pool};
pub use response::Response;
pub use stop::stop;
pub use submit::{Payload, submit, submit_file, submit_file_with_timeout};
pub(crate) use transport::Transport;
pub use verified_watcher::{VerifiedWatcher, WaitError};
pub use worker::{
    ReadyGuard, TaskAssignment, announce_ready, wait_for_assignment, wait_for_task, write_response,
};
