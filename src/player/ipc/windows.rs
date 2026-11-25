//! Windows named pipe transport for `mpv` IPC.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::ptr;

use windows_sys::Win32::System::Pipes::PeekNamedPipe;

use super::startup;

/// Named pipe IPC endpoint.
pub struct Endpoint(PathBuf);

impl Endpoint {
    pub fn unique() -> Self {
        Self(PathBuf::from(format!(
            r"\\.\pipe\{}",
            startup::unique_name()
        )))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// Named pipe client connection using `PeekNamedPipe` for non-blocking reads.
pub struct Connection(File);

impl Connection {
    pub fn connect(child: &mut Child, endpoint: &Endpoint) -> io::Result<Self> {
        let open = || {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(endpoint.path())
        };
        Ok(Self(startup::wait_for(child, open)?))
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;

        self.0.write_all(bytes)
    }

    /// What has arrived, `WouldBlock` when nothing has, and 0 once `mpv` has closed the pipe.
    #[expect(
        unsafe_code,
        reason = "only PeekNamedPipe tells what has arrived without waiting, and windows-sys \
                  offers it only as an unsafe call"
    )]
    pub fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        use std::io::Read;
        use std::os::windows::io::AsRawHandle;

        let mut arrived = 0u32;
        // SAFETY: the handle is the open pipe's, and only `arrived` is written to; the other
        // pointers are null, which asks for nothing else
        let peeked = unsafe {
            PeekNamedPipe(
                self.0.as_raw_handle(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                &mut arrived,
                ptr::null_mut(),
            )
        };
        if peeked == 0 {
            let error = io::Error::last_os_error();
            return match error.kind() {
                io::ErrorKind::BrokenPipe => Ok(0),
                _ => Err(error),
            };
        }
        if arrived == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let count = buffer.len().min(arrived as usize);
        self.0.read(&mut buffer[..count])
    }
}
