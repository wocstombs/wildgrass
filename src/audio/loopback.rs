//! Windows audio capture via WASAPI process loopback (Windows 10 build 20348+).
//!
//! Directly captures audio rendered by mpv's process graph without system microphone mixing,
//! resampled by Windows into 16 kHz stereo s16le PCM.

use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use std::{future, io, thread};

use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};
use wasapi::{AudioClient, Direction, SampleType, StreamMode, WasapiError, WaveFormat};

use super::capture::Capture;
use super::SAMPLE_RATE_HZ;

/// Name of the capture mechanism used for logging.
pub(super) const NAME: &str = "process loopback";

/// Capacity of the packet queue (~320 ms at standard 10 ms WASAPI quantum).
/// Packets arriving when full (e.g. while playback is idle) are dropped.
const PACKETS_IN_FLIGHT: usize = 32;

/// Timeout waiting for the next audio buffer before checking if the consumer disconnected.
const WAIT_MILLISECONDS: u32 = 500;

/// Starts capturing audio output from `mpv` (`pid`) via WASAPI process loopback.
pub(super) async fn start(pid: u32) -> io::Result<Option<Capture>> {
    Ok(Some(Capture {
        reader: Box::new(Reader::start(pid).await?),
        program: None,
    }))
}

/// No-op stub matching the monitor interface; WASAPI process loopback starts immediately.
pub(super) async fn output_appears(_pid: u32) {
    future::pending().await
}

/// Async stream adapter for captured WASAPI PCM packets.
struct Reader {
    packets: mpsc::Receiver<Vec<u8>>,
    /// Unconsumed residual bytes from the previous packet.
    pending: Vec<u8>,
}

impl Reader {
    /// Starts capturing what process `pid` and the processes it starts play. An error when
    /// this Windows cannot capture one process's audio.
    async fn start(pid: u32) -> io::Result<Self> {
        let (started_sender, started) = oneshot::channel();
        let (packets_sender, packets) = mpsc::channel(PACKETS_IN_FLIGHT);
        thread::Builder::new()
            .name("mpv capture".into())
            .spawn(move || run(pid, &packets_sender, started_sender))?;
        started
            .await
            .map_err(|_| io::Error::other("the capture thread ended"))??;
        Ok(Self {
            packets,
            pending: Vec::new(),
        })
    }
}

/// Sets the capture up and says on `started` whether that worked, then sends what `pid` plays
/// until the reader is dropped or the capture fails.
fn run(pid: u32, packets: &mpsc::Sender<Vec<u8>>, started: oneshot::Sender<io::Result<()>>) {
    let setup = || -> Result<_, WasapiError> {
        // Every thread that uses WASAPI joins COM first; this thread is wildgrass's only one
        let _ = wasapi::initialize_mta();
        let format = WaveFormat::new(16, 16, SampleType::Int, SAMPLE_RATE_HZ as usize, 2, None);
        let mut client = AudioClient::new_application_loopback_client(pid, true)?;
        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 0,
        };
        client.initialize_client(&format, Direction::Capture, mode, None)?;
        let event = client.set_get_eventhandle()?;
        let capture = client.get_audiocaptureclient()?;
        client.start_stream()?;
        Ok((client, event, capture))
    };
    let (client, event, capture) = match setup() {
        Ok(parts) => parts,
        Err(error) => {
            let _ = started.send(Err(io::Error::other(error.to_string())));
            return;
        }
    };
    let _ = started.send(Ok(()));

    let mut queue = VecDeque::new();
    'capture: loop {
        if event.wait_for_event(WAIT_MILLISECONDS).is_err() {
            // Nothing is playing (paused or stopped)
            if packets.is_closed() {
                break;
            }
            continue;
        }
        loop {
            match capture.get_next_packet_size() {
                Ok(Some(frames)) if frames > 0 => {}
                Ok(_) => break,
                Err(_) => break 'capture,
            }
            if capture.read_from_device_to_deque(&mut queue).is_err() {
                break 'capture;
            }
        }
        if queue.is_empty() {
            continue;
        }
        match packets.try_send(queue.drain(..).collect()) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Closed(_)) => break,
        }
    }
    let _ = client.stop_stream();
}

impl AsyncRead for Reader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.pending.is_empty() {
            match ready!(self.packets.poll_recv(context)) {
                Some(packet) => self.pending = packet,
                // The capture has ended: this is the end of the stream
                None => return Poll::Ready(Ok(())),
            }
        }
        let count = buffer.remaining().min(self.pending.len());
        buffer.put_slice(&self.pending[..count]);
        self.pending.drain(..count);
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use tokio::io::AsyncReadExt;

    use super::*;

    /// Plays a quiet tone with mpv for two seconds and checks that the capture hears it in both
    /// channels. It needs mpv and a sound device, which CI machines do not have, so it only runs
    /// when asked for (`--ignored`). mpv's volume is cubic, so 40 plays the tone at 0.4³ of its
    /// level: quiet, but far above the silence (all zeros) a capture that hears nothing gives.
    #[ignore]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_captures_what_mpv_plays() {
        let mut mpv = crate::player::mpv_programs()
            .iter()
            .find_map(|program| {
                Command::new(program)
                    .args([
                        "--no-config",
                        "--no-video",
                        "--volume=40",
                        "av://lavfi:sine=frequency=440:duration=2",
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .ok()
            })
            .expect("mpv starts");
        let mut reader = Reader::start(mpv.id()).await.expect("capture starts");
        // Stereo samples: left, then right
        let mut samples: Vec<[i16; 2]> = Vec::new();
        let mut pending = Vec::new();
        let mut chunk = [0u8; 4096];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        while samples.len() < SAMPLE_RATE_HZ as usize && tokio::time::Instant::now() < deadline {
            let read = tokio::time::timeout(Duration::from_millis(500), reader.read(&mut chunk));
            if let Ok(Ok(count)) = read.await {
                pending.extend_from_slice(&chunk[..count]);
                let whole = pending.len() / 4 * 4;
                samples.extend(pending[..whole].as_chunks::<4>().0.iter().map(
                    |&[left_low, left_high, right_low, right_high]| {
                        [
                            i16::from_le_bytes([left_low, left_high]),
                            i16::from_le_bytes([right_low, right_high]),
                        ]
                    },
                ));
                pending.drain(..whole);
            }
        }
        let _ = mpv.kill();
        let _ = mpv.wait();
        let loudest = |channel: usize| {
            samples
                .iter()
                .map(|sample| sample[channel].unsigned_abs())
                .max()
                .unwrap_or(0)
        };
        let (left, right) = (loudest(0), loudest(1));
        println!(
            "captured {} stereo samples, loudest {left} left and {right} right",
            samples.len()
        );
        assert!(
            samples.len() >= SAMPLE_RATE_HZ as usize / 2,
            "too little audio"
        );
        // mpv plays the mono tone in both channels
        assert!(left > 100 && right > 100, "the capture is silent");
    }
}
