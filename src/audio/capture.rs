//! The capture task: reads the station's audio from `mpv`'s own output, or from an `ffmpeg`
//! decode of the stream when that output cannot be captured, into the `PcmTap` and the
//! visualizer.

use std::io;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::AsyncRead;
use tokio::process::{Child, Command};
use tokio::sync::watch;

use super::monitor;
use super::pcm_tap::{self, PcmTap};
use crate::log;
use crate::spectrum::{Spectrum, SpectrumAnalyzer, HOP_LENGTH, SILENCE};

/// Where a capture's PCM is read from.
pub(super) type PcmReader = Box<dyn AsyncRead + Unpin + Send>;

/// A running capture of 16 kHz stereo s16le PCM.
pub(super) struct Capture {
    pub(super) reader: PcmReader,
    /// The program writing it to its standard output (`parec` or `ffmpeg`), killed once it is no
    /// longer read. None for Windows' process loopback (`loopback.rs`), which runs inside
    /// wildgrass and ends when `reader` is dropped.
    pub(super) program: Option<Child>,
}

impl Capture {
    /// The capture of a program started with a piped standard output.
    pub(super) fn from_program(mut child: Child) -> Self {
        let stdout = child
            .stdout
            .take()
            .expect("started with a piped standard output");
        Self {
            reader: Box::new(stdout),
            program: Some(child),
        }
    }
}

/// Where the captured audio comes from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CaptureSource {
    /// `mpv`'s own output (its PulseAudio/PipeWire sink input, or on Windows its process
    /// loopback): exactly what is playing, with no second network connection to the station.
    Monitor { mpv_pid: u32 },
    /// An independent `ffmpeg` decode of the station, paced to real time.
    Network,
}

impl CaptureSource {
    /// Delay between a sample being captured from this source and it reaching the speakers.
    pub(super) fn output_delay_seconds(self) -> f64 {
        match self {
            Self::Monitor { .. } => 0.08,
            // mpv's playback lags an independent connection to the same stream by about this much
            Self::Network => 1.5,
        }
    }

    /// Starts the capture. `Ok(None)` when `mpv` has no output to monitor yet.
    async fn spawn(self, stream_url: &str) -> io::Result<Option<Capture>> {
        match self {
            Self::Monitor { mpv_pid } => monitor::start(mpv_pid).await,
            Self::Network => spawn_network_decoder(stream_url)
                .await
                .map(|child| Some(Capture::from_program(child))),
        }
    }
}

/// After this many monitors in a row end at once without audio, monitoring is given up.
const MAX_QUICK_MONITOR_EXITS: u32 = 3;

/// Whether `mpv`'s own output can be captured.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Monitoring {
    /// Capture it.
    Use { mpv_pid: u32 },
    /// It has not appeared yet (a slow stream): decode with `ffmpeg` meanwhile, and switch
    /// over as soon as it does.
    Awaited { mpv_pid: u32 },
    /// Not on this system (no PulseAudio/PipeWire, no process loopback on Windows, or the
    /// capture keeps stopping).
    Unavailable,
}

/// Captures the station's audio until the task is aborted, feeding `pcm` and publishing
/// visualizer bands on `spectrum_sender` for every read.
///
/// Prefers tapping `mpv`'s output (visualizer and lyrics locked to what is audible) and falls
/// back to decoding the stream with `ffmpeg` when monitoring it is unavailable.
/// The fallback is a second connection to the station, whose distance from what `mpv` plays is
/// not known, so it is left again as soon as `mpv`'s output can be captured.
pub async fn capture(
    stream_url: String,
    mpv_pid: Option<u32>,
    pcm: PcmTap,
    spectrum_sender: watch::Sender<Spectrum>,
    mut idle: watch::Receiver<bool>,
) {
    let mut monitoring = mpv_pid.map_or(Monitoring::Unavailable, |mpv_pid| Monitoring::Use {
        mpv_pid,
    });
    let mut quick_monitor_exits = 0u32;

    loop {
        let source = match monitoring {
            Monitoring::Use { mpv_pid } => CaptureSource::Monitor { mpv_pid },
            Monitoring::Awaited { .. } | Monitoring::Unavailable => CaptureSource::Network,
        };
        let Capture { reader, program } = match source.spawn(&stream_url).await {
            Ok(Some(capture)) => capture,
            Ok(None) => {
                log::error(
                    "audio: mpv's audio output has not appeared within 20 seconds; decoding \
                     the stream with ffmpeg until it does",
                );
                if let Monitoring::Use { mpv_pid } = monitoring {
                    monitoring = Monitoring::Awaited { mpv_pid };
                }
                continue;
            }
            Err(error) if source == CaptureSource::Network => {
                log::error(format!("audio: cannot start ffmpeg: {error}"));
                if error.kind() == io::ErrorKind::NotFound {
                    // Not installed: there is nothing left to capture with
                    return;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            Err(error) => {
                log::error(format!(
                    "audio: cannot capture mpv's output ({error}); decoding the stream with ffmpeg instead"
                ));
                monitoring = Monitoring::Unavailable;
                continue;
            }
        };

        pcm.reset(source);
        log::sync(match source {
            CaptureSource::Monitor { .. } => "capture: mpv's output",
            CaptureSource::Network => "capture: ffmpeg fallback, lyric timing is an estimate",
        });
        let started = Instant::now();
        let got_audio = match monitoring {
            Monitoring::Awaited { mpv_pid } => tokio::select! {
                got_audio = pump(reader, &pcm, &spectrum_sender, &mut idle) => got_audio,
                () = monitor::output_appears(mpv_pid) => {
                    monitoring = Monitoring::Use { mpv_pid };
                    true
                }
            },
            _ => pump(reader, &pcm, &spectrum_sender, &mut idle).await,
        };
        if let Some(mut program) = program {
            let _ = program.kill().await;
        }
        if spectrum_sender.send(SILENCE).is_err() {
            return;
        }

        if let CaptureSource::Monitor { .. } = source {
            // A monitor that dies instantly without audio means stream monitoring is unsupported
            let quick_exit = !got_audio && started.elapsed() < Duration::from_secs(2);
            quick_monitor_exits = if quick_exit {
                quick_monitor_exits + 1
            } else {
                0
            };
            if quick_monitor_exits >= MAX_QUICK_MONITOR_EXITS {
                log::error(format!(
                    "audio: {} keeps stopping without audio; decoding the stream with ffmpeg \
                     instead",
                    monitor::NAME
                ));
                monitoring = Monitoring::Unavailable;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// Reads PCM until the source ends (or nobody is watching the spectrum any more), returning
/// whether any audio arrived.
async fn pump(
    mut reader: PcmReader,
    pcm: &PcmTap,
    spectrum_sender: &watch::Sender<Spectrum>,
    idle: &mut watch::Receiver<bool>,
) -> bool {
    use tokio::io::AsyncReadExt;

    /// One stereo sample: left, then right, each s16le.
    const SAMPLE_BYTES: usize = 4;
    const HOP_BYTES: usize = HOP_LENGTH * SAMPLE_BYTES;
    let mut analyzer = SpectrumAnalyzer::default();
    let mut got_audio = false;
    let mut pending: Vec<u8> = Vec::with_capacity(16 * 1024);
    let mut chunk = [0u8; 4096];

    loop {
        // Stop draining while playback is idle so the decoder is backpressured to zero CPU
        if idle.wait_for(|&is_idle| !is_idle).await.is_err() {
            return got_audio;
        }
        let read = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => return got_audio,
            Ok(read) => read,
        };
        got_audio = true;
        pending.extend_from_slice(&chunk[..read]);

        let whole = pending.len() / HOP_BYTES * HOP_BYTES;
        if whole == 0 {
            continue;
        }
        let now = Instant::now();
        let samples: Vec<[i16; 2]> = pending[..whole]
            .as_chunks::<SAMPLE_BYTES>()
            .0
            .iter()
            .map(|&[left_low, left_high, right_low, right_high]| {
                [
                    i16::from_le_bytes([left_low, left_high]),
                    i16::from_le_bytes([right_low, right_high]),
                ]
            })
            .collect();
        pending.drain(..whole);
        pcm.push(&samples, now);
        for hop in samples.as_chunks::<HOP_LENGTH>().0 {
            analyzer.push_hop(&hop.map(pcm_tap::mono));
        }

        // Only the newest window is visible, so analyze once per read
        if spectrum_sender.send(analyzer.bands()).is_err() {
            return got_audio;
        }
    }
}

/// Builds a quiet background command detached from the console on Windows.
fn background_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Decodes the station independently with `ffmpeg`, paced to real time (`-re`).
async fn spawn_network_decoder(stream_url: &str) -> io::Result<Child> {
    let direct_url = resolve_direct_audio_url(stream_url).await;
    background_command("ffmpeg")
        .args([
            "-loglevel",
            "quiet",
            "-re",
            "-i",
            &direct_url,
            "-vn",
            "-f",
            "s16le",
            "-ac",
            "2",
            "-ar",
            "16000",
            "-",
        ])
        .spawn()
}

/// Resolves YouTube live/watch URLs via `yt-dlp` into a direct media stream URL for `ffmpeg`.
/// Anything else, or a URL `yt-dlp` cannot resolve, is returned as it is.
async fn resolve_direct_audio_url(stream_url: &str) -> String {
    let lower = stream_url.to_ascii_lowercase();
    if !(lower.contains("youtube.com/") || lower.contains("youtu.be/")) {
        return stream_url.to_string();
    }
    let resolved = background_command("yt-dlp")
        .args(["-g", "-f", "bestaudio/best", "--no-playlist", stream_url])
        .output()
        .await;
    let direct = match &resolved {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string),
        _ => None,
    };
    direct.unwrap_or_else(|| {
        let reason = match resolved {
            Ok(output) => output.status.to_string(),
            Err(error) => error.to_string(),
        };
        log::error(format!(
            "audio: yt-dlp could not resolve {stream_url}: {reason}"
        ));
        stream_url.to_string()
    })
}

#[cfg(test)]
mod tests {
    use std::f32::consts::PI;
    use std::fs;

    use super::*;
    use crate::test_util::ScratchDir;

    #[tokio::test]
    async fn test_capture_emits_realtime_bands() {
        // Write a tiny 0.25s 16kHz mono WAV file with 440Hz + 1500Hz sine waves
        let scratch = ScratchDir::create();
        let wav_path = scratch.path().join("spectrum.wav");
        let sample_count: u32 = 4000;
        let data_bytes = sample_count * 2;
        let mut wav = Vec::with_capacity(44 + data_bytes as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&16000u32.to_le_bytes()); // 16kHz
        wav.extend_from_slice(&32000u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_bytes.to_le_bytes());
        for i in 0..sample_count {
            let time = i as f32 / 16000.0;
            let sample = ((2.0 * PI * 440.0 * time).sin() * 0.5
                + (2.0 * PI * 1500.0 * time).sin() * 0.4)
                * 24000.0;
            wav.extend_from_slice(&(sample as i16).to_le_bytes());
        }
        fs::write(&wav_path, &wav).unwrap();

        let (sender, mut receiver) = watch::channel(SILENCE);
        let (_idle_sender, idle) = watch::channel(false);
        let pcm = PcmTap::default();
        let path = wav_path.to_string_lossy().to_string();
        let handle = tokio::spawn(capture(path, None, pcm.clone(), sender, idle));

        // Wait for a few 16ms hops so the 1024-sample window fills with signal
        let mut max_seen = 0.0f32;
        for _ in 0..8 {
            let _ = tokio::time::timeout(Duration::from_millis(200), receiver.changed()).await;
            max_seen = receiver.borrow().iter().copied().fold(max_seen, f32::max);
            if max_seen > 0.5 {
                break;
            }
        }
        handle.abort();
        assert!(
            max_seen > 0.5,
            "expected active spectrum bands > 0.5, got {max_seen}"
        );
        assert!(
            pcm.snapshot(256).is_some(),
            "expected shared PCM ring buffer to accumulate samples"
        );
    }
}
