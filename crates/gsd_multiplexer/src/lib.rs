//! Multiplexer daemon for managing agent pools.
//!
//! The daemon communicates with:
//! - **Submitters** via Unix socket - connect, send task, receive result
//! - **Agents** via files - read `next_task`, write `output`
//!
//! See `AGENT_PROTOCOL.md` for details on the agent file protocol.

mod constants;
mod daemon;
mod lock;
mod stop;
mod submit;

pub use constants::{AGENTS_DIR, NEXT_TASK_FILE, OUTPUT_FILE};
pub use daemon::Multiplexer;
pub use stop::stop;
pub use submit::submit;
