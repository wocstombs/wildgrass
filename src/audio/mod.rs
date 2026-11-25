//! Real-time 16 kHz stereo PCM audio capture.
//!
//! Provides downmixed mono audio for the Braille spectrum visualizer and Shazam acoustic fingerprinting,
//! and supplies stereo audio to the vocal onset detector (`voice.rs`) for lyric synchronization.

mod capture;
#[cfg(windows)]
mod loopback;
mod pcm_tap;
#[cfg(unix)]
mod pulse;
mod voice;

pub use self::capture::capture;
pub use self::pcm_tap::{PcmTap, Snapshot, SAMPLE_RATE_HZ};

// Platform-specific audio loopback monitor: PulseAudio/PipeWire on Unix, WASAPI on Windows.
// Both expose `NAME`, `start`, and `output_appears`.
#[cfg(windows)]
use self::loopback as monitor;
#[cfg(unix)]
use self::pulse as monitor;
