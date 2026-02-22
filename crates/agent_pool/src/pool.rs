//! Pool ID management.
//!
//! Pools live in `/tmp/gsd/<id>/` with short, memorable IDs.

use std::fs;
use std::io;
use std::path::PathBuf;

/// Base directory for all pools.
const POOLS_BASE: &str = "/tmp/gsd";

/// Length of generated pool IDs.
const ID_LENGTH: usize = 8;

/// Characters used for ID generation (lowercase alphanumeric, no confusing chars).
const ID_CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// Generate a short random pool ID.
#[must_use]
#[allow(clippy::cast_possible_truncation)] // Intentional truncation for randomness
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

/// Get the path for a pool ID.
#[must_use]
pub fn id_to_path(id: &str) -> PathBuf {
    PathBuf::from(POOLS_BASE).join(id)
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

/// List all pools in the pools directory.
///
/// # Errors
///
/// Returns an error if the pools directory cannot be read.
pub fn list_pools() -> io::Result<Vec<PoolInfo>> {
    let base = PathBuf::from(POOLS_BASE);

    if !base.exists() {
        return Ok(Vec::new());
    }

    let mut pools = Vec::new();

    for entry in fs::read_dir(&base)? {
        let entry = entry?;
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

/// Check if a pool is running by testing if its socket is connectable.
#[cfg(unix)]
fn is_pool_running(pool_path: &std::path::Path) -> bool {
    use std::os::unix::net::UnixStream;

    let socket_path = pool_path.join(crate::constants::SOCKET_NAME);

    if !socket_path.exists() {
        return false;
    }

    // Try to connect - if successful, pool is running
    UnixStream::connect(&socket_path).is_ok()
}

/// Check if a pool is running (Windows stub - always returns false).
#[cfg(not(unix))]
fn is_pool_running(_pool_path: &std::path::Path) -> bool {
    // On Windows, we can't easily check if the named pipe is active
    // without more complex logic. For now, assume not running.
    false
}

/// Resolve a pool reference (ID or path) to a full path.
///
/// If the input looks like a path (contains `/`), returns it as-is.
/// Otherwise, treats it as an ID and converts to `/tmp/gsd/<id>`.
#[must_use]
pub fn resolve_pool(reference: &str) -> PathBuf {
    if reference.contains('/') {
        PathBuf::from(reference)
    } else {
        id_to_path(reference)
    }
}

/// Clean up stopped pools (remove directories for pools that aren't running).
///
/// # Errors
///
/// Returns an error if cleanup fails.
pub fn cleanup_stopped() -> io::Result<usize> {
    let pools = list_pools()?;
    let mut cleaned = 0;

    for pool in pools {
        if !pool.running {
            fs::remove_dir_all(&pool.path)?;
            cleaned += 1;
        }
    }

    Ok(cleaned)
}
