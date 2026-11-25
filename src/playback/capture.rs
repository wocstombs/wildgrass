//! Real-time audio capture pipeline for spectrum visualization and fingerprinting.
//!
//! Feeds the sliding PCM buffer for Shazam queries, drives the frequency visualizer,
//! and tracks vocal onset timestamps. Dropping a `Capture` terminates background processing.

use std::time::Instant;

use tokio::sync::watch;

use crate::audio::{self, PcmTap};
use crate::spectrum::{Spectrum, SpectrumSmoother, SILENCE};
use crate::task::Task;

pub(super) struct Capture {
    /// Rolling audio buffer for Shazam fingerprinting and vocal onset detection.
    pcm: PcmTap,
    /// Channel instructing the capture task to pause reading while playback is idle.
    idle_sender: watch::Sender<bool>,
    /// Delayed, smoothed frequency bands rendered by the visualizer.
    spectrum: SpectrumSmoother,
    spectrum_receiver: watch::Receiver<Spectrum>,
    _task: Task<()>,
}

impl Capture {
    /// Starts capturing audio from `mpv` (`mpv_pid`), falling back to `stream_url` via `ffmpeg`.
    pub(super) fn start(stream_url: String, mpv_pid: Option<u32>) -> Self {
        let (spectrum_sender, spectrum_receiver) = watch::channel(SILENCE);
        let (idle_sender, idle_receiver) = watch::channel(false);
        let pcm = PcmTap::default();
        let task = Task::spawn(audio::capture(
            stream_url,
            mpv_pid,
            pcm.clone(),
            spectrum_sender,
            idle_receiver,
        ));
        Self {
            pcm,
            idle_sender,
            spectrum: SpectrumSmoother::default(),
            spectrum_receiver,
            _task: task,
        }
    }

    /// Creates a silent capture instance for unit testing.
    #[cfg(test)]
    pub(super) fn silent() -> Self {
        Self {
            pcm: PcmTap::default(),
            idle_sender: watch::channel(false).0,
            spectrum: SpectrumSmoother::default(),
            spectrum_receiver: watch::channel(SILENCE).1,
            _task: Task::spawn(async {}),
        }
    }

    pub(super) fn pcm(&self) -> PcmTap {
        self.pcm.clone()
    }

    /// When singing started in what was heard since the last call (see `audio/voice.rs`).
    pub(super) fn take_voice_starts(&self) -> Vec<Instant> {
        self.pcm.take_voice_starts()
    }

    pub(super) fn spectrum(&self) -> &Spectrum {
        self.spectrum.current()
    }

    /// Feeds any newly captured analysis into the display delay queue and advances smoothing.
    pub(super) fn animate(&mut self, now: Instant) {
        if self.spectrum_receiver.has_changed().unwrap_or(false) {
            let latest = *self.spectrum_receiver.borrow_and_update();
            self.spectrum.push(latest, now);
        }
        self.spectrum.advance(now);
    }

    /// Lowers the wave slowly while playback is paused (see `SpectrumSmoother::fall`).
    pub(super) fn fall(&mut self, now: Instant) {
        self.spectrum.fall(now);
    }

    /// Stops or resumes reading the audio.
    pub(super) fn set_idle(&mut self, idle: bool) {
        if !idle {
            // Discard pre-idle analyses before resuming capture so animation continues cleanly.
            self.spectrum_receiver.mark_unchanged();
            self.spectrum.clear_pending();
        }
        self.idle_sender.send_replace(idle);
    }
}
