//! Station directory view: browsing, deletion, and launching playback or search.

use std::mem;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{Next, View};
use super::editor::StationEditor;
use super::keys;
use super::search::Search;
use crate::station::StationStore;
use crate::terminal::WindowAction;

/// Controls Enter key behavior and trailing row options in the station list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListMode {
    /// Play the selected station; trailing "Other" row opens online search.
    Play,
    /// Edit the selected station; trailing "Custom" row creates a new station.
    Edit,
}

impl ListMode {
    pub(super) fn toggled(self) -> Self {
        match self {
            Self::Play => Self::Edit,
            Self::Edit => Self::Play,
        }
    }
}

/// Station list view with trailing action row.
pub struct StationList {
    pub mode: ListMode,
    /// Tracks whether station deletion is awaiting confirmation.
    pub delete_confirm: bool,
}

impl StationList {
    pub(super) fn on_key(
        &mut self,
        key: KeyEvent,
        stations: &mut StationStore,
        cursor: &mut usize,
    ) -> Next {
        // Delete confirmation lasts for a single key press: any following key confirms or cancels
        let confirming = mem::take(&mut self.delete_confirm);
        let on_station = *cursor < stations.len();

        let control_held = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            _ if keys::is_ctrl(key, 's') => self.mode = self.mode.toggled(),
            _ if control_held => {}
            KeyCode::Backspace | KeyCode::Delete if on_station => {
                if confirming {
                    stations.remove(*cursor);
                    *cursor = (*cursor).min(stations.len().saturating_sub(1));
                } else {
                    self.delete_confirm = true;
                }
            }
            KeyCode::Esc if confirming => {}
            KeyCode::Char('q' | 'Q') | KeyCode::Esc => return Next::Quit,
            KeyCode::Char('t' | 'T') => return Next::Window(WindowAction::CycleLayer),
            KeyCode::Enter | KeyCode::Char('e' | 'E') => {
                return match (self.mode, stations.get(*cursor)) {
                    (ListMode::Play, Some(station)) => Next::Play(station.clone()),
                    (ListMode::Play, None) => Next::Show(View::Search(Search::default())),
                    (ListMode::Edit, station) => Next::Show(View::StationSettings(
                        StationEditor::new(station.cloned().unwrap_or_default()),
                    )),
                }
            }
            code => keys::navigate(cursor, stations.len() + 1, code),
        }
        Next::Stay
    }
}
