//! Process supervisor and JSON IPC controller for `mpv`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, io, thread};

use serde_json::{json, Value};

use super::ipc::{Connection, Endpoint};
use crate::log;

/// Buffer cushion in seconds before resuming playback after a network stall.
const STREAM_CUSHION_SECONDS: u32 = 1;

/// Maximum readahead buffer duration in seconds (absorbs network jitter on HLS/YouTube streams).
const READ_AHEAD_SECONDS: u32 = 30;

/// Events received from the `mpv` JSON IPC connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerEvent {
    IcyTitle(String),
    /// Indicates whether playback is stalled, paused, or buffering (from `mpv`'s `core-idle`).
    Idle(bool),
    Pause(bool),
    /// A stream URL failed to open or ended with an error (from `mpv`'s `end-file`).
    StreamError(String),
}

/// Manages a detached `mpv` subprocess communicating over a non-blocking JSON IPC connection.
pub struct MpvPlayer {
    child: Child,
    /// Active IPC connection; `None` if mpv terminated or closed the connection.
    stream: Option<Connection>,
    /// IPC endpoint handle; cleans up socket files on drop.
    _endpoint: Endpoint,
    read_buffer: Vec<u8>,
}

/// Candidate `mpv` executable paths, checking `PATH` before winget installation directories on Windows.
pub(crate) fn mpv_programs() -> Vec<PathBuf> {
    let mut programs = vec![PathBuf::from("mpv")];
    if cfg!(windows) {
        if let Some(directory) = env::var_os("ProgramFiles") {
            programs.push(PathBuf::from(directory).join("MPV Player").join("mpv.exe"));
        }
    }
    programs
}

/// Executes `launch` with the first available binary in `programs`.
fn spawn_first<T>(
    programs: &[PathBuf],
    mut launch: impl FnMut(&Path) -> io::Result<T>,
) -> io::Result<T> {
    let mut first_missing = None;
    for program in programs {
        match launch(program) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                first_missing.get_or_insert(error);
            }
            other => return other,
        }
    }
    Err(first_missing.unwrap_or_else(|| io::ErrorKind::NotFound.into()))
}

impl MpvPlayer {
    /// Starts a background `mpv` in `--idle=yes` mode and connects to its JSON IPC connection.
    pub fn spawn() -> io::Result<Self> {
        let endpoint = Endpoint::unique();

        let launch = |program: &Path| {
            let mut command = Command::new(program);
            command
                .args([
                    "--no-config",
                    "--player-operation-mode=cplayer",
                    "--force-window=no",
                    "--osc=no",
                    "--ytdl=yes",
                    "--no-video",
                    "--no-audio-display",
                    "--terminal=no",
                    "--input-default-bindings=no",
                    "--idle=yes",
                    "--keep-open=no",
                    "--cache=yes",
                    &format!("--cache-secs={READ_AHEAD_SECONDS}"),
                    "--cache-pause-initial=no",
                    &format!("--cache-pause-wait={STREAM_CUSHION_SECONDS}"),
                    "--demuxer-max-bytes=4194304",
                    &format!("--demuxer-readahead-secs={READ_AHEAD_SECONDS}"),
                    "--audio-buffer=0.4",
                    "--volume=85",
                    "--network-timeout=10",
                ])
                .arg(format!("--input-ipc-server={}", endpoint.path().display()))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                // Keep mpv detached from wildgrass's console so it cannot disturb raw mode
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                command.creation_flags(CREATE_NO_WINDOW);
            }
            command.spawn()
        };
        let mut child =
            spawn_first(&mpv_programs(), launch).map_err(|error| match error.kind() {
                io::ErrorKind::NotFound => io::Error::new(error.kind(), "mpv is not installed"),
                _ => error,
            })?;

        let stream = match Connection::connect(&mut child, &endpoint) {
            Ok(stream) => stream,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };

        let mut player = Self {
            child,
            stream: Some(stream),
            _endpoint: endpoint,
            read_buffer: Vec::with_capacity(4096),
        };
        // mpv v0.42+'s libcurl backend ignores `--network-timeout` when an HTTP server stalls
        // after accepting TCP, causing ~60s hangs on flaky HLS segments. Disabling it over IPC
        // falls back to FFmpeg's HTTP stack (which enforces `--network-timeout`) while staying
        // harmless on older mpv builds that do not have the `curl-enabled` property.
        player.send_command(json!(["set_property", "curl-enabled", false]));
        for (id, property) in [(1, "metadata"), (2, "core-idle"), (3, "pause")] {
            player.send_command(json!(["observe_property", id, property]));
        }
        Ok(player)
    }

    /// OS process id of the `mpv` daemon (used to find its audio output stream).
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    fn send_command(&mut self, command: Value) {
        let Some(stream) = &mut self.stream else {
            return;
        };
        let mut payload = json!({ "command": command }).to_string().into_bytes();
        payload.push(b'\n');
        if let Err(error) = stream.write_all(&payload) {
            log::error(format!("mpv: sending {command}: {error}"));
        }
    }

    /// Plays the first of `urls`. The rest are queued as mpv playlist entries, so mpv moves on
    /// to the next address by itself when one fails to open or its stream ends.
    pub fn load_stream(&mut self, urls: &[String]) {
        for (i, url) in urls.iter().enumerate() {
            let mode = if i == 0 { "replace" } else { "append-play" };
            self.send_command(json!(["loadfile", url, mode]));
        }
        self.send_command(json!(["set_property", "pause", false]));
    }

    pub fn stop(&mut self) {
        self.send_command(json!(["stop"]));
    }

    pub fn toggle_pause(&mut self) {
        self.send_command(json!(["cycle", "pause"]));
    }

    pub fn toggle_mute(&mut self) {
        self.send_command(json!(["cycle", "mute"]));
    }

    pub fn adjust_volume(&mut self, delta: i32) {
        self.send_command(json!(["add", "volume", delta]));
    }

    /// Drains any pending JSON IPC lines from `mpv` without blocking the UI thread.
    pub fn poll_events(&mut self) -> Vec<PlayerEvent> {
        let Some(stream) = &mut self.stream else {
            return Vec::new();
        };

        let mut chunk = [0u8; 4096];
        while self.read_buffer.len() < 65536 {
            match stream.read(&mut chunk) {
                Ok(0) => {
                    log::error("mpv: the control connection closed; mpv has quit");
                    self.stream = None;
                    break;
                }
                Ok(read) => self.read_buffer.extend_from_slice(&chunk[..read]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => {
                    log::error(format!("mpv: reading the control connection: {error}"));
                    self.stream = None;
                    break;
                }
            }
        }

        let mut events = Vec::new();
        while let Some(newline) = self.read_buffer.iter().position(|&byte| byte == b'\n') {
            let line_bytes: Vec<u8> = self.read_buffer.drain(..=newline).collect();
            if let Some(event) = parse_ipc_line(&String::from_utf8_lossy(&line_bytes)) {
                events.push(event);
            }
        }
        events
    }
}

fn parse_ipc_line(line: &str) -> Option<PlayerEvent> {
    let message: Value = serde_json::from_str(line.trim()).ok()?;
    match message.get("event")?.as_str()? {
        "property-change" => {
            let data = message.get("data")?;
            match message.get("name")?.as_str()? {
                "metadata" => {
                    let raw_icy = data.get("icy-title").and_then(Value::as_str)?.trim();
                    Some(PlayerEvent::IcyTitle(raw_icy.to_string()))
                }
                "core-idle" => Some(PlayerEvent::Idle(data.as_bool().unwrap_or(false))),
                "pause" => Some(PlayerEvent::Pause(data.as_bool().unwrap_or(false))),
                _ => None,
            }
        }
        "end-file" if message.get("reason").and_then(Value::as_str) == Some("error") => {
            let reason = message
                .get("file_error")
                .and_then(Value::as_str)
                .unwrap_or("loading failed");
            Some(PlayerEvent::StreamError(reason.to_string()))
        }
        _ => None,
    }
}

impl Drop for MpvPlayer {
    fn drop(&mut self) {
        self.send_command(json!(["quit"]));
        let deadline = Instant::now() + Duration::from_millis(400);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(15));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mpv_is_looked_for_on_the_path_first_then_where_its_installer_puts_it() {
        let programs = [PathBuf::from("mpv"), PathBuf::from("C:/mpv/mpv.exe")];
        let missing = || io::Error::new(io::ErrorKind::NotFound, "missing");
        let only = |found: &'static str| {
            move |program: &Path| match program.to_str() {
                Some(name) if name == found => Ok(name.to_string()),
                _ => Err(missing()),
            }
        };
        assert_eq!(spawn_first(&programs, only("mpv")).unwrap(), "mpv");
        assert_eq!(
            spawn_first(&programs, only("C:/mpv/mpv.exe")).unwrap(),
            "C:/mpv/mpv.exe"
        );
        let none = spawn_first(&programs, only("elsewhere")).unwrap_err();
        assert_eq!(none.kind(), io::ErrorKind::NotFound);

        // Anything but a missing program is not worked around
        let mut tried = Vec::new();
        let denied = spawn_first(&programs, |program: &Path| -> io::Result<()> {
            tried.push(program.to_path_buf());
            Err(io::ErrorKind::PermissionDenied.into())
        });
        assert_eq!(denied.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(tried, [PathBuf::from("mpv")]);

        assert_eq!(mpv_programs()[0], PathBuf::from("mpv"));
        assert_eq!(mpv_programs().len(), if cfg!(windows) { 2 } else { 1 });
    }

    /// Starts mpv and checks that both directions of the control connection (a socket, or on
    /// Windows a named pipe) work: the pause state mpv reports on starting comes in, a command
    /// changes it, and mpv quitting closes the connection. Nothing is played. It needs mpv, which CI machines do not have, so it
    /// only runs when asked for (`--ignored`).
    #[ignore]
    #[test]
    fn test_mpv_answers_over_its_control_connection() {
        fn wait_for(player: &mut MpvPlayer, wanted: PlayerEvent) -> bool {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if player.poll_events().contains(&wanted) {
                    return true;
                }
                thread::sleep(Duration::from_millis(20));
            }
            false
        }

        let mut player = MpvPlayer::spawn().expect("mpv starts");
        assert!(
            wait_for(&mut player, PlayerEvent::Pause(false)),
            "no answer"
        );
        player.toggle_pause();
        assert!(
            wait_for(&mut player, PlayerEvent::Pause(true)),
            "the command was not carried out"
        );

        player.send_command(json!(["quit"]));
        let deadline = Instant::now() + Duration::from_secs(3);
        while player.stream.is_some() && Instant::now() < deadline {
            player.poll_events();
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            player.stream.is_none(),
            "the closed connection went unnoticed"
        );
    }

    #[test]
    fn test_parse_ipc_metadata_and_property_events() {
        let metadata_line = r#"{"event":"property-change","id":1,"name":"metadata","data":{"icy-name":"SomaFM","icy-title":"Phoebe Bridgers - Kyoto"}}"#;
        assert_eq!(
            parse_ipc_line(metadata_line),
            Some(PlayerEvent::IcyTitle("Phoebe Bridgers - Kyoto".into()))
        );

        let idle_line = r#"{"event":"property-change","id":2,"name":"core-idle","data":true}"#;
        assert_eq!(parse_ipc_line(idle_line), Some(PlayerEvent::Idle(true)));

        let pause_line = r#"{"event":"property-change","id":3,"name":"pause","data":true}"#;
        assert_eq!(parse_ipc_line(pause_line), Some(PlayerEvent::Pause(true)));

        let empty_icy =
            r#"{"event":"property-change","id":1,"name":"metadata","data":{"icy-title":""}}"#;
        assert_eq!(
            parse_ipc_line(empty_icy),
            Some(PlayerEvent::IcyTitle(String::new()))
        );

        let stream_error = r#"{"event":"end-file","reason":"error","file_error":"loading failed"}"#;
        assert_eq!(
            parse_ipc_line(stream_error),
            Some(PlayerEvent::StreamError("loading failed".into()))
        );

        let normal_eof = r#"{"event":"end-file","reason":"eof"}"#;
        assert_eq!(parse_ipc_line(normal_eof), None);

        let ignored_line = r#"{"event":"property-change","id":4,"name":"volume","data":90.0}"#;
        assert_eq!(parse_ipc_line(ignored_line), None);
    }
}
