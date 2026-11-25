//! Linux audio stream monitoring via PulseAudio/PipeWire (`parec --monitor-stream`).

use std::process::Stdio;
use std::time::Duration;
use std::{future, io};

use tokio::process::Command;

use super::capture::Capture;

/// Name of the capture tool used for logging.
pub(super) const NAME: &str = "parec";

/// Captures mpv's audio output stream by monitoring its PulseAudio/PipeWire sink input.
/// Returns `Ok(None)` if mpv has not produced audio within 20 seconds.
pub(super) async fn start(pid: u32) -> io::Result<Option<Capture>> {
    // mpv creates its sink input once audio decoding starts; poll for up to 20s
    for _ in 0..80 {
        let Some(index) = find_mpv_sink_input(pid).await? else {
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        };
        // 25ms requested latency stays at/above PipeWire's default quantum so
        // capturing never forces the whole audio graph into smaller periods.
        return Command::new("parec")
            .args([
                "--raw",
                "--format=s16le",
                "--rate=16000",
                "--channels=2",
                "--latency-msec=25",
                &format!("--monitor-stream={index}"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map(|child| Some(Capture::from_program(child)));
    }
    Ok(None)
}

/// Waits until mpv produces an active audio sink input stream.
pub(super) async fn output_appears(pid: u32) {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        match find_mpv_sink_input(pid).await {
            Ok(Some(_)) => return,
            Ok(None) => {}
            Err(_) => future::pending().await,
        }
    }
}

/// Finds the PulseAudio/PipeWire sink-input index owned by `pid` in `pactl list sink-inputs` output.
fn parse_sink_input_for_pid(pactl_output: &str, pid: u32) -> Option<u32> {
    let wanted = format!("application.process.id = \"{pid}\"");
    let mut current = None;
    for line in pactl_output.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Sink Input #") {
            current = rest.trim().parse().ok();
        } else if line == wanted {
            return current;
        }
    }
    None
}

/// The PulseAudio/PipeWire sink input of process `pid`: `Ok(None)` while it has none yet, and
/// an error when sink inputs cannot be listed at all.
async fn find_mpv_sink_input(pid: u32) -> io::Result<Option<u32>> {
    let output = Command::new("pactl")
        .args(["list", "sink-inputs"])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "pactl list sink-inputs failed ({})",
            output.status
        )));
    }
    Ok(parse_sink_input_for_pid(
        &String::from_utf8_lossy(&output.stdout),
        pid,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sink_input_for_pid() {
        let pactl = "\
Sink Input #41
\tDriver: protocol-native.c
\tProperties:
\t\tapplication.name = \"Firefox\"
\t\tapplication.process.id = \"999\"
Sink Input #57
\tProperties:
\t\tapplication.name = \"mpv\"
\t\tapplication.process.id = \"1234\"
";
        assert_eq!(parse_sink_input_for_pid(pactl, 1234), Some(57));
        assert_eq!(parse_sink_input_for_pid(pactl, 999), Some(41));
        assert_eq!(parse_sink_input_for_pid(pactl, 5), None);
    }
}
