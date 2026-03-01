//! Task submission operations for interacting with the agent pool daemon.

mod file;
mod payload;
mod socket;
mod stop;

use std::fs;
use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::constants::STATUS_FILE;
use crate::fs::VerifiedWatcher;

pub use file::{submit_file, submit_file_with_timeout};
pub use payload::Payload;
pub use socket::submit;
pub use stop::stop;

/// Wait for the agent pool daemon to become ready.
///
/// Uses a filesystem watcher with canary verification to efficiently wait
/// for the status file.
///
/// # Errors
///
/// Returns an error if the timeout is exceeded before the pool becomes ready.
pub fn wait_for_pool_ready(root: impl AsRef<Path>, timeout: Duration) -> io::Result<()> {
    let root = root.as_ref();
    let start = Instant::now();

    // Wait for directory to exist (daemon subprocess needs time to create it)
    // TODO: Use a watcher instead of spinning
    while !root.exists() {
        if start.elapsed() > timeout {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("pool directory does not exist: {}", root.display()),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }

    // Canonicalize to match FSEvents paths (e.g., /var -> /private/var on macOS)
    let root = fs::canonicalize(root)?;

    let status_path = root.join(STATUS_FILE);
    let canary_path = root.join(format!("{}.canary", Uuid::new_v4()));

    // Use VerifiedWatcher with lazy verification
    let mut watcher = VerifiedWatcher::new(&root, canary_path)?;
    watcher.wait_for(&status_path, Some(timeout))
}
