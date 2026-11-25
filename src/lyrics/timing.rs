//! Lyrics timing verification and synchronization against detected vocal onsets.
//!
//! Measures timing discrepancies between catalogue lyric timestamps and actual vocal
//! onsets (from `audio/voice.rs`). Calculates the median offset across lines to dynamically
//! calibrate lyrics display so lines appear exactly when sung.

use std::fmt;

use super::Lyrics;

/// Search window radius around each lyric line timestamp.
const SEARCH_SECONDS: f64 = 2.5;
/// Lag before vocal onsets are reported (four 50 ms detector frames plus loop dispatch).
const REPORT_LAG_SECONDS: f64 = 0.5;
/// Minimum correlated lines and maximum interquartile spread required to apply an offset.
const MIN_LINES: usize = 8;
const MAX_SPREAD_SECONDS: f64 = 0.5;

#[derive(Debug, Clone, PartialEq)]
pub struct Singing {
    from: f64,
    starts: Vec<f64>,
}

/// Middle 50% (Q1..Q3) of observed line timing offsets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MiddleHalf {
    pub low: f64,
    pub high: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lateness {
    /// Median offset in seconds (positive when lyrics arrive after the singing).
    pub seconds: f64,
    pub lines: usize,
    pub middle_half: MiddleHalf,
}

impl fmt::Display for Lateness {
    /// "1.57s late (27 lines, middle half +1.33..+1.62s)", for the sync log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let way = if self.seconds < 0.0 { "early" } else { "late" };
        let MiddleHalf { low, high } = self.middle_half;
        write!(
            f,
            "{:.2}s {way} ({} lines, middle half {low:+.2}..{high:+.2}s)",
            self.seconds.abs(),
            self.lines
        )
    }
}

impl Lateness {
    /// Whether the lines agree well enough, and are enough, to go by.
    pub fn is_clear(&self) -> bool {
        self.lines >= MIN_LINES
            && self.middle_half.high - self.middle_half.low <= MAX_SPREAD_SECONDS
    }
}

impl Singing {
    /// Listening from `position` in the song on.
    pub fn new(position: f64) -> Self {
        Self {
            from: position,
            starts: Vec::new(),
        }
    }

    /// Singing started at `position` in the song.
    pub fn heard(&mut self, position: f64) {
        self.starts.push(position);
    }

    /// How late the lines of `lyrics` heard in full by `position` are timed. Each is compared
    /// with the nearest start of singing within `SEARCH_SECONDS`; `None` when no line has one.
    pub fn lateness(&self, lyrics: &Lyrics, position: f64) -> Option<Lateness> {
        let heard = self.from + SEARCH_SECONDS..=position - SEARCH_SECONDS - REPORT_LAG_SECONDS;
        let mut gaps: Vec<f64> = lyrics
            .lines
            .iter()
            .filter(|line| heard.contains(&line.time))
            .filter_map(|line| {
                self.starts
                    .iter()
                    .map(|start| line.time - start)
                    .filter(|gap| gap.abs() <= SEARCH_SECONDS)
                    .min_by(|a, b| a.abs().total_cmp(&b.abs()))
            })
            .collect();
        if gaps.is_empty() {
            return None;
        }
        gaps.sort_by(f64::total_cmp);
        let count = gaps.len();
        let median = if count % 2 == 1 {
            gaps[count / 2]
        } else {
            (gaps[count / 2 - 1] + gaps[count / 2]) / 2.0
        };
        Some(Lateness {
            seconds: median,
            lines: count,
            middle_half: MiddleHalf {
                low: gaps[count / 4],
                high: gaps[3 * count / 4],
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::LyricLine;

    /// Lines at `times`.
    fn lyrics(times: &[f64]) -> Lyrics {
        let lines = times
            .iter()
            .map(|&time| LyricLine {
                time,
                text: format!("line at {time}"),
                words: None,
            })
            .collect();
        Lyrics::from_lines(lines).unwrap()
    }

    /// Twelve lines, 6 to 8 s apart.
    const LINES: [f64; 12] = [
        20.0, 26.5, 34.0, 41.0, 47.5, 55.0, 62.0, 68.5, 76.0, 83.0, 89.5, 97.0,
    ];

    fn heard(from: f64, starts: impl IntoIterator<Item = f64>) -> Singing {
        let mut singing = Singing::new(from);
        for start in starts {
            singing.heard(start);
        }
        singing
    }

    #[test]
    fn test_lines_timed_late_are_measured_late() {
        // The singing starts 1.5 s before each line's time, give or take 0.1 s
        let wobble = [
            0.1, -0.05, 0.0, 0.08, -0.1, 0.02, 0.05, -0.02, 0.0, 0.1, -0.08, 0.03,
        ];
        let singing = heard(
            10.0,
            LINES
                .iter()
                .zip(wobble)
                .map(|(time, wobble)| time - 1.5 + wobble),
        );
        let lateness = singing.lateness(&lyrics(&LINES), 120.0).unwrap();
        assert_eq!(lateness.lines, 12);
        assert!((lateness.seconds - 1.5).abs() < 0.05, "{lateness:?}");
        assert!(lateness.is_clear());

        // Early lines come out negative
        let singing = heard(10.0, LINES.iter().map(|time| time + 1.2));
        let lateness = singing.lateness(&lyrics(&LINES), 120.0).unwrap();
        assert!((lateness.seconds + 1.2).abs() < 1e-9, "{lateness:?}");
    }

    #[test]
    fn test_only_lines_heard_in_full_count() {
        let singing = heard(10.0, LINES.iter().map(|time| time - 1.0));
        // At 50 s, the lines up to 47.0 s have been heard with their whole search window
        let lateness = singing.lateness(&lyrics(&LINES), 50.0).unwrap();
        assert_eq!(lateness.lines, 4);
        assert!(!lateness.is_clear(), "too few lines yet");
        // Listening from 30 s, the lines before 32.5 s are left out: their singing may have
        // been missed
        let singing = heard(30.0, LINES.iter().map(|time| time - 1.0));
        assert_eq!(singing.lateness(&lyrics(&LINES), 120.0).unwrap().lines, 10);
    }

    #[test]
    fn test_scattered_starts_are_not_clear() {
        // Starts that have nothing to do with the lines (an instrument in the centre)
        let offsets = [
            -2.2, 1.9, -0.4, 2.4, -1.6, 0.9, -2.4, 1.3, -0.9, 2.1, -1.2, 0.3,
        ];
        let singing = heard(
            10.0,
            LINES
                .iter()
                .zip(offsets)
                .map(|(time, offset)| time + offset),
        );
        let lateness = singing.lateness(&lyrics(&LINES), 120.0).unwrap();
        assert_eq!(lateness.lines, 12);
        assert!(!lateness.is_clear(), "{lateness:?}");
        // No singing at all near the lines
        assert_eq!(heard(10.0, [5.0]).lateness(&lyrics(&LINES), 120.0), None);
    }
}
