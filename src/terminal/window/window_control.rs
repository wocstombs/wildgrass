//! Window control backend selection and lifecycle management.

use super::action::{Direction, Layer, WindowAction};
use super::helper;
use super::windows::Windows;
use super::x11::{self, X11};

/// Controls terminal window positioning and z-order. Restores default layer and dimensions on drop.
pub(in crate::terminal) struct WindowControl {
    backend: Backend,
    /// Active z-order layer.
    layer: Layer,
}

impl WindowControl {
    /// Detects and connects to the window management backend for the current platform.
    /// Prefers Windows/WSL backend first (since WSLg sets `DISPLAY`), followed by Linux X11.
    pub(in crate::terminal) fn detect() -> Option<Self> {
        let backend = if cfg!(windows) || helper::is_wsl() {
            Backend::Windows(Windows::start())
        } else if x11::is_x11_session() {
            Backend::X11(Box::new(X11::connect()?))
        } else {
            return None;
        };
        Some(Self {
            backend,
            layer: Layer::Normal,
        })
    }

    pub(in crate::terminal) fn apply(&mut self, action: WindowAction) {
        match action {
            WindowAction::Move(direction) => self.backend.move_window(direction),
            WindowAction::CycleLayer => {
                self.layer = self.layer.next();
                self.backend.set_layer(self.layer);
            }
        }
    }
}

impl Drop for WindowControl {
    fn drop(&mut self) {
        // Restore normal layer and window dimensions on exit
        self.backend.put_back(self.layer);
    }
}

/// Window management backend. Non-critical operations fail silently without panicking.
enum Backend {
    /// Windows (native or WSL) via PowerShell helper.
    Windows(Windows),
    /// An X11 session, through the X server: moves and layers.
    X11(Box<X11>),
}

impl Backend {
    fn move_window(&mut self, direction: Direction) {
        match self {
            Self::Windows(windows) => windows.move_window(direction),
            Self::X11(x11) => {
                let _ = x11.move_window(direction);
            }
        }
    }

    fn set_layer(&mut self, layer: Layer) {
        match self {
            Self::Windows(windows) => windows.set_layer(layer),
            Self::X11(x11) => {
                let _ = x11.set_layer(layer);
            }
        }
    }

    /// Puts the window back to the normal layer from `layer`, and gives it back its size when
    /// wildgrass changed it, as wildgrass is about to exit. The X server has a request once it
    /// is sent.
    fn put_back(&mut self, layer: Layer) {
        match self {
            Self::Windows(windows) => windows.put_back(layer),
            Self::X11(x11) => {
                if layer != Layer::Normal {
                    let _ = x11.set_layer(Layer::Normal);
                }
            }
        }
    }
}
