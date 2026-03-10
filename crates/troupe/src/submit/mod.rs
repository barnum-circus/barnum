//! Task submission operations for interacting with the agent pool daemon.

mod file;
mod payload;
mod socket;

pub use file::{submit_file, submit_file_with_timeout};
pub use payload::Payload;
pub use socket::submit;
