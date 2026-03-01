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
mod fs;
mod lock;
mod pool;
mod response;
mod transport;
mod worker;

// Grouped modules
mod daemon;
mod submit;

pub use constants::{AGENTS_DIR, RESPONSE_FILE, STATUS_FILE, TASK_FILE};
pub use daemon::{DaemonConfig, run_with_config};
pub use lock::is_daemon_running;
pub use pool::{
    cleanup_stopped, default_pool_root, generate_id, id_to_path, list_pools, resolve_pool,
};
pub use response::Response;
pub use submit::{
    Payload, stop, submit, submit_file, submit_file_with_timeout, wait_for_pool_ready,
};
pub use transport::Transport;
pub use worker::{AgentEvent, create_watcher, verify_watcher_sync, wait_for_task};
