//! In-place editor for station name and stream URL.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{Next, View};
use super::keys;
use super::list::ListMode;
use super::text_input::TextInput;
use crate::station::{Station, StationStore};
use crate::terminal::WindowAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsField {
    Name,
    Link,
}

impl SettingsField {
    fn other(self) -> Self {
        match self {
            Self::Name => Self::Link,
            Self::Link => Self::Name,
        }
    }

    fn value(self, station: &Station) -> String {
        match self {
            Self::Name => station.name.clone(),
            Self::Link => station.url.clone(),
        }
    }

    /// Stores a non-empty trimmed value; returns true if saved.
    fn apply(self, station: &mut Station, value: &str) -> bool {
        let value = value.trim().to_string();
        if value.is_empty() {
            return false;
        }
        match self {
            Self::Name => station.name = value,
            Self::Link => station.url = value,
        }
        true
    }
}

/// In-place editor for a station. Drafts edits for an existing station or creates a new one.
pub struct StationEditor {
    pub station: Station,
    pub field: SettingsField,
    /// Active text edit buffer while typing.
    pub edit: Option<TextInput>,
    /// Field that caused a conflict because another station already uses that value.
    /// Reset on the next key press.
    pub conflict: Option<SettingsField>,
}

impl StationEditor {
    pub fn new(station: Station) -> Self {
        Self {
            station,
            field: SettingsField::Name,
            edit: None,
            conflict: None,
        }
    }

    pub(super) fn on_key(
        &mut self,
        key: KeyEvent,
        stations: &mut StationStore,
        cursor: &mut usize,
    ) -> Next {
        self.conflict = None;
        let control_held = key.modifiers.contains(KeyModifiers::CONTROL);

        let Some(buffer) = &mut self.edit else {
            match key.code {
                _ if keys::is_ctrl(key, 's') => return Next::Show(View::list(ListMode::Play)),
                KeyCode::Esc | KeyCode::Char('q' | 'Q') => {
                    return Next::Show(View::list(ListMode::Edit))
                }
                KeyCode::Char('w' | 'W' | 's' | 'S')
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Tab => self.field = self.field.other(),
                KeyCode::Char('t' | 'T') => return Next::Window(WindowAction::CycleLayer),
                KeyCode::Enter | KeyCode::Char('e' | 'E') => {
                    self.edit = Some(TextInput::new(self.field.value(&self.station)))
                }
                _ => {}
            }
            return Next::Stay;
        };

        match key.code {
            KeyCode::Char('u' | 'U') if control_held => buffer.clear(),
            KeyCode::Char(character) if !control_held => buffer.insert(character),
            KeyCode::Backspace => buffer.backspace(),
            KeyCode::Delete => buffer.delete(),
            KeyCode::Left => buffer.move_left(),
            KeyCode::Right => buffer.move_right(),
            KeyCode::Home => buffer.move_start(),
            KeyCode::End => buffer.move_end(),
            KeyCode::Esc => self.edit = None,
            KeyCode::Enter | KeyCode::Up | KeyCode::Down | KeyCode::Tab => {
                let saved = self.commit(stations, cursor);
                if saved && key.code != KeyCode::Enter {
                    // Switch directly to editing the other field
                    self.field = self.field.other();
                    self.edit = Some(TextInput::new(self.field.value(&self.station)));
                }
            }
            _ => {}
        }
        Next::Stay
    }

    /// Saves and closes the active field. Returns false and leaves the field open
    /// if another station already uses this name or link.
    fn commit(&mut self, stations: &mut StationStore, cursor: &mut usize) -> bool {
        let Some(text) = self.edit.take() else {
            return true;
        };
        let mut updated = self.station.clone();
        if self.field.apply(&mut updated, text.as_str()) {
            if stations.conflicts(*cursor, &updated) {
                self.edit = Some(text);
                self.conflict = Some(self.field);
                return false;
            }
            self.station = updated;
            // The cursor targets the edited station, or the trailing row until a new
            // station has both fields filled
            if *cursor < stations.len() {
                stations.replace(*cursor, self.station.clone());
            } else if self.station.is_complete() {
                *cursor = stations.add(self.station.clone());
            }
        }
        true
    }
}
