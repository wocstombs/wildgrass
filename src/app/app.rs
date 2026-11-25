//! Main application state: active view, station directory, player, and input transitions.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use reqwest::Client;

use super::editor::StationEditor;
use super::keys;
use super::list::{ListMode, StationList};
use super::search::Search;
use crate::playback::Playback;
use crate::player::{MpvPlayer, PlayerEvent};
use crate::station::{Station, StationStore};
use crate::terminal::{Direction, WindowAction};

/// Action returned by a view after handling a key press.
pub(super) enum Next {
    /// Stay on the current view.
    Stay,
    Quit,
    /// Move the terminal window or adjust its stacking order.
    Window(WindowAction),
    Show(View),
    /// Play a listed station.
    Play(Station),
    /// Play a station chosen from search, saving it to the directory first if new.
    PlayFound(Station),
}

/// Control flow decision for the main event loop after processing an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Window(WindowAction),
    Quit,
}

pub enum View {
    StationList(StationList),
    StationSettings(StationEditor),
    Search(Search),
    NowPlaying(Box<Playback>),
}

impl View {
    pub fn list(mode: ListMode) -> Self {
        Self::StationList(StationList {
            mode,
            delete_confirm: false,
        })
    }
}

/// Handles keyboard shortcuts on the now-playing screen. Player is `None` in unit tests.
fn on_now_playing_key(
    playback: &mut Playback,
    key: KeyEvent,
    player: Option<&mut MpvPlayer>,
) -> Next {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return Next::Stay;
    }
    match key.code {
        KeyCode::Char('q' | 'Q') | KeyCode::Esc => {
            if let Some(mpv) = player {
                mpv.stop();
            }
            return Next::Show(View::list(ListMode::Play));
        }
        KeyCode::Char('t' | 'T') => return Next::Window(WindowAction::CycleLayer),
        KeyCode::Left => return Next::Window(WindowAction::Move(Direction::Left)),
        KeyCode::Right => return Next::Window(WindowAction::Move(Direction::Right)),
        KeyCode::Up => return Next::Window(WindowAction::Move(Direction::Up)),
        KeyCode::Down => return Next::Window(WindowAction::Move(Direction::Down)),
        _ => {
            let Some(mpv) = player else {
                return Next::Stay;
            };
            match key.code {
                KeyCode::Char(' ') => {
                    mpv.toggle_pause();
                    playback.set_paused(!playback.is_paused());
                }
                KeyCode::Char('m' | 'M') => mpv.toggle_mute(),
                KeyCode::Char('-') => mpv.adjust_volume(-5),
                KeyCode::Char('+' | '=') => mpv.adjust_volume(5),
                _ => {}
            }
        }
    }
    Next::Stay
}

pub struct App {
    pub stations: StationStore,
    /// Cursor index in the station list; `stations.len()` points to the trailing action row.
    pub cursor: usize,
    pub view: View,
    /// Player instance (`None` in unit tests that run without `mpv`).
    player: Option<MpvPlayer>,
    client: Client,
}

impl App {
    pub fn new(stations: StationStore, player: Option<MpvPlayer>, client: Client) -> Self {
        Self {
            stations,
            cursor: 0,
            view: View::list(ListMode::Play),
            player,
            client,
        }
    }

    /// Plays `station`, moving the selection cursor to it if listed.
    pub fn play(&mut self, station: &Station) {
        if let Some(index) = self.stations.position_of(station) {
            self.cursor = index;
        }
        if let Some(mpv) = &mut self.player {
            let _ = mpv.poll_events();
            mpv.load_stream(&station.stream_urls());
        }
        let mpv_pid = self.player.as_ref().map(MpvPlayer::pid);
        let mut playback = Playback::start(station, mpv_pid, self.client.clone());
        if self.player.is_some() {
            // mpv stays core-idle=true while opening a stream and sends no new event until audio
            // starts; start in buffering state so startup silence is not sent to Shazam.
            playback.set_idle(true);
        }
        self.view = View::NowPlaying(Box::new(playback));
    }

    fn apply(&mut self, next: Next) -> Flow {
        match next {
            Next::Stay => {}
            Next::Quit => return Flow::Quit,
            Next::Window(action) => return Flow::Window(action),
            Next::Show(view) => self.view = view,
            Next::Play(station) => self.play(&station),
            // Add new stations before playing; existing stations play directly
            Next::PlayFound(station) => {
                let index = self.stations.add(station);
                let saved = self.stations[index].clone();
                self.play(&saved);
            }
        }
        Flow::Continue
    }

    /// Event loop poll timeout: ~60 FPS during playback for smooth spectrum animations.
    pub fn poll_timeout(&self) -> Duration {
        match &self.view {
            View::NowPlaying(playback) if !playback.is_idle() && !playback.is_paused() => {
                Duration::from_millis(16)
            }
            _ => Duration::from_millis(50),
        }
    }

    /// Processes player events and completed background tasks on each loop iteration.
    pub async fn tick(&mut self) {
        let events = self
            .player
            .as_mut()
            .map(MpvPlayer::poll_events)
            .unwrap_or_default();

        let next = match &mut self.view {
            View::NowPlaying(playback) => {
                for event in events {
                    match event {
                        PlayerEvent::IcyTitle(raw) => playback.on_icy_title(&raw),
                        PlayerEvent::Idle(idle) => playback.set_idle(idle),
                        PlayerEvent::Pause(paused) => playback.set_paused(paused),
                        PlayerEvent::StreamError(reason) => playback.on_stream_error(&reason),
                    }
                }
                playback.tick().await;
                Next::Stay
            }
            View::Search(search) => search.collect().await.map_or(Next::Stay, Next::PlayFound),
            View::StationList(_) | View::StationSettings(_) => Next::Stay,
        };
        self.apply(next);
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Flow {
        if keys::is_ctrl(key, 'c') {
            return Flow::Quit;
        }
        let next = match &mut self.view {
            View::StationList(list) => list.on_key(key, &mut self.stations, &mut self.cursor),
            View::StationSettings(editor) => {
                editor.on_key(key, &mut self.stations, &mut self.cursor)
            }
            View::Search(search) => search.on_key(key, &self.client),
            View::NowPlaying(playback) => on_now_playing_key(playback, key, self.player.as_mut()),
        };
        self.apply(next)
    }
}
