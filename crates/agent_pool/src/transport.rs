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
    /// Files are written as `<dir>/<filename>`.
    Directory(PathBuf),
    /// Flat file transport using a directory and UUID prefix.
    /// Files are written as `<dir>/<uuid>.<filename>`.
    FlatFile {
        /// The directory containing the flat files.
        dir: PathBuf,
        /// The UUID prefix for file names.
        uuid: String,
    },
    /// Socket-based transport for direct RPC.
    Socket(Stream),
}

// Manual Debug impl because Stream doesn't implement Debug
impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Transport::Directory(path) => f.debug_tuple("Directory").field(path).finish(),
            Transport::FlatFile { dir, uuid } => f
                .debug_struct("FlatFile")
                .field("dir", dir)
                .field("uuid", uuid)
                .finish(),
            Transport::Socket(_) => f.debug_tuple("Socket").field(&"...").finish(),
        }
    }
}

impl Transport {
    /// Read content from this transport.
    ///
    /// For directory-based transports, reads from the specified file.
    /// For flat file transports, reads from `<uuid>.<filename>`.
    /// For socket-based transports, reads from the socket (filename is ignored).
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or if socket I/O fails.
    pub fn read(&self, filename: &str) -> io::Result<String> {
        match self {
            Transport::Directory(path) => fs::read_to_string(path.join(filename)),
            Transport::FlatFile { dir, uuid } => {
                fs::read_to_string(dir.join(format!("{uuid}.{filename}")))
            }
            Transport::Socket(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "socket read not yet implemented",
            )),
        }
    }

    /// Write content to this transport.
    ///
    /// For directory-based transports, writes atomically (write to temp file, then rename).
    /// For flat file transports, writes to `<uuid>.<filename>`.
    /// For socket-based transports, sends over the socket (filename is ignored).
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written or if socket I/O fails.
    pub fn write(&self, filename: &str, content: &str) -> io::Result<()> {
        match self {
            Transport::Directory(path) => {
                // Write atomically: write to temp file in same directory, then rename.
                // Temp file must be on the same filesystem as target for rename to work.
                let target = path.join(filename);
                let temp = path.join(format!(".{}.{}.tmp", filename, uuid::Uuid::new_v4()));
                fs::write(&temp, content)?;
                fs::rename(&temp, &target)
            }
            Transport::FlatFile { dir, uuid } => {
                let target_name = format!("{uuid}.{filename}");
                let target = dir.join(&target_name);
                let temp = dir.join(format!(".{}.{}.tmp", target_name, uuid::Uuid::new_v4()));
                fs::write(&temp, content)?;
                fs::rename(&temp, &target)
            }
            Transport::Socket(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "socket write not yet implemented",
            )),
        }
    }

    /// Get the base path for file-based transports.
    ///
    /// Returns `None` for socket-based transports.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Transport::Directory(path) => Some(path),
            Transport::FlatFile { dir, .. } => Some(dir),
            Transport::Socket(_) => None,
        }
    }

    /// Get the UUID for flat file transports.
    ///
    /// Returns `None` for other transport types.
    pub fn uuid(&self) -> Option<&str> {
        match self {
            Transport::FlatFile { uuid, .. } => Some(uuid),
            _ => None,
        }
    }
}
