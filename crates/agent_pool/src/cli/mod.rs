//! CLI commands for interacting with the agent pool daemon.

mod stop;
mod submit;
mod submit_file;

pub use stop::stop;
pub use submit::submit;
pub use submit_file::{cleanup_submission, submit_file};
