//! Online station search prompt using the Radio Browser directory.

use std::mem;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use reqwest::Client;

use super::app::{Next, View};
use super::keys;
use super::list::ListMode;
use super::text_input::TextInput;
use crate::station::{self, Station};
use crate::task::{self, Task};

const SEARCH_RESULT_LIMIT: usize = 15;
/// Debounce duration before dispatching a search query while typing.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(250);

/// Online Radio Browser search prompt and results list.
#[derive(Default)]
pub struct Search {
    pub query: TextInput,
    pub results: Vec<Station>,
    pub selected: usize,
    request: Option<Task<Vec<Station>>>,
    /// Play the first result once the active request finishes (when Enter is pressed early).
    play_when_done: bool,
}

impl Search {
    /// Creates a prompt with preloaded query results (for unit tests).
    #[cfg(test)]
    pub(super) fn showing(query: &str, results: Vec<Station>) -> Self {
        Self {
            query: TextInput::new(query.to_string()),
            results,
            ..Self::default()
        }
    }

    /// Simulates an in-flight search task yielding `results` (for unit tests).
    #[cfg(test)]
    pub(super) fn awaiting(&mut self, results: Vec<Station>) {
        self.request = Some(Task::spawn(async move { results }));
    }

    /// Restarts the search for the current query, aborting any pending in-flight request.
    fn restart(&mut self, client: &Client) {
        self.selected = 0;
        self.play_when_done = false;
        let query = self.query.as_str().trim().to_string();
        if query.is_empty() {
            self.request = None;
            self.results.clear();
            return;
        }
        let client = client.clone();
        self.request = Some(Task::spawn(async move {
            // Typing replaces this task, cancelling the previous debounce or request
            tokio::time::sleep(SEARCH_DEBOUNCE).await;
            station::search_radio_browser(&client, &query, SEARCH_RESULT_LIMIT).await
        }));
    }

    /// Collects completed search results; returns the station to play if Enter was pressed early.
    pub(super) async fn collect(&mut self) -> Option<Station> {
        self.results = task::take_finished(&mut self.request).await?;
        if mem::take(&mut self.play_when_done) {
            self.results.first().cloned()
        } else {
            None
        }
    }

    pub(super) fn on_key(&mut self, key: KeyEvent, client: &Client) -> Next {
        let control_held = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // Dropping the search view cancels any pending background request
            KeyCode::Esc => return Next::Show(View::list(ListMode::Play)),
            KeyCode::Up | KeyCode::Down => {
                keys::navigate(&mut self.selected, self.results.len(), key.code);
            }
            KeyCode::Left => self.query.move_left(),
            KeyCode::Right => self.query.move_right(),
            KeyCode::Home => self.query.move_start(),
            KeyCode::End => self.query.move_end(),
            KeyCode::Char('u' | 'U') if control_held => {
                self.query.clear();
                self.restart(client);
            }
            KeyCode::Backspace => {
                self.query.backspace();
                self.restart(client);
            }
            KeyCode::Delete => {
                self.query.delete();
                self.restart(client);
            }
            KeyCode::Char(character) if !control_held => {
                self.query.insert(character);
                self.restart(client);
            }
            KeyCode::Enter => {
                if let Some(station) = self.results.get(self.selected) {
                    return Next::PlayFound(station.clone());
                }
                if self.request.is_none() {
                    if self.query.as_str().trim().is_empty() {
                        return Next::Stay;
                    }
                    self.restart(client);
                }
                self.play_when_done = true;
            }
            _ => {}
        }
        Next::Stay
    }
}
