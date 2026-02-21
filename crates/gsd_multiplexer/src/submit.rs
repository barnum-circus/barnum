//! Task submission to the multiplexer daemon.

use crate::constants::SOCKET_NAME;
use interprocess::local_socket::{prelude::*, GenericFilePath, Stream};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;

/// Submit a task to the multiplexer and wait for the result.
///
/// Connects to the daemon's local socket, sends the task, and blocks
/// until the result is available.
pub fn submit(root: impl AsRef<Path>, input: &str) -> io::Result<String> {
    let socket_path = root.as_ref().join(SOCKET_NAME);
    let name = socket_path
        .to_fs_name::<GenericFilePath>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

    let mut stream = Stream::connect(name)
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;

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

    String::from_utf8(output).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
