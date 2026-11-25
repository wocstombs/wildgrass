//! Terminal window movement and layer positioning.
//!
//! Supports repositioning the window with arrow keys and cycling window layer
//! (always-on-top, normal, bottom) with `t`.
//!
//! Platform backends:
//! - Windows (native and WSL): uses a background PowerShell helper (`windows.rs`).
//! - Linux X11: sends EWMH requests directly to the X server via `x11rb` (`x11.rs`).
//! - Wayland: window repositioning is unsupported by standard protocols; operations no-op safely.

mod action;
mod geometry;
mod helper;
mod window_control;
mod windows;
mod x11;

pub use self::action::{Direction, WindowAction};
pub(super) use self::window_control::WindowControl;
