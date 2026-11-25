//! Sliding PCM audio ring buffer shared across capture, fingerprinting, and voice detection.

use std::collections::VecDeque;
use std::mem;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::capture::CaptureSource;
use super::voice::VoiceDetector;

pub const SAMPLE_RATE_HZ: u32 = 16_000;
/// Capacity of the sliding audio buffer: 12 seconds at 16 kHz.
/// 12 seconds provides sufficient acoustic material for Shazam recognition on difficult matches.
const BUFFER_SAMPLES: usize = SAMPLE_RATE_HZ as usize * 12;
/// Maximum capture gap (pause or network rebuffering) before clearing stale audio.
const CAPTURE_GAP: Duration = Duration::from_millis(250);
/// Maximum buffered vocal onset events retained before eviction.
const MAX_VOICE_STARTS: usize = 64;

/// Downmixes a stereo sample pair [left, right] into a single mono sample.
pub(super) fn mono([left, right]: [i16; 2]) -> i16 {
    ((i32::from(left) + i32::from(right)) / 2) as i16
}

/// Computes the estimated playback timestamp for a sample relative to `captured_at`.
fn audible_instant(
    captured_at: Instant,
    samples_before: u64,
    output_delay_seconds: f64,
) -> Instant {
    let ago = Duration::from_secs_f64(samples_before as f64 / f64::from(SAMPLE_RATE_HZ));
    captured_at.checked_sub(ago).unwrap_or(captured_at)
        + Duration::from_secs_f64(output_delay_seconds)
}

#[derive(Default)]
struct PcmRing {
    /// Rolling mono audio buffer.
    samples: VecDeque<i16>,
    voice: VoiceDetector,
    /// Pending vocal onset timestamps.
    voice_starts: Vec<Instant>,
    last_push_at: Option<Instant>,
    /// Estimated hardware output latency.
    output_delay_seconds: f64,
    /// True if latency is measured from mpv loopback directly rather than estimated via ffmpeg.
    exact_timing: bool,
}

/// Snapshot of the rolling PCM audio buffer.
pub struct Snapshot {
    /// Mono audio samples ordered chronologically.
    pub samples: Vec<i16>,
    /// Estimated audible timestamp of `samples[0]`.
    pub audible_at: Instant,
    /// True if timing is derived from direct loopback capture.
    pub exact_timing: bool,
}

/// Thread-safe handle to the sliding PCM audio buffer and vocal onset detector.
#[derive(Clone, Default)]
pub struct PcmTap(Arc<Mutex<PcmRing>>);

impl PcmTap {
    #[expect(
        clippy::disallowed_methods,
        reason = "a capture task that panicked holding the buffer must not stop Shazam and the \
                  lyrics' timing check from reading it"
    )]
    fn lock(&self) -> MutexGuard<'_, PcmRing> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Clears the buffer when the capture source changes.
    pub(super) fn reset(&self, source: CaptureSource) {
        let mut ring = self.lock();
        ring.samples.clear();
        ring.voice = VoiceDetector::default();
        ring.last_push_at = None;
        ring.output_delay_seconds = source.output_delay_seconds();
        ring.exact_timing = matches!(source, CaptureSource::Monitor { .. });
    }

    /// Takes in stereo samples read at `now`, the last of them captured then.
    pub(super) fn push(&self, samples: &[[i16; 2]], now: Instant) {
        let mut ring = self.lock();
        // Discard audio from before a gap so timestamps stay contiguous
        if ring
            .last_push_at
            .is_some_and(|last_push| now.saturating_duration_since(last_push) > CAPTURE_GAP)
        {
            ring.samples.clear();
            ring.voice = VoiceDetector::default();
        }
        let overflow = (ring.samples.len() + samples.len()).saturating_sub(BUFFER_SAMPLES);
        ring.samples.drain(..overflow);
        ring.samples.extend(samples.iter().copied().map(mono));

        let starts: Vec<u64> = samples
            .iter()
            .filter_map(|&sample| ring.voice.push(sample))
            .collect();
        // Timed back from the last sample taken in, captured `now`
        let (taken, delay_seconds) = (ring.voice.samples(), ring.output_delay_seconds);
        for start in starts {
            let audible_at = audible_instant(now, taken - start, delay_seconds);
            ring.voice_starts.push(audible_at);
        }
        let excess = ring.voice_starts.len().saturating_sub(MAX_VOICE_STARTS);
        ring.voice_starts.drain(..excess);
        ring.last_push_at = Some(now);
    }

    /// When singing started in what was heard since the last call (see `voice.rs`): the
    /// instants it was, or will be, audible, as `snapshot` gives them.
    pub fn take_voice_starts(&self) -> Vec<Instant> {
        mem::take(&mut self.lock().voice_starts)
    }

    /// The buffered samples, when the first of them was (or will be) audible, and whether that
    /// timing is exact, if at least `min_samples` are buffered. All three are read under one
    /// lock so a capture source switch cannot mix samples from one source with the label of
    /// another.
    pub fn snapshot(&self, min_samples: usize) -> Option<Snapshot> {
        let ring = self.lock();
        if ring.samples.len() < min_samples {
            return None;
        }
        let tail_at = ring.last_push_at?;
        let buffered = ring.samples.len() as u64;
        Some(Snapshot {
            samples: ring.samples.iter().copied().collect(),
            audible_at: audible_instant(tail_at, buffered, ring.output_delay_seconds),
            exact_timing: ring.exact_timing,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_gap_reset() {
        let pcm = PcmTap::default();
        pcm.reset(CaptureSource::Monitor { mpv_pid: 1 });
        let start = Instant::now();
        pcm.push(&[[1, 1]; 16_000], start);
        assert!(pcm.snapshot(16_000).is_some());
        // A capture gap (e.g. pause) discards stale audio so timestamps stay contiguous
        pcm.push(&[[2, 2]; 256], start + Duration::from_secs(2));
        assert!(pcm.snapshot(16_000).is_none());
        let snapshot = pcm.snapshot(256).unwrap();
        assert_eq!(snapshot.samples.len(), 256);
        assert!(snapshot.audible_at > start + Duration::from_secs(2));
        assert!(snapshot.exact_timing);

        // Switching to the ffmpeg fallback marks subsequent snapshots as estimated
        pcm.reset(CaptureSource::Network);
        pcm.push(&[[3, 3]; 256], start + Duration::from_secs(3));
        assert!(!pcm.snapshot(256).unwrap().exact_timing);
    }
}
