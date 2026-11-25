//! Fast Fourier Transform (FFT) spectrum analysis for the Braille soundwave visualizer.
//!
//! Transforms 16 ms audio hops into 72 logarithmically spaced, dB-scaled frequency bands.

use std::array;
use std::collections::VecDeque;
use std::f32::consts::PI;
use std::sync::Arc;
use std::time::{Duration, Instant};

use realfft::{RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex;

pub const SPECTRUM_BANDS: usize = 72;
pub type Spectrum = [f32; SPECTRUM_BANDS];
pub const SILENCE: Spectrum = [0.0; SPECTRUM_BANDS];

/// Samples per hop: 16 ms at 16 kHz.
pub const HOP_LENGTH: usize = 256;
const FFT_LENGTH: usize = 1024;

/// Delay applied between capturing a spectrum frame and driving the visualizer with it.
const DISPLAY_DELAY: Duration = Duration::from_millis(100);
/// Exponential attack time constant in seconds for rising bands.
const ATTACK_TIME_CONSTANT_SECONDS: f32 = 0.020;
/// Exponential decay time constant in seconds for falling bands.
const DECAY_TIME_CONSTANT_SECONDS: f32 = 0.180;
/// Cap on a single smoothing step so OS stalls do not snap band levels immediately.
const MAX_STEP_SECONDS: f32 = 0.100;
/// Level a band loses per second while playback is paused: one dot of the six-dot wave
/// (`ui::quantize_braille_dots` scales levels by 6.4), so a full bar reaches the bottom in ~5 s.
const PAUSED_FALL_PER_SECOND: f32 = 1.0 / 6.4;

/// FFT bin mapping and spectral tilt parameters for one visualizer band.
#[derive(Clone, Copy)]
struct BandLayout {
    start_bin: usize,
    end_bin: usize,
    /// Interpolation weight between `start_bin` and adjacent bin for narrow bass bands.
    fraction: f32,
    /// High-frequency emphasis countering 1/f spectral roll-off.
    tilt: f32,
}

/// Sliding-window FFT analyzer across a 64 ms window (1024 samples).
pub struct SpectrumAnalyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    window: [f32; FFT_LENGTH],
    bands: [BandLayout; SPECTRUM_BANDS],
    max_bin: usize,
    /// The newest `FFT_LENGTH` samples, oldest first, scaled to `[-1.0, 1.0]`.
    history: [f32; FFT_LENGTH],
    time_buffer: [f32; FFT_LENGTH],
    frequency_buffer: Vec<Complex<f32>>,
    magnitudes: [f32; FFT_LENGTH / 2 + 1],
}

impl Default for SpectrumAnalyzer {
    fn default() -> Self {
        use rustfft::num_traits::Zero;

        let window =
            array::from_fn(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / (FFT_LENGTH - 1) as f32).cos()));
        let bands: [BandLayout; SPECTRUM_BANDS] = array::from_fn(|band| {
            let start = band as f32 / SPECTRUM_BANDS as f32;
            let end = (band + 1) as f32 / SPECTRUM_BANDS as f32;
            let start_position = 1.0 + start * 24.0 + start.powf(2.4) * 295.0;
            let end_position = 1.0 + end * 24.0 + end.powf(2.4) * 295.0;
            let start_bin = (start_position.floor() as usize).clamp(1, FFT_LENGTH / 2 - 1);
            let end_bin = (end_position.ceil() as usize).clamp(start_bin + 1, FFT_LENGTH / 2);
            BandLayout {
                start_bin,
                end_bin,
                fraction: start_position - start_bin as f32,
                tilt: 1.0 + band as f32 * 0.08,
            }
        });
        Self {
            fft: RealFftPlanner::<f32>::new().plan_fft_forward(FFT_LENGTH),
            window,
            max_bin: bands[SPECTRUM_BANDS - 1].end_bin,
            bands,
            history: [0.0; FFT_LENGTH],
            time_buffer: [0.0; FFT_LENGTH],
            frequency_buffer: vec![Complex::zero(); FFT_LENGTH / 2 + 1],
            magnitudes: [0.0; FFT_LENGTH / 2 + 1],
        }
    }
}

impl SpectrumAnalyzer {
    pub fn push_hop(&mut self, hop: &[i16; HOP_LENGTH]) {
        self.history.copy_within(HOP_LENGTH.., 0);
        let tail = self.history[FFT_LENGTH - HOP_LENGTH..].iter_mut().zip(hop);
        for (slot, &sample) in tail {
            *slot = f32::from(sample) * (1.0 / 32768.0);
        }
    }

    /// Analyzes the current window into normalized `[0.0, 1.0]` band levels.
    pub fn bands(&mut self) -> Spectrum {
        let windowed = self
            .time_buffer
            .iter_mut()
            .zip(&self.history)
            .zip(&self.window);
        for ((slot, &sample), &weight) in windowed {
            *slot = sample * weight;
        }
        // Only fails on mismatched buffer lengths, which are fixed by construction
        let _ = self
            .fft
            .process(&mut self.time_buffer, &mut self.frequency_buffer);

        let magnitude_scale = 1.0 / (FFT_LENGTH as f32 * 0.12);
        for (magnitude, bin) in self.magnitudes[1..=self.max_bin]
            .iter_mut()
            .zip(&self.frequency_buffer[1..=self.max_bin])
        {
            *magnitude = (bin.re * bin.re + bin.im * bin.im).sqrt() * magnitude_scale;
        }

        self.bands.map(|layout| {
            let raw_magnitude = if layout.end_bin - layout.start_bin <= 2 {
                let index = layout.start_bin;
                self.magnitudes[index] * (1.0 - layout.fraction)
                    + self.magnitudes[index + 1] * layout.fraction
            } else {
                self.magnitudes[layout.start_bin..=layout.end_bin]
                    .iter()
                    .copied()
                    .fold(0.0_f32, f32::max)
            };
            let decibels = 20.0 * (raw_magnitude * layout.tilt + 1e-6).log10();
            ((decibels + 38.0) / 34.0).clamp(0.0, 1.0)
        })
    }
}

struct DelayedSpectrum {
    recorded_at: Instant,
    bands: Spectrum,
}

/// Delays incoming spectrum frames and applies time-based asymmetric attack/decay smoothing.
pub struct SpectrumSmoother {
    pending: VecDeque<DelayedSpectrum>,
    target: Spectrum,
    current: Spectrum,
    last_updated: Option<Instant>,
}

impl Default for SpectrumSmoother {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            target: SILENCE,
            current: SILENCE,
            last_updated: None,
        }
    }
}

impl SpectrumSmoother {
    /// Returns the smoothed band levels currently ready to render.
    pub fn current(&self) -> &Spectrum {
        &self.current
    }

    /// Enqueues a newly analyzed spectrum frame captured at `now`.
    pub fn push(&mut self, bands: Spectrum, now: Instant) {
        self.pending.push_back(DelayedSpectrum {
            recorded_at: now,
            bands,
        });
    }

    /// Drops queued frames and resets the animation clock while preserving `current`.
    pub fn clear_pending(&mut self) {
        self.pending.clear();
        self.target = SILENCE;
        self.last_updated = None;
    }

    /// Releases any queued frames whose delay has elapsed and advances smoothing to `now`.
    pub fn advance(&mut self, now: Instant) {
        while self
            .pending
            .front()
            .is_some_and(|entry| now.saturating_duration_since(entry.recorded_at) >= DISPLAY_DELAY)
        {
            if let Some(ready) = self.pending.pop_front() {
                self.target = ready.bands;
            }
        }

        let elapsed = self.step(now);
        if elapsed == 0.0 {
            return;
        }

        let attack_retain = (-elapsed / ATTACK_TIME_CONSTANT_SECONDS).exp();
        let decay_retain = (-elapsed / DECAY_TIME_CONSTANT_SECONDS).exp();
        let bands = self.current.iter_mut().zip(&self.target);
        for (drawn, &target) in bands {
            let retain = if target > *drawn {
                attack_retain
            } else {
                decay_retain
            };
            *drawn = *drawn * retain + target * (1.0 - retain);
        }
    }

    /// Lowers every band at a steady `PAUSED_FALL_PER_SECOND`, so the wave settles slowly
    /// while playback is paused.
    pub fn fall(&mut self, now: Instant) {
        let drop = self.step(now) * PAUSED_FALL_PER_SECOND;
        for level in &mut self.current {
            *level = (*level - drop).max(0.0);
        }
    }

    /// Returns the seconds since the previous step (at most `MAX_STEP_SECONDS`) and restarts
    /// the clock at `now`.
    fn step(&mut self, now: Instant) -> f32 {
        let elapsed = self.last_updated.map_or(0.0, |previous| {
            now.saturating_duration_since(previous)
                .as_secs_f32()
                .min(MAX_STEP_SECONDS)
        });
        self.last_updated = Some(now);
        elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smoother_delays_incoming_bands_by_display_delay() {
        let mut smoother = SpectrumSmoother::default();
        let start = Instant::now();
        let loud = [1.0; SPECTRUM_BANDS];

        smoother.push(loud, start);
        smoother.advance(start);
        smoother.advance(start + Duration::from_millis(90));
        assert_eq!(*smoother.current(), SILENCE);

        smoother.advance(start + DISPLAY_DELAY);
        assert!(smoother.current()[0] > 0.3);
    }

    #[test]
    fn test_smoother_decays_slower_than_attack() {
        let mut smoother = SpectrumSmoother::default();
        let start = Instant::now();
        let loud = [1.0; SPECTRUM_BANDS];

        smoother.push(loud, start);
        smoother.advance(start);
        let release = start + DISPLAY_DELAY;
        smoother.advance(release - Duration::from_millis(16));
        smoother.advance(release);
        let rise_in_one_frame = smoother.current()[0];

        // Hold peak long enough to settle near 1.0, then drop to silence.
        let mut time = release;
        for _ in 0..10 {
            time += Duration::from_millis(16);
            smoother.advance(time);
        }
        let peak = smoother.current()[0];
        assert!(peak > 0.99);

        smoother.push(SILENCE, time);
        let drop_start = time + DISPLAY_DELAY;
        smoother.advance(drop_start - Duration::from_millis(16));
        smoother.advance(drop_start);
        let fall_in_one_frame = peak - smoother.current()[0];
        assert!(fall_in_one_frame < rise_in_one_frame / 4.0);

        // After 160 ms of decay from ~1.0, levels remain well above the baseline threshold.
        let mut decay_time = drop_start;
        for _ in 0..10 {
            decay_time += Duration::from_millis(16);
            smoother.advance(decay_time);
        }
        assert!(smoother.current()[0] > 0.35);
    }

    #[test]
    fn test_smoother_is_frame_rate_independent() {
        let start = Instant::now();
        let loud = [1.0; SPECTRUM_BANDS];

        let mut coarse = SpectrumSmoother::default();
        let mut fine = SpectrumSmoother::default();
        for smoother in [&mut coarse, &mut fine] {
            smoother.push(loud, start);
            smoother.advance(start);
            let mut time = start + DISPLAY_DELAY;
            smoother.advance(time - Duration::from_millis(16));
            for _ in 0..12 {
                smoother.advance(time);
                time += Duration::from_millis(16);
            }
            smoother.push(SILENCE, time);
            smoother.advance(time + DISPLAY_DELAY - Duration::from_millis(16));
        }

        let release = start + DISPLAY_DELAY + Duration::from_millis(12 * 16) + DISPLAY_DELAY;
        for step in 0..4 {
            coarse.advance(release + Duration::from_millis(step * 16));
        }
        for step in 0..16 {
            fine.advance(release + Duration::from_millis(step * 4));
        }

        // Compare both at the exact same final instant (release + 48 ms vs release + 60 ms).
        coarse.advance(release + Duration::from_millis(64));
        fine.advance(release + Duration::from_millis(64));
        assert!((coarse.current()[0] - fine.current()[0]).abs() < 1e-4);
    }

    #[test]
    fn test_smoother_falls_one_dot_per_second_while_paused() {
        let mut smoother = SpectrumSmoother {
            current: [1.0; SPECTRUM_BANDS],
            ..SpectrumSmoother::default()
        };
        let start = Instant::now();
        smoother.fall(start);

        // Main-loop passes while paused are 50 ms apart
        let level_after = |smoother: &mut SpectrumSmoother, seconds: u64| {
            let mut time = start;
            while time < start + Duration::from_secs(seconds) {
                time += Duration::from_millis(50);
                smoother.fall(time);
            }
            smoother.current()[5]
        };
        assert!((level_after(&mut smoother, 1) - (1.0 - PAUSED_FALL_PER_SECOND)).abs() < 1e-4);
        // Below 1.5 / 6.4 the bar shows only its bottom dot
        assert!(level_after(&mut smoother, 5) < 1.5 / 6.4);
        assert_eq!(level_after(&mut smoother, 8), 0.0);
    }

    #[test]
    fn test_smoother_clear_pending_drops_queued_frames_and_keeps_current() {
        let mut smoother = SpectrumSmoother::default();
        let start = Instant::now();
        let loud = [0.8; SPECTRUM_BANDS];

        smoother.push(loud, start);
        smoother.advance(start);
        smoother.advance(start + DISPLAY_DELAY - Duration::from_millis(16));
        smoother.advance(start + DISPLAY_DELAY);
        let held = smoother.current()[0];
        assert!(held > 0.3);

        // Queue a stronger peak, then clear as if resuming from idle.
        smoother.push([1.0; SPECTRUM_BANDS], start + DISPLAY_DELAY);
        smoother.clear_pending();

        // First advance after clear establishes the clock without moving `current`.
        let resumed = start + Duration::from_secs(5);
        smoother.advance(resumed);
        assert_eq!(smoother.current()[0], held);

        // Subsequent ticks decay gently toward silence instead of applying the cleared peak.
        smoother.advance(resumed + Duration::from_millis(16));
        assert!(smoother.current()[0] < held);
        assert!(smoother.current()[0] > held * 0.85);
    }
}
