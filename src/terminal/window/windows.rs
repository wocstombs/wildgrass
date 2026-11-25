//! Windows and WSL window management via the PowerShell helper.
//!
//! Handles moving, layering, and sizing the terminal window. Resizes single-tab Windows Terminal
//! windows directly via escape codes, falling back to the helper for multi-tab windows.

use super::action::{Direction, Layer};
use super::helper::{Helper, WindowCommand};
use crate::terminal::size::{self, Resized, Shrink};

/// Mechanism used to apply wildgrass's compact window size.
enum OwnSize {
    /// Resized directly by Windows Terminal escape sequence.
    ByTerminal(Resized),
    /// Resized via PowerShell helper (multi-tab window fallback).
    ByHelper,
}

/// Windows terminal window controller backed by the PowerShell helper.
pub(super) struct Windows {
    helper: Helper,
    /// Strategy used for compact sizing, if applied.
    own_size: Option<OwnSize>,
}

impl Windows {
    /// Starts the background helper and shrinks Windows Terminal to compact size.
    pub(super) fn start() -> Self {
        let helper = Helper::start();
        let own_size = match size::shrink() {
            Shrink::Done(resized) => Some(OwnSize::ByTerminal(resized)),
            // Window has multiple tabs: let the helper resize it once initialized
            Shrink::Refused => {
                helper.send(WindowCommand::Resize);
                Some(OwnSize::ByHelper)
            }
            Shrink::Unneeded => None,
        };
        Self { helper, own_size }
    }

    pub(super) fn move_window(&self, direction: Direction) {
        self.helper.send(WindowCommand::Move(direction));
    }

    pub(super) fn set_layer(&self, layer: Layer) {
        self.helper.send(WindowCommand::Layer(layer));
    }

    /// Restores the window's default layer and previous dimensions on exit.
    pub(super) fn put_back(&mut self, layer: Layer) {
        let layered = layer != Layer::Normal;
        let resized_by_helper = matches!(self.own_size, Some(OwnSize::ByHelper));
        if layered {
            self.helper.send(WindowCommand::Layer(Layer::Normal));
        }
        if resized_by_helper {
            self.helper.send(WindowCommand::UndoResize);
        }
        if layered || resized_by_helper {
            self.helper.finish();
        }
        // Asynchronously restore dimensions via escape sequence
        if let Some(OwnSize::ByTerminal(resized)) = &self.own_size {
            size::restore(resized);
        }
    }
}
