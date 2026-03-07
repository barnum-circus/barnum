//! Pool ID management.
//!
//! Pools live in `<root>/pools/<id>/` with short, memorable IDs.
//! Default root on Unix: `/tmp/agent_pool/`
//! Default root on Windows: `%TEMP%\agent_pool\`

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Default directory name for the root.
const DEFAULT_ROOT_DIR: &str = "agent_pool";

/// Subdirectory within root where pools live.
const POOLS_DIR: &str = "pools";

/// Get the default root directory.
///
/// Uses /tmp explicitly on Unix to ensure atomic writes (which also use /tmp)
/// are on the same filesystem.
#[must_use]
pub fn default_root() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/tmp").join(DEFAULT_ROOT_DIR)
    }
    #[cfg(not(unix))]
    {
        std::env::temp_dir().join(DEFAULT_ROOT_DIR)
    }
}

/// Get the pools directory within the root.
#[must_use]
pub fn pools_dir(root: &Path) -> PathBuf {
    root.join(POOLS_DIR)
}

/// Length of generated pool IDs.
const ID_LENGTH: usize = 8;

/// Characters used for ID generation (lowercase alphanumeric, no confusing chars).
const ID_CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// Generate a short random pool ID.
#[must_use]
#[expect(clippy::cast_possible_truncation)] // Intentional truncation for randomness
pub fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Simple random using time + process id as seed
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0) as u64
        ^ u64::from(std::process::id());

    let mut id = String::with_capacity(ID_LENGTH);
    let mut state = seed;

    for _ in 0..ID_LENGTH {
        // Simple xorshift for randomness
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;

        let idx = (state as usize) % ID_CHARS.len();
        id.push(ID_CHARS[idx] as char);
    }

    id
}

/// Get the path for a pool ID within the given root.
///
/// Returns `<root>/pools/<id>`.
#[must_use]
pub fn id_to_path(root: &Path, id: &str) -> PathBuf {
    pools_dir(root).join(id)
}

/// Information about a pool.
#[derive(Debug)]
pub struct PoolInfo {
    /// Pool ID.
    pub id: String,
    /// Full path to the pool directory.
    pub path: PathBuf,
    /// Whether the pool is currently running.
    pub running: bool,
}

/// List all pools in the given root directory.
///
/// # Errors
///
/// Returns an error if the pools directory cannot be read.
pub fn list_pools(root: &Path) -> io::Result<Vec<PoolInfo>> {
    let pools_path = pools_dir(root);
    if !pools_path.exists() {
        return Ok(Vec::new());
    }

    let mut pools = Vec::new();

    let entries = fs::read_dir(&pools_path).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "[E065] failed to read pools dir {}: {e}",
                pools_path.display()
            ),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "[E066] failed to read pool entry in {}: {e}",
                    pools_path.display()
                ),
            )
        })?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let Some(id) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        let running = is_pool_running(&path);

        pools.push(PoolInfo {
            id: id.to_string(),
            path,
            running,
        });
    }

    Ok(pools)
}

/// Check if a pool is running by verifying the lock file PID is alive.
#[cfg(unix)]
fn is_pool_running(pool_path: &std::path::Path) -> bool {
    use std::fs;

    let lock_path = pool_path.join(crate::constants::LOCK_FILE);

    let Ok(pid_str) = fs::read_to_string(&lock_path) else {
        return false;
    };

    let Ok(pid) = pid_str.trim().parse::<u32>() else {
        return false;
    };

    // Check if the process is still alive using kill -0
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Check if a pool is running (Windows stub - always returns false).
#[cfg(not(unix))]
fn is_pool_running(_pool_path: &std::path::Path) -> bool {
    // On Windows, we'd need different logic to check process status.
    false
}

/// Resolve a pool reference (ID or path) to a full path.
///
/// If the reference looks like a path (contains `/` or `\`), returns it as-is.
/// Otherwise, treats it as an ID and converts to `<root>/pools/<id>`.
#[must_use]
pub fn resolve_pool(root: &Path, reference: &str) -> PathBuf {
    if reference.contains('/') || reference.contains('\\') {
        PathBuf::from(reference)
    } else {
        id_to_path(root, reference)
    }
}
