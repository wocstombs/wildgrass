//! Shared initialization and readiness polling for `mpv` IPC connections.

use std::process::Child;
use std::time::{Duration, Instant};
use std::{io, thread};

use uuid::Uuid;

/// Timeout waiting for `mpv` to create its IPC endpoint.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(3);

/// Generates a unique IPC endpoint name for each session.
pub(super) fn unique_name() -> String {
    let id = Uuid::new_v4().simple().to_string();
    format!("wildgrass-mpv-{}", &id[..12])
}

/// Polls `open` until `mpv` creates the IPC endpoint or exits with an error.
pub(super) fn wait_for<T>(
    child: &mut Child,
    mut open: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if let Ok(connection) = open() {
            return Ok(connection);
        }
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "mpv exited at startup ({status})"
            )));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "mpv did not open its control socket within {} seconds",
                STARTUP_TIMEOUT.as_secs()
            )));
        }
        thread::sleep(Duration::from_millis(20));
    }
}
