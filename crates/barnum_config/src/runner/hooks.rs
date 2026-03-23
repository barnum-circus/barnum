//! Hook execution (wake scripts).

use std::io;
use std::process::Command;

use tracing::info;

/// Call a wake script before starting the runner.
pub fn call_wake_script(script: &str) -> io::Result<()> {
    info!(script, "calling wake script");
    let status = Command::new("sh").arg("-c").arg(script).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "[E019] wake script failed with status: {status}"
        )))
    }
}
