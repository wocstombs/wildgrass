//! Terminal guard: manages terminal setup and automatic restoration for the TUI.

use std::io;

use crossterm::execute;
// Imported unqualified to avoid collision with this crate's own `terminal` module
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};

use super::opacity::Window;
use super::profile;
use super::window::{WindowAction, WindowControl};

/// RAII guard managing raw mode, alternate screen, opacity, and window properties.
/// Restores terminal and window state on drop, even when exiting on an error.
pub struct TerminalGuard {
    translucent: Vec<Window>,
    window: Option<WindowControl>,
}

impl TerminalGuard {
    pub fn acquire() -> io::Result<Self> {
        enable_raw_mode()?;
        // Dropping the guard from this point safely rolls back all acquired state
        let mut guard = Self {
            translucent: Vec::new(),
            window: None,
        };
        execute!(io::stdout(), EnterAlternateScreen)?;
        profile::set_background();
        guard.translucent = Window::make_translucent();
        // Before rendering the first frame: resize window where supported (e.g. Windows Terminal)
        guard.window = WindowControl::detect();
        Ok(guard)
    }

    /// Applies window actions (moving or layering) if supported by the platform.
    pub fn apply(&mut self, action: WindowAction) {
        if let Some(window) = &mut self.window {
            window.apply(action);
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Restores window layering and original size on drop
        drop(self.window.take());
        for window in &self.translucent {
            window.restore();
        }
        profile::reset_background();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}
