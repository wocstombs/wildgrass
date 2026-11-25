//! Background `mpv` player process management and JSON IPC control.

mod ipc;
mod mpv_player;

/// Internal helper for Windows capture tests.
#[cfg(all(windows, test))]
pub(crate) use self::mpv_player::mpv_programs;
pub use self::mpv_player::{MpvPlayer, PlayerEvent};
