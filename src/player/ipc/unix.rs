//! Unix domain socket transport for `mpv` IPC.

use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::{env, fs};

use super::startup;
use crate::paths;

const SOCKET_PATH_LIMIT: usize = 100;

/// Unix domain socket endpoint handle. Unlinks the socket path on drop.
pub struct Endpoint(PathBuf);

impl Endpoint {
    /// Creates a unique socket path under `$XDG_RUNTIME_DIR` (or temp directory if path is too long).
    pub fn unique() -> Self {
        let path = socket_path(&paths::runtime_directory());
        let _ = fs::remove_file(&path);
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn socket_path(runtime_directory: &Path) -> PathBuf {
    let short_name = format!("{}.sock", startup::unique_name());
    let candidate = runtime_directory.join(&short_name);
    if candidate.as_os_str().len() <= SOCKET_PATH_LIMIT {
        return candidate;
    }
    env::temp_dir().join(short_name)
}

pub struct Connection(UnixStream);

impl Connection {
    pub fn connect(child: &mut Child, endpoint: &Endpoint) -> io::Result<Self> {
        let stream = startup::wait_for(child, || UnixStream::connect(endpoint.path()))?;
        stream.set_nonblocking(true)?;
        Ok(Self(stream))
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;

        self.0.write_all(bytes)
    }

    pub fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        use std::io::Read;

        self.0.read(buffer)
    }
}
