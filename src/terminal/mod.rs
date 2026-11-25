//! Terminal environment management: raw mode, alternate screen, opacity, and window control.
//!
//! Handles terminal setup and cleanup: entering raw mode, toggling the alternate screen,
//! applying window opacity under Hyprland or kitty, and moving or layering windows where
//! supported. On Windows, it handles Windows Terminal profile switching, sizing, and background colors.

#[cfg(not(windows))]
mod no_profile;
mod opacity;
#[cfg(windows)]
mod profile;
mod size;
mod terminal_guard;
mod window;
mod windows_terminal;

pub use self::profile::reopen_in_profile;
pub use self::terminal_guard::TerminalGuard;
pub use self::window::{Direction, WindowAction};

/// Platform stubs on non-Windows systems where profile switching is not used.
#[cfg(not(windows))]
use self::no_profile as profile;

/// Command-line flag passed when relaunching inside Windows Terminal to prevent infinite relaunch loops.
pub const REOPENED: &str = "--reopened";
