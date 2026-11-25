//! Active song state: lyrics lookup lifecycle, timeline tracking, and vocal timing checks.

use std::mem;
use std::time::Instant;

use reqwest::Client;

use crate::apple_music::{self, TrackLengths};
use crate::lyrics::{self, Found, Lyrics, LyricsCache, Release, Singing};
use crate::shazam::{self, SongMatch};
use crate::task::Task;
use crate::timeline::{Outcome, Shifts, Timeline};
use crate::track::{self, TrackName};
use crate::{log, text};

/// Minimum duration after the final lyric line before rechecking song completion via Shazam.
const OUTRO: f64 = 4.0;
/// Grace period past known track duration before clearing an unconfirmed song during error backoff.
const SONG_END_GRACE: f64 = 5.0;
/// Fallback outro window past the last lyric line when total duration is unknown and Shazam fails.
const UNKNOWN_LENGTH_OUTRO: f64 = 60.0;

/// Returns true if playback position has progressed past the last lyric line by at least `OUTRO`.
fn lyrics_ended_at(lyrics: &Lyrics, position: f64) -> bool {
    position >= lyrics.last_line_time() + OUTRO
}

/// Fetched Apple Music track length for persistent caching in `TrackLengths`.
pub(super) struct FetchedLength {
    pub(super) id: String,
    pub(super) seconds: f64,
}

/// Result returned by an asynchronous lyrics lookup task to `Song::settle`.
#[derive(Default)]
pub(super) struct LookupOutput {
    pub(super) lookup: Lookup,
    pub(super) fetched_apple_length: Option<FetchedLength>,
    /// Track duration for the release (cached or retrieved from Apple Music).
    pub(super) length: Option<f64>,
    /// Latin-script track name from Shazam's `en-US` fallback when `zh-CN` transliterated a
    /// Western artist and no catalogue matched under the transliterated name.
    pub(super) fallback_name: Option<TrackName>,
}

/// Current status of a song's lyrics lookup.
#[derive(Default)]
pub(super) enum Lookup {
    /// Waiting for Shazam to identify the exact release before querying catalogues.
    Idle,
    /// Actively querying catalogues in the background.
    Running(Task<LookupOutput>),
    /// Successfully retrieved a matching catalogue entry.
    Found(Found),
    /// No matching lyrics found across catalogues.
    #[default]
    NotFound,
}

/// Cached Apple Music length for `matched`, if known.
fn known_length_of(matched: &SongMatch, track_lengths: &TrackLengths) -> Option<f64> {
    matched
        .apple_music_id
        .as_deref()
        .and_then(|id| track_lengths.get(id))
}

/// Whether `name` looks like a Western track whose artist Shazam transliterated into CJK.
fn needs_english_fallback(name: &TrackName) -> bool {
    !text::contains_cjk(&name.title) && text::contains_cjk(&name.artist)
}

impl Lookup {
    /// Looks up lyrics for `matched`, using `known_length` when cached or fetching the length
    /// from Apple Music. If no catalogue matches a Latin-titled track whose artist arrived in
    /// CJK, fetches Shazam's `en-US` track name and retries.
    fn for_match(
        client: &Client,
        matched: &SongMatch,
        known_length: Option<f64>,
        cache: Option<&LyricsCache>,
    ) -> Self {
        let client = client.clone();
        let name = matched.track.clone();
        let shazam_key = matched.key.clone();
        let apple_music_id = matched.apple_music_id.clone();
        let cache = cache.cloned();
        Self::Running(Task::spawn(async move {
            let (length, fetched_apple_length) = match (known_length, apple_music_id) {
                (Some(length), _) => (Some(length), None),
                (None, Some(id)) => match apple_music::track_length(&client, &id).await {
                    Some(seconds) => (Some(seconds), Some(FetchedLength { id, seconds })),
                    None => (None, None),
                },
                (None, None) => (None, None),
            };
            let release = Release::new(&name.title, length);
            let mut found = lyrics::fetch_lyrics(&client, &name, release, cache.as_ref()).await;
            let mut fallback_name = None;
            if found.is_none() && !shazam_key.is_empty() && needs_english_fallback(&name) {
                if let Some(english) = shazam::english_track_name(&client, &shazam_key).await {
                    found = lyrics::fetch_lyrics(&client, &english, release, cache.as_ref()).await;
                    if let Some(matched) = &found {
                        // Also file under Shazam's zh-CN name so future plays hit the cache directly;
                        // leave `fallback_name` empty so `self.reported` stays aligned with that file
                        // when `hear_singing` later saves timing offsets.
                        if let Some(cache) = &cache {
                            cache.save_lookup(&name, &matched.entry);
                        }
                    } else {
                        fallback_name = Some(english);
                    }
                }
            }
            let lookup = match found {
                Some(found) => Self::Found(found),
                None => Self::NotFound,
            };
            LookupOutput {
                lookup,
                fetched_apple_length,
                length,
                fallback_name,
            }
        }))
    }

    /// Replaces a lookup that has finished with its result, returning the settled output when
    /// it finished just now.
    pub(super) async fn settle(&mut self) -> Option<LookupOutput> {
        if !matches!(self, Self::Running(task) if task.is_finished()) {
            return None;
        }
        if let Self::Running(task) = mem::take(self) {
            let mut output = task.join().await;
            *self = mem::take(&mut output.lookup);
            return Some(output);
        }
        None
    }
}

/// One Shazam release already looked up, so neither the same Shazam key nor a different key
/// carrying the same title, artist and Apple Music id is searched twice.
pub(super) struct TriedRelease {
    pub(super) key: String,
    pub(super) track: TrackName,
    pub(super) apple_music_id: Option<String>,
}

impl TriedRelease {
    fn from_match(matched: &SongMatch) -> Self {
        Self {
            key: matched.key.clone(),
            track: matched.track.clone(),
            apple_music_id: matched.apple_music_id.clone(),
        }
    }

    fn matches(&self, matched: &SongMatch) -> bool {
        self.key == matched.key
            || (self.track == matched.track && self.apple_music_id == matched.apple_music_id)
    }
}

/// State of vocal onset synchronization against lyric line timestamps (see `lyrics::Singing`).
#[derive(Default)]
pub(super) enum TimingCheck {
    /// Awaiting confirmed song position before analyzing vocal timing.
    #[default]
    Waiting,
    /// Measuring vocal onset timestamps to compute median offset.
    Listening(Singing),
    /// Timing offset calculation completed or skipped.
    Done,
}

/// Active song tracked on the station. Replaced when a new song is identified.
pub(super) struct Song {
    /// Normalized track name from ICY metadata or Shazam.
    pub(super) reported: TrackName,
    /// Previously queried releases to prevent duplicate requests.
    pub(super) tried: Vec<TriedRelease>,
    pub(super) lookup: Lookup,
    /// Track duration from Apple Music if known.
    pub(super) known_length: Option<f64>,
    /// Playback timeline position tracking.
    pub(super) timeline: Timeline,
    /// True if the track was initially identified from ICY stream metadata.
    pub(super) from_icy: bool,
    /// Number of consecutive Shazam matches for this track.
    pub(super) heard: u8,
    /// Formatted track string (`Artist - Title`) displayed in the UI once settled.
    pub(super) shown_name: Option<String>,
    /// Tracks if an early Shazam query was already dispatched after lyrics completed.
    pub(super) outro_checked: bool,
    pub(super) timing: TimingCheck,
}

impl Song {
    /// Creates a song entry from ICY metadata, awaiting Shazam confirmation for timeline and release info.
    pub(super) fn new_from_icy(reported: TrackName) -> Self {
        Self {
            lookup: Lookup::Idle,
            reported,
            tried: Vec::new(),
            known_length: None,
            timeline: Timeline::new(""),
            from_icy: true,
            heard: 0,
            shown_name: None,
            outro_checked: false,
            timing: TimingCheck::Waiting,
        }
    }

    /// The song a Shazam answer names, with the position it gives.
    pub(super) fn new_from_shazam(
        client: &Client,
        matched: &SongMatch,
        shifts: &mut Shifts,
        cache: Option<&LyricsCache>,
        track_lengths: &TrackLengths,
    ) -> Self {
        let mut timeline = Timeline::new(matched.key.as_str());
        timeline.observe(matched.heard_at, &matched.alignments, shifts);
        let known_length = known_length_of(matched, track_lengths);
        Self {
            lookup: Lookup::for_match(client, matched, known_length, cache),
            reported: matched.track.clone(),
            tried: vec![TriedRelease::from_match(matched)],
            known_length,
            timeline,
            from_icy: false,
            heard: 1,
            shown_name: None,
            outro_checked: false,
            timing: TimingCheck::Waiting,
        }
    }

    /// Takes in the lookup's result once it has finished, caches any newly fetched Apple Music
    /// length, and works out the name to show with it.
    pub(super) async fn settle(&mut self, track_lengths: &mut TrackLengths) {
        let Some(output) = self.lookup.settle().await else {
            return;
        };
        if let Some(fetched) = output.fetched_apple_length {
            track_lengths.remember(fetched.id, fetched.seconds);
        }
        if output.length.is_some() {
            self.known_length = output.length;
        }
        if let Some(english) = output.fallback_name {
            if english.same_song(&self.reported) {
                self.reported = english;
            } else {
                // Keep the original title so later zh-CN Shazam answers still match this song
                self.reported.artist = english.artist;
            }
        }
        self.show_result();
    }

    /// Works out the name to show from the finished lookup.
    pub(super) fn show_result(&mut self) {
        let found = self.found();
        let canonical = found.map(|found| &found.entry.name);
        // Lyrics of another release are not shown, but still tell the song's language
        let lyrics = found.and_then(|found| found.entry.lyrics.as_ref());
        let name = track::choose_display_name(&self.reported, canonical, lyrics);
        self.shown_name = Some(name.display());
    }

    /// The song's lyrics catalogue entry, once the lookup has found one.
    fn found(&self) -> Option<&Found> {
        match &self.lookup {
            Lookup::Found(found) => Some(found),
            _ => None,
        }
    }

    /// The lyrics to show: those timed to the release that is playing.
    pub(super) fn lyrics(&self) -> Option<&Lyrics> {
        self.found()?.lyrics()
    }

    /// Where the lyrics are at `now` (seconds): the position in the song, plus how late the
    /// entry's lines were measured to be, so that each shows when its singing starts.
    pub(super) fn lyrics_position(&self, now: Instant) -> Option<f64> {
        let late_by = self.found().and_then(|found| found.entry.late_by);
        Some(self.timeline.position_at(now)? + late_by.unwrap_or(0.0))
    }

    /// Whether the music has gone on past the last lyric line, by `OUTRO`, at `now`.
    pub(super) fn lyrics_ended(&self, now: Instant) -> bool {
        self.lyrics().is_some_and(|lyrics| {
            self.lyrics_position(now)
                .is_some_and(|position| lyrics_ended_at(lyrics, position))
        })
    }

    /// Total length (seconds) of the release playing, from Apple Music or the matching lyrics
    /// entry.
    fn length(&self) -> Option<f64> {
        self.known_length.or_else(|| {
            self.found()
                .filter(|found| found.in_time)
                .and_then(|found| found.entry.length)
        })
    }

    /// Whether playback has run past the song's known total length (or well past its last lyric
    /// line when total length is unknown).
    pub(super) fn past_end(&self, now: Instant) -> bool {
        let Some(position) = self.timeline.position_at(now) else {
            return false;
        };
        if let Some(length) = self.length() {
            return position >= length + SONG_END_GRACE;
        }
        self.lyrics()
            .is_some_and(|lyrics| position >= lyrics.last_line_time() + UNKNOWN_LENGTH_OUTRO)
    }

    /// Takes in where singing started (`starts`: the instants it was audible) and, once enough
    /// of it lines up with the lyrics, measures how late the entry's lines are timed. The entry
    /// is then saved in `cache` with that measurement, so the next time the song plays its
    /// lyrics are in time from the start.
    pub(super) fn hear_singing(
        &mut self,
        starts: &[Instant],
        now: Instant,
        cache: Option<&LyricsCache>,
    ) {
        let Some(position) = self.timeline.position_at(now) else {
            return;
        };
        if let TimingCheck::Waiting = self.timing {
            self.timing = TimingCheck::Listening(Singing::new(position));
        }
        let TimingCheck::Listening(singing) = &mut self.timing else {
            return;
        };
        for &at in starts {
            if let Some(start) = self.timeline.position_at(at) {
                singing.heard(start);
            }
        }
        let Lookup::Found(found) = &mut self.lookup else {
            return;
        };
        if found.entry.late_by.is_some() {
            // Measured the last time the song played
            self.timing = TimingCheck::Done;
            return;
        }
        // None for an instrumental or another release, until a later lookup brings lyrics
        let Some(lyrics) = found.lyrics() else {
            return;
        };
        let ended = lyrics_ended_at(lyrics, position);
        // Only new singing, or the lyrics ending, can change the answer
        if starts.is_empty() && !ended {
            return;
        }
        let title = &found.entry.name.title;
        match singing.lateness(lyrics, position) {
            Some(lateness) if lateness.is_clear() => {
                log::sync(format!(
                    "lyrics timing: \"{title}\" is timed {lateness} against the singing: \
                     shown that much earlier"
                ));
                found.entry.late_by = Some(lateness.seconds);
                if let Some(cache) = cache {
                    cache.save_lookup(&self.reported, &found.entry);
                }
                self.timing = TimingCheck::Done;
            }
            lateness if ended => {
                let heard = lateness.map_or("no singing heard near its lines".into(), |lateness| {
                    format!("not clear from the singing: {lateness}")
                });
                log::sync(format!("lyrics timing: \"{title}\" left as it is, {heard}"));
                self.timing = TimingCheck::Done;
            }
            _ => {}
        }
    }

    /// Whether `name` names this song (by its reported or its catalogue name).
    pub(super) fn is(&self, name: &TrackName) -> bool {
        self.reported.same_song(name)
            || self
                .found()
                .is_some_and(|found| found.entry.name.same_song(name))
    }

    /// Whether the song is settled enough to show: the station named it, or two Shazam
    /// answers did.
    pub(super) fn is_confirmed(&self) -> bool {
        self.from_icy || self.heard >= 2
    }

    /// Takes in a Shazam answer naming this song: the position follows the recording the
    /// lyrics belong to, and a song still without lyrics is looked up as the release the
    /// answer names, if that is a new one.
    pub(super) fn on_match(
        &mut self,
        client: &Client,
        matched: &SongMatch,
        shifts: &mut Shifts,
        cache: Option<&LyricsCache>,
        track_lengths: &TrackLengths,
    ) -> Outcome {
        self.heard = self.heard.saturating_add(1);
        if self.wants_release(matched) {
            log::sync(format!(
                "looking up \"{}\" as release {}",
                matched.track.title, matched.key
            ));
            let known_length = known_length_of(matched, track_lengths);
            self.reported = matched.track.clone();
            self.known_length = known_length;
            self.tried.push(TriedRelease::from_match(matched));
            self.lookup = Lookup::for_match(client, matched, known_length, cache);
            self.timeline.follow(&matched.key);
        }
        self.timeline
            .observe(matched.heard_at, &matched.alignments, shifts)
    }

    /// Whether to look the song up again as the release `matched` names. Shazam files one song
    /// under several releases (the original, a remaster, an instrumental) and names now one,
    /// now another. The first whose lyrics are found stays, and a different Shazam key with the
    /// same title, artist and Apple Music id is not searched again.
    fn wants_release(&self, matched: &SongMatch) -> bool {
        // ICY metadata gave no release, so lyrics waited for Shazam's first answer
        self.tried.is_empty()
            || (!self.tried.iter().any(|tried| tried.matches(matched))
                && !matches!(self.lookup, Lookup::Running(_))
                && self.lyrics().is_none())
    }
}
