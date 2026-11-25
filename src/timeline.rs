//! Track playback position tracking based on Shazam acoustic match alignments.
//!
//! Shazam compares captured audio against reference acoustic databases and returns all aligned
//! recordings (`Alignment`). Because different releases (album cuts, radio edits, remasters) can
//! have varying intros or shifts, `Timeline` anchors to a primary recording and confirms positions
//! only after consecutive agreeing matches.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::{fmt, fs, io};

use serde::{Deserialize, Serialize};

use crate::{files, log};

/// Maximum tolerance (seconds) between two start-time calculations to consider them in agreement.
const AGREE: f64 = 0.5;

/// Consecutive responses missing the primary recording before allowing another aligned recording to drive timing.
const MAIN_ABSENT_LIMIT: u8 = 3;

fn agree(a: f64, b: f64) -> bool {
    (a - b).abs() <= AGREE
}

/// Seconds from `from` to `to`, negative when `to` is the earlier one.
fn seconds_between(from: Instant, to: Instant) -> f64 {
    match to.checked_duration_since(from) {
        Some(forward) => forward.as_secs_f64(),
        None => -from.duration_since(to).as_secs_f64(),
    }
}

/// One reference recording a sample lined up with, and the position (seconds into that
/// recording) of the sample's first moment.
#[derive(Debug, Clone, PartialEq)]
pub struct Alignment {
    pub recording: String,
    pub offset: f64,
}

/// Where one recording an answer listed puts the song's start, in seconds after the timeline's
/// `epoch`.
#[derive(Debug, Clone, Copy)]
struct RecordingStart<'answer> {
    recording: &'answer str,
    at: f64,
}

/// What an answer did to the timeline, for the sync log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The answer carried no position.
    Nothing,
    /// A first position, not yet believed.
    Provisional,
    /// A second answer agreed with the first: the position is believed from now on.
    Confirmed,
    /// The answer agreed with the position in use.
    Kept,
    /// The answer disagreed with the position in use; it waits for a second answer like it.
    Pending,
    /// A second answer agreed with the pending one: the position moved.
    Adopted,
    /// The answer named only recordings other than the one followed, and was left out.
    Ignored,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Nothing => "no position",
            Self::Provisional => "first position",
            Self::Confirmed => "position confirmed",
            Self::Kept => "position agrees",
            Self::Pending => "position disagrees, waiting for a second answer",
            Self::Adopted => "position moved",
            Self::Ignored => "other recordings only, ignored",
        })
    }
}

/// Whether a proposed song start came from the followed recording (directly or via a learned shift).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Followed,
    Other,
}

impl Source {
    fn is_followed(self) -> bool {
        self == Self::Followed
    }
}

/// Song start time in seconds after `epoch`.
#[derive(Debug, Clone, Copy)]
struct Start {
    at: f64,
    /// Measured on the followed recording (`main` or `fallback`), directly or through a known shift.
    main: bool,
    confirmed: bool,
}

/// A start that disagrees with the one in use.
#[derive(Debug, Clone, Copy)]
struct Pending {
    at: f64,
    main: bool,
    /// It came with the latest answer.
    fresh: bool,
}

/// The start one recording gave last, and whether the answer before agreed.
#[derive(Debug, Clone, Copy)]
struct Sighting {
    at: f64,
    steady: bool,
}

/// The playback position within one song.
#[derive(Debug)]
pub struct Timeline {
    /// The recording to follow: the track key of the release the lyrics belong to.
    main: String,
    /// A steady recording followed when `main` has never appeared in Shazam's answers (many
    /// older Shazam tracks use a key that is not itself a recording id). Cleared if `main`
    /// ever appears.
    fallback: Option<String>,
    /// What starts are measured from. Moving it later holds the position still (see `delay`).
    epoch: Instant,
    seen: HashMap<String, Sighting>,
    start: Option<Start>,
    pending: Option<Pending>,
    main_absent: u8,
}

fn find_start(starts: &[RecordingStart], recording: &str) -> Option<f64> {
    starts
        .iter()
        .find(|start| start.recording == recording)
        .map(|start| start.at)
}

fn find_shifted(starts: &[RecordingStart], target: &str, shifts: &Shifts) -> Option<f64> {
    starts
        .iter()
        .find_map(|start| Some(start.at - shifts.get(start.recording, target)?))
}

impl Timeline {
    pub fn new(main: &str) -> Self {
        Self {
            main: main.to_string(),
            fallback: None,
            epoch: Instant::now(),
            seen: HashMap::new(),
            start: None,
            pending: None,
            main_absent: 0,
        }
    }

    /// Follows the recording `main` from now on. The position in use stays until that recording
    /// says otherwise.
    pub fn follow(&mut self, main: &str) {
        if self.main == main {
            return;
        }
        self.main = main.to_string();
        self.fallback = None;
        self.main_absent = 0;
        if let Some(start) = &mut self.start {
            start.main = false;
        }
    }

    fn is_steady(&self, recording: &str) -> bool {
        self.seen.get(recording).is_some_and(|seen| seen.steady)
    }

    /// Records the constant gap between `reference` and any other steady recording in `starts`.
    fn learn_shifts(
        &self,
        reference: &str,
        reference_at: f64,
        starts: &[RecordingStart],
        shifts: &mut Shifts,
    ) {
        if !self.is_steady(reference) {
            return;
        }
        for &RecordingStart { recording, at } in starts {
            if recording != reference && self.is_steady(recording) && !agree(at, reference_at) {
                shifts.learn(recording, reference, at - reference_at);
            }
        }
    }

    /// Picks a steady recording to follow when `self.main` has never appeared, preferring one
    /// that agrees with the provisional start already in use.
    fn choose_fallback(&self, starts: &[RecordingStart]) -> Option<String> {
        let matching_start = self.start.and_then(|start| {
            starts.iter().find(|candidate| {
                self.is_steady(candidate.recording) && agree(candidate.at, start.at)
            })
        });
        let chosen = matching_start.or_else(|| {
            starts
                .iter()
                .find(|candidate| self.is_steady(candidate.recording))
        })?;
        Some(chosen.recording.to_string())
    }

    /// Takes in one Shazam answer: the recordings a sample lined up with, the sample's first
    /// moment having been audible at `heard_at`. Shifts between recordings that the answer
    /// shows are saved in `shifts`, and those saved before are used.
    pub fn observe(
        &mut self,
        heard_at: Instant,
        alignments: &[Alignment],
        shifts: &mut Shifts,
    ) -> Outcome {
        if alignments.is_empty() {
            return Outcome::Nothing;
        }
        if let Some(pending) = &mut self.pending {
            pending.fresh = false;
        }
        let heard = seconds_between(self.epoch, heard_at);
        let starts: Vec<RecordingStart> = alignments
            .iter()
            .map(|alignment| RecordingStart {
                recording: &alignment.recording,
                at: heard - alignment.offset,
            })
            .collect();
        for &RecordingStart { recording, at } in &starts {
            let steady = self
                .seen
                .get(recording)
                .is_some_and(|seen| agree(seen.at, at));
            self.seen
                .insert(recording.to_string(), Sighting { at, steady });
        }

        if let Some(main_at) = find_start(&starts, &self.main) {
            self.fallback = None;
            self.learn_shifts(&self.main, main_at, &starts, shifts);
            self.main_absent = 0;
            return self.propose(main_at, Source::Followed);
        }
        if let Some(shifted) = find_shifted(&starts, &self.main, shifts) {
            self.main_absent = self.main_absent.saturating_add(1);
            return self.observe_others(&starts, Some(shifted));
        }
        // Older tracks often use a key that never appears as a recording id: lock onto a steady
        // recording from the answers so stray alternate cuts cannot bounce the position around
        if !self.seen.contains_key(&self.main) {
            if self.fallback.is_none() {
                self.fallback = self.choose_fallback(&starts);
            }
            if let Some(fallback) = self.fallback.as_deref() {
                if let Some(fallback_at) = find_start(&starts, fallback) {
                    self.learn_shifts(fallback, fallback_at, &starts, shifts);
                    self.main_absent = 0;
                    return self.propose(fallback_at, Source::Followed);
                }
                let fallback_shifted = find_shifted(&starts, fallback, shifts);
                self.main_absent = self.main_absent.saturating_add(1);
                return self.observe_others(&starts, fallback_shifted);
            }
        }
        self.main_absent = self.main_absent.saturating_add(1);
        self.observe_others(&starts, None)
    }

    /// Handles an answer missing the followed recording (`main` or `fallback`).
    /// Keeps the position if a known shift or another recording agrees; only replaces a confirmed
    /// main-recording position after `MAIN_ABSENT_LIMIT` consecutive misses.
    fn observe_others(&mut self, starts: &[RecordingStart], shifted: Option<f64>) -> Outcome {
        let (guess_at, guess_source) =
            shifted.map_or((starts[0].at, Source::Other), |at| (at, Source::Followed));
        let Some(start) = self.start else {
            return self.propose(guess_at, guess_source);
        };
        if let Some(at) = shifted.filter(|&at| agree(at, start.at)) {
            return self.propose(at, Source::Followed);
        }
        let closest = starts
            .iter()
            .map(|recording_start| recording_start.at)
            .filter(|&at| agree(at, start.at))
            .min_by(|a, b| (a - start.at).abs().total_cmp(&(b - start.at).abs()));
        match closest {
            Some(at) => self.propose(at, Source::Other),
            None if start.main && start.confirmed && self.main_absent < MAIN_ABSENT_LIMIT => {
                Outcome::Ignored
            }
            None => self.propose(guess_at, guess_source),
        }
    }

    /// Weighs a newly measured start against the one in use.
    fn propose(&mut self, at: f64, source: Source) -> Outcome {
        let main = source.is_followed();
        let Some(start) = self.start else {
            self.start = Some(Start {
                at,
                main,
                confirmed: false,
            });
            return Outcome::Provisional;
        };
        if agree(start.at, at) {
            self.start = Some(Start {
                at,
                main: main || start.main,
                confirmed: true,
            });
            self.pending = None;
            return if start.confirmed {
                Outcome::Kept
            } else {
                Outcome::Confirmed
            };
        }
        match self.pending.filter(|pending| agree(pending.at, at)) {
            Some(pending) => {
                self.start = Some(Start {
                    at,
                    main: main || pending.main,
                    confirmed: true,
                });
                self.pending = None;
                Outcome::Adopted
            }
            None => {
                self.pending = Some(Pending {
                    at,
                    main,
                    fresh: true,
                });
                Outcome::Pending
            }
        }
    }

    /// Whether the next answer is wanted soon: the position is not believed yet, or the latest
    /// answer disagreed with it.
    pub fn unsettled(&self) -> bool {
        self.start.is_some_and(|start| !start.confirmed)
            || self.pending.is_some_and(|pending| pending.fresh)
    }

    /// Position in the song (seconds) at `now`, once two answers have agreed on it.
    pub fn position_at(&self, now: Instant) -> Option<f64> {
        let start = self.start.filter(|start| start.confirmed)?;
        Some((seconds_between(self.epoch, now) - start.at).max(0.0))
    }

    /// Holds the position still for `by`, so that time spent paused or rebuffering does not
    /// count as playback.
    pub fn delay(&mut self, by: Duration) {
        self.epoch += by;
    }
}

/// How far a recording's timeline is off a followed recording's (`main` or `fallback`) of the
/// same song.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Shift {
    main: String,
    /// The recording's start minus the followed recording's, in seconds.
    #[serde(rename = "secs")]
    seconds: f64,
}

/// Shifts between recordings learned from answers that listed both, kept on disk so that an
/// answer listing only the other recording can still be placed on the followed recording's
/// timeline.
#[derive(Debug, Default)]
pub struct Shifts {
    /// Where they are saved; `None` keeps them in memory only.
    path: Option<PathBuf>,
    known: HashMap<String, Shift>,
}

impl Shifts {
    /// The shifts saved at `path`; none when the file is missing or unreadable.
    pub fn load(path: PathBuf) -> Self {
        let known = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            known,
        }
    }

    fn get(&self, recording: &str, main: &str) -> Option<f64> {
        let shift = self.known.get(recording)?;
        (shift.main == main).then_some(shift.seconds)
    }

    fn learn(&mut self, recording: &str, main: &str, seconds: f64) {
        if self
            .get(recording, main)
            .is_some_and(|known| (known - seconds).abs() < 0.05)
        {
            return;
        }
        log::sync(format!(
            "recording {recording} runs {seconds:+.2}s off main recording {main}"
        ));
        let shift = Shift {
            main: main.to_string(),
            seconds,
        };
        self.known.insert(recording.to_string(), shift);
        if let Err(error) = self.save() {
            log::error(format!("recording shifts: {error}"));
        }
    }

    /// Writes the shifts atomically, so an interrupted save leaves the previous file intact.
    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let json = serde_json::to_vec(&self.known)?;
        files::write_atomically(path, &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ScratchDir;

    fn answer(alignments: &[(&str, f64)]) -> Vec<Alignment> {
        alignments
            .iter()
            .map(|&(recording, offset)| Alignment {
                recording: recording.to_string(),
                offset,
            })
            .collect()
    }

    /// A timeline following `main`, and a way to turn seconds into a recorded clip into instants.
    fn timeline(main: &str) -> (Timeline, impl Fn(f64) -> Instant) {
        let timeline = Timeline::new(main);
        let clip_start = timeline.epoch;
        (timeline, move |seconds| {
            clip_start + Duration::from_secs_f64(seconds)
        })
    }

    const MAIN: &str = "628935200";

    /// Four Shazam responses for "I'm Good (Blue)" taken 6 s apart. Track key 628935200 is the
    /// main recording; 628935208 and 638814137 sit 37.5 s earlier on the same track.
    fn im_good_answers() -> [(f64, Vec<Alignment>); 4] {
        [
            (
                95.0,
                answer(&[
                    ("628935208", 7.834),
                    ("638814137", 7.881),
                    (MAIN, 45.340),
                    ("637948173", 85.343),
                ]),
            ),
            (
                101.0,
                answer(&[("638814137", 13.921), (MAIN, 51.350), ("628935208", 13.877)]),
            ),
            (
                107.0,
                answer(&[
                    ("638814137", 19.918),
                    ("637948163", 95.474),
                    ("634434330", 57.411),
                ]),
            ),
            (
                113.0,
                answer(&[("638814137", 25.921), (MAIN, 63.346), ("628935208", 25.871)]),
            ),
        ]
    }

    #[test]
    fn test_position_follows_the_main_recording_whichever_is_listed_first() {
        let (mut timeline, at) = timeline(MAIN);
        let mut shifts = Shifts::default();
        let outcomes: Vec<Outcome> = im_good_answers()
            .iter()
            .map(|(seconds, answer)| timeline.observe(at(*seconds), answer, &mut shifts))
            .collect();
        assert_eq!(
            outcomes,
            [
                Outcome::Provisional,
                Outcome::Confirmed,
                // The main recording is missing from the third answer, but 638814137 is known
                // by then to run 37.4 seconds off it
                Outcome::Kept,
                Outcome::Kept
            ]
        );
        let position = timeline.position_at(at(113.0)).unwrap();
        assert!((position - 63.346).abs() < 0.05, "{position}");
        assert!(!timeline.unsettled());
    }

    #[test]
    fn test_a_single_answer_is_not_believed() {
        let (mut timeline, at) = timeline(MAIN);
        let (seconds, first) = &im_good_answers()[0];
        timeline.observe(at(*seconds), first, &mut Shifts::default());
        assert_eq!(timeline.position_at(at(96.0)), None);
        assert!(timeline.unsettled());
    }

    #[test]
    fn test_learned_shift_places_another_recording_on_the_main_timeline() {
        let scratch = ScratchDir::create();
        let path = scratch.path().join("recordings.json");
        let (mut first_time, at) = timeline(MAIN);
        let mut shifts = Shifts::load(path.clone());
        for (seconds, answer) in &im_good_answers() {
            first_time.observe(at(*seconds), answer, &mut shifts);
        }

        // Another day, Shazam lists only the other recording
        let mut shifts = Shifts::load(path);
        let (mut later, at) = timeline(MAIN);
        for (seconds, offset) in [(101.0, 13.921), (107.0, 19.918)] {
            later.observe(at(seconds), &answer(&[("638814137", offset)]), &mut shifts);
        }
        let position = later.position_at(at(107.0)).unwrap();
        assert!((position - 57.35).abs() < 0.1, "{position}");
    }

    #[test]
    fn test_a_skip_in_the_stream_moves_the_position_once_confirmed() {
        // 14 seconds of stream audio dropped between the first and second sample
        let (mut timeline, at) = timeline("40390772");
        let mut shifts = Shifts::default();
        let mut observe = |seconds, offset| {
            timeline.observe(at(seconds), &answer(&[("268519757", offset)]), &mut shifts)
        };
        assert_eq!(observe(20.0, 94.490), Outcome::Provisional);
        assert_eq!(observe(60.0, 148.517), Outcome::Pending);
        assert_eq!(observe(100.0, 188.513), Outcome::Adopted);
        let position = timeline.position_at(at(100.0)).unwrap();
        assert!((position - 188.513).abs() < 0.01, "{position}");
    }

    #[test]
    fn test_one_answer_at_the_wrong_place_changes_nothing() {
        let (mut timeline, at) = timeline("main");
        let mut shifts = Shifts::default();
        let mut observe = |seconds, offset| {
            timeline.observe(at(seconds), &answer(&[("main", offset)]), &mut shifts)
        };
        observe(0.0, 30.0);
        assert_eq!(observe(8.0, 38.0), Outcome::Confirmed);
        // The chorus, matched at its second appearance a minute later
        assert_eq!(observe(16.0, 106.0), Outcome::Pending);
        assert!(timeline.unsettled());
        assert_eq!(timeline.position_at(at(16.0)), Some(46.0));
        let mut observe = |seconds, offset| {
            timeline.observe(at(seconds), &answer(&[("main", offset)]), &mut shifts)
        };
        assert_eq!(observe(19.0, 49.0), Outcome::Kept);
        assert!(!timeline.unsettled());
    }

    #[test]
    fn test_other_recordings_take_over_only_after_the_main_one_stays_away() {
        let (mut timeline, at) = timeline("main");
        let mut shifts = Shifts::default();
        timeline.observe(at(0.0), &answer(&[("main", 30.0)]), &mut shifts);
        timeline.observe(at(8.0), &answer(&[("main", 38.0)]), &mut shifts);

        let mut other = |seconds, offset| {
            timeline.observe(at(seconds), &answer(&[("other", offset)]), &mut shifts)
        };
        assert_eq!(other(16.0, 10.0), Outcome::Ignored);
        assert_eq!(other(24.0, 18.0), Outcome::Ignored);
        assert_eq!(other(32.0, 26.0), Outcome::Pending);
        assert_eq!(other(35.0, 29.0), Outcome::Adopted);
        assert_eq!(timeline.position_at(at(35.0)), Some(29.0));
    }

    #[test]
    fn test_a_recording_that_agrees_keeps_the_position_without_the_main_one() {
        let (mut timeline, at) = timeline("main");
        let mut shifts = Shifts::default();
        timeline.observe(at(0.0), &answer(&[("main", 30.0)]), &mut shifts);
        timeline.observe(at(8.0), &answer(&[("main", 38.0)]), &mut shifts);
        let others = answer(&[("remix", 99.0), ("copy", 46.1)]);
        assert_eq!(
            timeline.observe(at(16.0), &others, &mut shifts),
            Outcome::Kept
        );
        let position = timeline.position_at(at(16.0)).unwrap();
        assert!((position - 46.1).abs() < 1e-6);
    }

    #[test]
    fn test_delay_holds_the_position_still() {
        let (mut timeline, at) = timeline("main");
        let mut shifts = Shifts::default();
        timeline.observe(at(0.0), &answer(&[("main", 42.5)]), &mut shifts);
        timeline.observe(at(3.0), &answer(&[("main", 45.5)]), &mut shifts);
        assert_eq!(timeline.position_at(at(4.0)), Some(46.5));
        // Paused for 2 seconds
        timeline.delay(Duration::from_secs(2));
        assert_eq!(timeline.position_at(at(6.0)), Some(46.5));
        // An answer without positions changes nothing
        assert_eq!(
            timeline.observe(at(7.0), &[], &mut shifts),
            Outcome::Nothing
        );
    }

    #[test]
    fn test_track_key_absent_from_matches_locks_onto_a_steady_recording() {
        // Older tracks (such as 邝美云's "堆积情感", key 41160038) never list their key in
        // `matches`; instead recording 238029182 appears steadily, while 376564092 diverges by
        // 1.5 seconds in the second half.
        let (mut timeline, at) = timeline("41160038");
        let mut shifts = Shifts::default();
        assert_eq!(
            timeline.observe(at(0.0), &answer(&[("238029182", 10.0)]), &mut shifts),
            Outcome::Provisional
        );
        assert_eq!(
            timeline.observe(at(8.0), &answer(&[("238029182", 18.0)]), &mut shifts),
            Outcome::Confirmed
        );
        // Stray alternate recording 1.5 seconds off is ignored instead of moving the timeline
        assert_eq!(
            timeline.observe(at(16.0), &answer(&[("376564092", 27.5)]), &mut shifts),
            Outcome::Ignored
        );
        assert_eq!(
            timeline.observe(at(24.0), &answer(&[("238029182", 34.0)]), &mut shifts),
            Outcome::Kept
        );
        let position = timeline.position_at(at(24.0)).unwrap();
        assert!((position - 34.0).abs() < 0.05, "{position}");
    }
}
