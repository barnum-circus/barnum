//! Task submission to the agent pool daemon.

use super::payload::Payload;
use crate::constants::SOCKET_NAME;
use crate::response::Response;
use interprocess::local_socket::{GenericFilePath, Stream, prelude::*};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;

/// Submit a task to the agent pool and wait for the result.
///
/// Connects to the daemon's Unix socket, sends the task, and blocks
/// until the result is available. Returns a structured [`Response`]
/// that indicates whether the task was processed successfully.
///
/// # Errors
///
/// Returns an error if:
/// - The daemon socket doesn't exist or can't be connected to
/// - Communication with the daemon fails
/// - The response contains invalid JSON
pub fn submit(root: impl AsRef<Path>, payload: &Payload) -> io::Result<Response> {
    let socket_path = root.as_ref().join(SOCKET_NAME);

    let name = socket_path
        .to_fs_name::<GenericFilePath>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

    let input = serde_json::to_string(payload)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let mut stream =
        Stream::connect(name).map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
    writeln!(stream, "{}", input.len())?;
    stream.write_all(input.as_bytes())?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);

    let mut len_line = String::new();
    reader.read_line(&mut len_line)?;
    let len: usize = len_line
        .trim()
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let mut output = vec![0u8; len];
    reader.read_exact(&mut output)?;

    let json =
        String::from_utf8(output).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    serde_json::from_str(&json).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
