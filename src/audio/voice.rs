//! Vocal onset detection in stereo audio.
//!
//! In typical stereo mixes, vocals are panned to the center (equal in left and right channels)
//! while instruments are panned toward the sides. Summing channels (L+R) isolates vocal energy,
//! while subtracting them (L-R) attenuates it. Comparing mid vs. side energy in the vocal range
//! (250–3500 Hz) detects when singing begins, aligning lyric timestamps with playback.

use std::array;
use std::collections::VecDeque;
use std::f64::consts::{FRAC_1_SQRT_2, PI};

use super::SAMPLE_RATE_HZ;

const VOICE_LOW_HZ: f64 = 250.0;
const VOICE_HIGH_HZ: f64 = 3500.0;

/// Samples per 50 ms frame at 16 kHz.
const FRAME_SAMPLES: u64 = SAMPLE_RATE_HZ as u64 / 20;

/// Floor and ceiling (dB, mid over side) for the unsung baseline between lines.
const UNSUNG_DB: f64 = 6.0;
const MAX_UNSUNG_DB: f64 = 10.0;
/// Required rise above the unsung reference for sustained singing and the onset frame (dB).
const MIN_RISE_DB: f64 = 3.0;
const MIN_ONSET_RISE_DB: f64 = 2.0;

/// Frames inspected before (0.5 s) and after (0.2 s) a candidate onset.
const BEFORE: usize = 10;
const MIN_UNSUNG_BEFORE: usize = 8;
const AFTER: usize = 4;
const MIN_SUNG_AFTER: usize = 3;
/// Minimum spacing between distinct onsets (0.8 s).
const MIN_FRAMES_APART: u64 = 16;

#[derive(Clone, Copy)]
enum Pass {
    High,
    Low,
}

/// Second-order RBJ biquad filter matching FFmpeg's `highpass` and `lowpass`.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    /// The last two inputs and outputs.
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    /// Lets through what is above or below `frequency_hz`, as `pass` says.
    fn new(frequency_hz: f64, pass: Pass) -> Self {
        let (sin, cos) = (2.0 * PI * frequency_hz / f64::from(SAMPLE_RATE_HZ)).sin_cos();
        // Q of 1/√2: the flattest response
        let alpha = sin / (2.0 * FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b = match pass {
            Pass::High => [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
            Pass::Low => [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0],
        };
        Self {
            b: b.map(|coefficient| coefficient / a0),
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    fn filter(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// Keeps the voice's frequency range: each edge filtered twice, as
/// `ffmpeg -af highpass=f=250,highpass=f=250,lowpass=f=3500,lowpass=f=3500` does.
struct VoiceBand([Biquad; 4]);

impl VoiceBand {
    fn new() -> Self {
        let high = Biquad::new(VOICE_LOW_HZ, Pass::High);
        let low = Biquad::new(VOICE_HIGH_HZ, Pass::Low);
        Self([high, high, low, low])
    }

    fn filter(&mut self, x: f64) -> f64 {
        self.0.iter_mut().fold(x, |x, stage| stage.filter(x))
    }
}

/// Finds where singing starts in a stream of stereo samples.
pub(super) struct VoiceDetector {
    centre: VoiceBand,
    sides: VoiceBand,
    /// The two signals' energy so far in the frame being measured.
    centre_energy: f64,
    sides_energy: f64,
    /// Samples taken in.
    samples: u64,
    /// How much louder (dB) the centre was than the sides, in each of the last frames.
    levels: VecDeque<f64>,
    /// The frame the last start was found in.
    last_start: Option<u64>,
}

impl Default for VoiceDetector {
    fn default() -> Self {
        Self {
            centre: VoiceBand::new(),
            sides: VoiceBand::new(),
            centre_energy: 0.0,
            sides_energy: 0.0,
            samples: 0,
            levels: VecDeque::with_capacity(BEFORE + AFTER + 1),
            last_start: None,
        }
    }
}

impl VoiceDetector {
    /// Samples taken in so far.
    pub(super) fn samples(&self) -> u64 {
        self.samples
    }

    /// Takes in the next stereo sample (left, right). Returns the number of the sample where
    /// singing started (counting from 0, the first sample taken in), once that is clear: four
    /// frames (200 ms) after it.
    pub(super) fn push(&mut self, [left, right]: [i16; 2]) -> Option<u64> {
        let (left, right) = (f64::from(left), f64::from(right));
        let centre = self.centre.filter(left + right);
        let sides = self.sides.filter(left - right);
        self.centre_energy += centre * centre;
        self.sides_energy += sides * sides;
        self.samples += 1;
        if !self.samples.is_multiple_of(FRAME_SAMPLES) {
            return None;
        }

        // The 1s keep silence (both energies 0) at 0 dB
        let level = 10.0 * ((self.centre_energy + 1.0) / (self.sides_energy + 1.0)).log10();
        self.centre_energy = 0.0;
        self.sides_energy = 0.0;
        self.levels.push_back(level);
        if self.levels.len() > BEFORE + AFTER {
            self.levels.pop_front();
        }
        if self.levels.len() < BEFORE + AFTER {
            return None;
        }

        // The frame singing may start in, with `BEFORE` frames before it and `AFTER - 1` after
        let frame = self.samples / FRAME_SAMPLES - AFTER as u64;
        let mut before: [f64; BEFORE] = array::from_fn(|i| self.levels[i]);
        before.sort_by(f64::total_cmp);
        // At least `MIN_UNSUNG_BEFORE` of the `BEFORE` frames sit at or below this level
        let before_ceiling = before[MIN_UNSUNG_BEFORE - 1];
        if before_ceiling >= MAX_UNSUNG_DB {
            return None;
        }
        let reference = before_ceiling.max(UNSUNG_DB);
        let sung_threshold = reference + MIN_RISE_DB;
        let onset_threshold = reference + MIN_ONSET_RISE_DB;
        let sung_after = self
            .levels
            .iter()
            .skip(BEFORE)
            .filter(|&&level| level >= sung_threshold)
            .count();
        let starts = sung_after >= MIN_SUNG_AFTER
            && self.levels[BEFORE] >= onset_threshold
            && self
                .last_start
                .is_none_or(|last| frame - last > MIN_FRAMES_APART);
        if !starts {
            return None;
        }
        self.last_start = Some(frame);
        Some(frame * FRAME_SAMPLES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = SAMPLE_RATE_HZ as f64;

    /// A sample of noise, the same from run to run (a linear congruential generator).
    fn noise(state: &mut u32) -> f64 {
        *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(*state >> 8) / f64::from(1u32 << 24) - 0.5
    }

    /// `seconds` of instruments spread to both sides (noise of its own in each channel), with a
    /// voice in the centre (a 600 Hz tone in both) while `sung` says so.
    fn song(seconds: f64, sung: impl Fn(f64) -> bool) -> Vec<[i16; 2]> {
        let mut state = 1;
        (0..(seconds * RATE) as usize)
            .map(|i| {
                let time = i as f64 / RATE;
                let voice = if sung(time) {
                    (2.0 * PI * 600.0 * time).sin() * 8000.0
                } else {
                    0.0
                };
                let left = noise(&mut state) * 6000.0 + voice;
                let right = noise(&mut state) * 6000.0 + voice;
                [left as i16, right as i16]
            })
            .collect()
    }

    fn starts(samples: &[[i16; 2]]) -> Vec<f64> {
        let mut detector = VoiceDetector::default();
        samples
            .iter()
            .filter_map(|&sample| detector.push(sample))
            .map(|start| start as f64 / RATE)
            .collect()
    }

    #[test]
    fn test_finds_where_the_centre_voice_starts() {
        // Lines sung from 2 s, 6.5 s and 11 s, each for 3 s
        let lines = [2.0, 6.5, 11.0];
        let sung = |time: f64| {
            lines
                .iter()
                .any(|&start| (start..start + 3.0).contains(&time))
        };
        let found = starts(&song(15.0, sung));
        assert_eq!(found.len(), lines.len(), "{found:?}");
        for (found, line) in found.iter().zip(lines) {
            assert!((found - line).abs() <= 0.05, "{found} for {line}");
        }
    }

    #[test]
    fn test_finds_voice_starts_above_centred_instruments() {
        // Instruments partially centred (baseline around 7–8 dB above sides, as on 伍思凯's
        // "爱与愁"), with singing raising the centre well above that baseline
        fn centred_song(seconds: f64, sung: impl Fn(f64) -> bool) -> Vec<[i16; 2]> {
            let mut state = 7;
            (0..(seconds * RATE) as usize)
                .map(|i| {
                    let time = i as f64 / RATE;
                    let centred_bed = noise(&mut state) * 4250.0;
                    let side_left = noise(&mut state) * 3000.0;
                    let side_right = noise(&mut state) * 3000.0;
                    let voice = if sung(time) {
                        (2.0 * PI * 600.0 * time).sin() * 5000.0
                    } else {
                        0.0
                    };
                    [
                        (centred_bed + side_left + voice) as i16,
                        (centred_bed + side_right + voice) as i16,
                    ]
                })
                .collect()
        }

        let lines = [2.0, 6.5, 11.0];
        let sung = |time: f64| {
            lines
                .iter()
                .any(|&start| (start..start + 3.0).contains(&time))
        };
        let found = starts(&centred_song(15.0, sung));
        assert_eq!(found.len(), lines.len(), "{found:?}");
        for (found, line) in found.iter().zip(lines) {
            assert!((found - line).abs() <= 0.05, "{found} for {line}");
        }
        // The centred instruments alone trigger nothing
        assert!(starts(&centred_song(10.0, |_| false)).is_empty());
    }

    #[test]
    fn test_hears_nothing_in_instruments_alone_or_in_mono() {
        assert!(starts(&song(10.0, |_| false)).is_empty());
        // Mono (both channels the same) has no sides at all, so nothing ever stands out
        let mono: Vec<[i16; 2]> = song(10.0, |time| time > 5.0)
            .iter()
            .map(|&[left, _]| [left, left])
            .collect();
        assert!(starts(&mono).is_empty());
    }
}
