//! Cross-platform non-blocking IPC connection to `mpv` (`--input-ipc-server`).
//!
//! Uses Unix domain sockets on Unix and named pipes on Windows. Both transport newline-delimited JSON.

mod startup;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(super) use self::unix::{Connection, Endpoint};
#[cfg(windows)]
pub(super) use self::windows::{Connection, Endpoint};
