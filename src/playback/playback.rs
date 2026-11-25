//! Playback session management: track identification, candidate confirmation, and query pacing.

#[cfg(test)]
mod tests;

use std::time::Instant;

use reqwest::Client;

use super::capture::Capture;
use super::schedule::Schedule;
use super::song::Song;
use crate::apple_music::TrackLengths;
use crate::lyrics::{Lyrics, LyricsCache};
use crate::shazam::{self, ShazamResult, SongMatch};
use crate::spectrum::Spectrum;
use crate::station::Station;
use crate::task::{self, Task};
use crate::timeline::{Outcome, Shifts};
use crate::track::TrackName;
use crate::{icy, log, paths};

/// Maximum consecutive mismatched Shazam answers before dismissing the current song.
const MAX_STRIKES: u8 = 3;

/// Station-provided song title (from ICY metadata).
///
/// When present, the station's title is displayed directly in the UI.
/// Song identity and lyric timestamps still derive from Shazam recognition.
struct StationTitle {
    name: TrackName,
    /// Formatted display string.
    shown: String,
}

/// Synchronized lyrics slice and playback offset.
pub struct ShownLyrics<'lyrics> {
    pub lyrics: &'lyrics Lyrics,
    /// Playback offset in seconds.
    pub position: f64,
}

/// Last non-match Shazam state written to `sync.log`, avoiding repeated log spam.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum ShazamLogState {
    #[default]
    Normal,
    NoSong,
    NoAudio,
}

pub struct Playback {
    pub station_name: String,
    client: Client,
    song: Option<Song>,
    station_title: Option<StationTitle>,
    /// Replacement candidate identified by Shazam. Promoted to active song when
    /// confirmed by a consecutive match with a consistent position.
    candidate: Option<Song>,
    /// Consecutive Shazam matches that differed from the active song.
    strikes: u8,
    /// Learned timeline offsets between different recordings of the same song.
    shifts: Shifts,
    /// Cached Apple Music recording durations.
    track_lengths: TrackLengths,
    schedule: Schedule,
    shazam_log_state: ShazamLogState,
    paused: bool,
    /// Timestamp when playback paused or started buffering; freezes lyric progression.
    idle_since: Option<Instant>,
    capture: Capture,
    /// Lyrics cache storage (`None` in unit tests to avoid disk I/O).
    lyrics_cache: Option<LyricsCache>,
    /// Pending in-flight Shazam request.
    shazam: Option<Task<ShazamResult>>,
}

impl Playback {
    /// Starts capturing station audio and loads persistent caches.
    pub fn start(station: &Station, mpv_pid: Option<u32>, client: Client) -> Self {
        // Fallback decodes the primary stream URL directly with ffmpeg if mpv capture fails
        let capture = Capture::start(station.stream_urls().swap_remove(0), mpv_pid);
        let shifts = Shifts::load(paths::recordings_file());
        let track_lengths = TrackLengths::load(paths::track_lengths_file());
        let lyrics_cache = LyricsCache::new(paths::lyrics_cache_directory());
        Self::new(
            station.name.clone(),
            client,
            shifts,
            track_lengths,
            capture,
            Some(lyrics_cache),
        )
    }

    /// Initializes a blank playback session ready for immediate identification.
    fn new(
        station_name: String,
        client: Client,
        shifts: Shifts,
        track_lengths: TrackLengths,
        capture: Capture,
        lyrics_cache: Option<LyricsCache>,
    ) -> Self {
        Self {
            station_name,
            client,
            song: None,
            station_title: None,
            candidate: None,
            strikes: 0,
            shifts,
            track_lengths,
            schedule: Schedule::default(),
            shazam_log_state: ShazamLogState::Normal,
            paused: false,
            idle_since: None,
            capture,
            lyrics_cache,
            shazam: None,
        }
    }

    pub fn is_idle(&self) -> bool {
        self.idle_since.is_some()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.update_idle(paused);
    }

    pub fn on_stream_error(&self, reason: &str) {
        log::error(format!("mpv: {} stream error: {reason}", self.station_name));
        log::sync(format!("stream error ({}): {reason}", self.station_name));
    }

    pub fn spectrum(&self) -> &Spectrum {
        self.capture.spectrum()
    }

    /// Returns the formatted track string (`Artist - Title`) for the UI.
    /// Prefers station ICY titles when available, otherwise uses the confirmed Shazam track name.
    pub fn display_name(&self) -> Option<&str> {
        if let Some(title) = &self.station_title {
            return Some(&title.shown);
        }
        self.confirmed_song()?.shown_name.as_deref()
    }

    /// Returns the current song's synchronized lyrics and playback position in seconds.
    /// Returns `None` if the playback position has not yet been confirmed by acoustic matches.
    pub fn lyrics_at(&self) -> Option<ShownLyrics<'_>> {
        let song = self.confirmed_song()?;
        let lyrics = song.lyrics()?;
        let position = song.lyrics_position(self.lyric_clock())?;
        Some(ShownLyrics { lyrics, position })
    }

    /// Returns the active song if confirmed.
    fn confirmed_song(&self) -> Option<&Song> {
        self.song.as_ref().filter(|song| song.is_confirmed())
    }

    /// Returns the active timeline reference instant, freezing time while paused or buffering.
    fn lyric_clock(&self) -> Instant {
        self.idle_since.unwrap_or_else(Instant::now)
    }

    pub fn on_icy_title(&mut self, raw: &str) {
        if raw.trim().is_empty() {
            // An empty StreamTitle clears a station-named track (such as between classical pieces
            // or during ad breaks), without disturbing a Shazam-identified song on stations that
            // never broadcast ICY titles.
            if self.station_title.take().is_some()
                && self.song.as_ref().is_some_and(|song| song.from_icy)
            {
                self.clear_song();
            }
            return;
        }
        let Some(track) = icy::parse_icy_title(raw, &self.station_name) else {
            self.station_title = None;
            self.clear_song();
            return;
        };
        // Reconnecting to the same stream title should not reset Shazam's confirmed progress
        if self
            .station_title
            .as_ref()
            .is_some_and(|title| title.name == track)
        {
            return;
        }
        self.station_title = Some(StationTitle {
            shown: track.display(),
            name: track.clone(),
        });
        if self.song.as_ref().is_some_and(|song| song.is(&track)) {
            return;
        }
        log::sync(format!("station names {}", track.display()));
        self.start_song(Song::new_from_icy(track));
        // Accelerate the next Shazam query to confirm timeline position
        self.schedule.hurry(Instant::now());
    }

    /// Clears the active song and any pending candidate.
    fn clear_song(&mut self) {
        self.song = None;
        self.candidate = None;
        self.strikes = 0;
    }

    /// Replaces the active song with `song` and resets candidate state.
    fn start_song(&mut self, song: Song) {
        self.clear_song();
        self.song = Some(song);
    }

    /// Creates a `Song` from a Shazam match using the session's caches.
    fn song_from_shazam(&mut self, matched: &SongMatch) -> Song {
        Song::new_from_shazam(
            &self.client,
            matched,
            &mut self.shifts,
            self.lyrics_cache.as_ref(),
            &self.track_lengths,
        )
    }

    /// Handles a Shazam match that differs from the currently confirmed song.
    fn challenge_song(&mut self, matched: &SongMatch) -> &'static str {
        self.strikes += 1;
        if let Some(mut candidate) = self.candidate.take().filter(|c| c.is(&matched.track)) {
            let outcome = candidate.on_match(
                &self.client,
                matched,
                &mut self.shifts,
                self.lyrics_cache.as_ref(),
                &self.track_lengths,
            );
            if !matches!(outcome, Outcome::Pending | Outcome::Ignored) {
                self.start_song(candidate);
                return "second answer naming it: now the song";
            }
            if self.strikes < MAX_STRIKES {
                self.candidate = Some(candidate);
                return "same candidate, but its position disagreed: waiting for agreement";
            }
        }
        let next = self.song_from_shazam(matched);
        if self.strikes >= MAX_STRIKES {
            self.start_song(next);
            "third answer in a row naming another song: the song showing is over"
        } else {
            self.candidate = Some(next);
            "another song than the one showing: waiting for a second answer"
        }
    }

    /// Processes a Shazam match: updates the active song, stages/promotes a candidate, or starts a new song.
    fn on_shazam_match(&mut self, matched: SongMatch) {
        let same_song = self
            .song
            .as_ref()
            .is_some_and(|song| song.is(&matched.track));
        let event = if same_song {
            self.candidate = None;
            self.strikes = 0;
            let song = self.song.as_mut().expect("checked above");
            song.on_match(
                &self.client,
                &matched,
                &mut self.shifts,
                self.lyrics_cache.as_ref(),
                &self.track_lengths,
            )
            .to_string()
        } else if self.song.as_ref().is_some_and(Song::is_confirmed) {
            self.challenge_song(&matched).to_string()
        } else {
            let song = self.song_from_shazam(&matched);
            self.start_song(song);
            "new song: waiting for a second answer".to_string()
        };
        log::sync(format!("{}: {event}", describe(&matched)));
    }

    /// Whether the next Shazam answer is wanted soon, to confirm what the latest one said.
    fn awaits_confirmation(&self) -> bool {
        self.candidate.is_some()
            || self
                .song
                .as_ref()
                .is_some_and(|song| !song.is_confirmed() || song.timeline.unsettled())
    }

    pub fn set_idle(&mut self, idle: bool) {
        // Never unfreeze playback while the user has it paused
        if self.paused && !idle {
            return;
        }
        self.update_idle(idle);
    }

    fn update_idle(&mut self, idle: bool) {
        let now = Instant::now();
        match (idle, self.idle_since) {
            (true, None) => self.idle_since = Some(now),
            (false, Some(idle_since)) => {
                self.idle_since = None;
                let songs = self.song.iter_mut().chain(&mut self.candidate);
                for song in songs {
                    song.timeline.delay(now - idle_since);
                }
            }
            _ => return,
        }
        self.capture.set_idle(idle);
    }

    /// Starts a Shazam lookup now, replacing (and cancelling) any lookup in flight.
    fn request_shazam(&mut self) {
        let (client, pcm) = (self.client.clone(), self.capture.pcm());
        self.shazam = Some(Task::spawn(
            async move { shazam::identify(&client, &pcm).await },
        ));
    }

    /// Collects finished background work, starts the next Shazam check when it is due, and
    /// advances the visualizer animation. Called once per main-loop pass.
    pub async fn tick(&mut self) {
        // Settle the candidate's lookup too so promotion shows name and lyrics on the same frame
        let songs = self.song.iter_mut().chain(&mut self.candidate);
        for song in songs {
            song.settle(&mut self.track_lengths).await;
        }
        // Before the idle check: singing heard just before a pause is placed in the song now,
        // while the clock still stands where it was heard
        let voice_starts = self.capture.take_voice_starts();
        let clock = self.lyric_clock();
        if let Some(song) = &mut self.song {
            song.hear_singing(&voice_starts, clock, self.lyrics_cache.as_ref());
        }

        if let Some(result) = task::take_finished(&mut self.shazam).await {
            match result {
                ShazamResult::Match(matched) => {
                    self.shazam_log_state = ShazamLogState::Normal;
                    self.on_shazam_match(matched);
                    self.schedule.after_match(self.awaits_confirmation());
                }
                ShazamResult::NoMatch { retry_after } => {
                    // Shazam is no longer detecting a song: clear the matched song and lyrics
                    // (keeping any station title)
                    if self.song.is_some() || self.shazam_log_state != ShazamLogState::NoSong {
                        log::sync("no song");
                        self.shazam_log_state = ShazamLogState::NoSong;
                    }
                    self.clear_song();
                    self.schedule.after_no_match(retry_after);
                }
                ShazamResult::NoAudio => {
                    if self.shazam_log_state != ShazamLogState::NoAudio {
                        log::sync("waiting for audio to fingerprint");
                        self.shazam_log_state = ShazamLogState::NoAudio;
                    }
                    self.schedule.after_no_audio();
                }
                ShazamResult::Error { retry_after } => {
                    self.shazam_log_state = ShazamLogState::Normal;
                    let wait = self.schedule.after_failure(retry_after);
                    log::sync(format!(
                        "shazam request failed, next in {:.0}s",
                        wait.as_secs_f64()
                    ));
                }
            }
        }

        if self.is_idle() {
            // A paused wave falls slowly; a buffering one is drawn flat (see `ui.rs`)
            if self.paused {
                self.capture.fall(Instant::now());
            }
            return;
        }

        let now = Instant::now();
        if self.schedule.is_backing_off()
            && self.song.as_ref().is_some_and(|song| song.past_end(now))
        {
            log::sync("song ended while Shazam is failing");
            self.clear_song();
        }
        self.hurry_once_the_lyrics_have_ended(now);
        if self.shazam.is_none() && self.schedule.is_due(now) {
            self.request_shazam();
        }
        self.capture.animate(now);
    }

    /// Once the song's lyrics have ended, check sooner whether the song has, too. Once per song:
    /// the usual checks go on from there. Doing this on every pass would cancel each wait as
    /// soon as it was set, so Shazam would be asked as fast as it answers; and a backoff is
    /// never cut short (see `Schedule::may_hurry`).
    fn hurry_once_the_lyrics_have_ended(&mut self, now: Instant) {
        if !self.schedule.may_hurry(now) {
            return;
        }
        let Some(song) = &mut self.song else {
            return;
        };
        if !song.outro_checked && song.lyrics_ended(now) {
            song.outro_checked = true;
            self.schedule.hurry(now);
        }
    }
}

/// A Shazam answer in one line for the sync log: the song, its ids, and the first recordings
/// with their positions.
fn describe(matched: &SongMatch) -> String {
    const SHOWN: usize = 6;
    let mut recordings: Vec<String> = matched
        .alignments
        .iter()
        .take(SHOWN)
        .map(|alignment| format!("{}@{:.2}", alignment.recording, alignment.offset))
        .collect();
    if matched.alignments.len() > SHOWN {
        recordings.push(format!("+{} more", matched.alignments.len() - SHOWN));
    }
    format!(
        "heard \"{}\" by \"{}\" key={} apple={}{} recordings=[{}]",
        matched.track.title,
        matched.track.artist,
        matched.key,
        matched.apple_music_id.as_deref().unwrap_or("none"),
        if matched.exact_timing {
            ""
        } else {
            " (ffmpeg fallback, timing estimated)"
        },
        recordings.join(" ")
    )
}
