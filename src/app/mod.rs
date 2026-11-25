//! Application view state and keyboard interaction.
//!
//! Each view processes its own key events and returns a `Next` action instructing the app
//! what to do next (transition views, play a station, or update window layout).

mod app;
mod editor;
mod keys;
mod list;
mod search;
#[cfg(test)]
mod tests;
mod text_input;

pub use self::app::{App, Flow, View};
pub use self::editor::{SettingsField, StationEditor};
pub use self::list::{ListMode, StationList};
pub use self::search::Search;
pub use self::text_input::TextInput;
