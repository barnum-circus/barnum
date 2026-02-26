//! Transport abstraction for file-based and socket-based communication.
//!
//! The `Transport` enum provides a unified interface for reading and writing
//! data, whether through filesystem operations or socket I/O.

use interprocess::local_socket::Stream;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Communication transport for agents and daemon.
///
/// Both the daemon and agents use this enum to abstract over the communication
/// mechanism. Currently file-based transport is fully supported; socket-based
/// transport will be added later.
pub enum Transport {
    /// Filesystem-based transport using a directory.
    Directory(PathBuf),
    /// Socket-based transport for direct RPC.
    Socket(Stream),
}

// Manual Debug impl because Stream doesn't implement Debug
impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Transport::Directory(path) => f.debug_tuple("Directory").field(path).finish(),
            Transport::Socket(_) => f.debug_tuple("Socket").field(&"...").finish(),
        }
    }
}

impl Transport {
    /// Read content from this transport.
    ///
    /// For directory-based transports, reads from the specified file.
    /// For socket-based transports, reads from the socket (filename is ignored).
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or if socket I/O fails.
    pub fn read(&self, filename: &str) -> io::Result<String> {
        match self {
            Transport::Directory(path) => fs::read_to_string(path.join(filename)),
            Transport::Socket(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "socket read not yet implemented",
            )),
        }
    }

    /// Write content to this transport.
    ///
    /// For directory-based transports, writes atomically (write to temp file in /tmp, then rename).
    /// The temp file is in /tmp so the watcher doesn't see spurious events for it.
    /// For socket-based transports, sends over the socket (filename is ignored).
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written or if socket I/O fails.
    pub fn write(&self, filename: &str, content: &str) -> io::Result<()> {
        match self {
            Transport::Directory(path) => {
                // Ensure directory exists (daemon creates agent directories on first write)
                fs::create_dir_all(path)?;
                // Write atomically: write to temp file in same directory, then rename.
                // Temp file must be on the same filesystem as target for rename to work.
                let target = path.join(filename);
                let temp = path.join(format!(".{}.{}.tmp", filename, uuid::Uuid::new_v4()));
                fs::write(&temp, content)?;
                fs::rename(&temp, &target)
            }
            Transport::Socket(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "socket write not yet implemented",
            )),
        }
    }

    /// Get the path for directory-based transports.
    ///
    /// Returns `None` for socket-based transports.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Transport::Directory(path) => Some(path),
            Transport::Socket(_) => None,
        }
    }
}
