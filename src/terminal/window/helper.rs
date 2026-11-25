//! The helper process that reaches the window where wildgrass cannot itself: `powershell.exe`
//! running `windows_helper.ps1` on Windows (natively or from WSL). It reads one letter per line
//! on its standard input and keeps running, so key presses do not pay its startup cost each time
//! (PowerShell's `Add-Type` takes seconds).

use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use std::{env, fs, thread};

use base64::engine::general_purpose::STANDARD;

use super::action::{Direction, Layer};

/// How long `Helper::finish` waits for the helper to carry out what is queued.
const HELPER_FINISH_WAIT: Duration = Duration::from_secs(2);

/// What the helper script is asked to do to the terminal window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WindowCommand {
    Move(Direction),
    Layer(Layer),
    /// Give the window wildgrass's own size (`windows_helper.ps1`), when Windows Terminal will not.
    Resize,
    /// Give the window back the size and place it had before `Resize`, as wildgrass exits.
    UndoResize,
}

impl WindowCommand {
    /// The line `windows_helper.ps1` reads: one letter for each command.
    fn wire_command(self) -> &'static [u8] {
        match self {
            Self::Move(Direction::Left) => b"L\n",
            Self::Move(Direction::Right) => b"R\n",
            Self::Move(Direction::Up) => b"U\n",
            Self::Move(Direction::Down) => b"D\n",
            Self::Layer(Layer::Top) => b"T\n",
            Self::Layer(Layer::Bottom) => b"B\n",
            Self::Layer(Layer::Normal) => b"N\n",
            Self::Resize => b"S\n",
            Self::UndoResize => b"Q\n",
        }
    }
}

/// The running helper process, fed from a thread of its own.
pub(super) struct Helper {
    sender: Option<mpsc::SyncSender<WindowCommand>>,
    /// Never sent on: it disconnects when the helper's thread ends, which is how `finish`
    /// knows that everything queued has been carried out.
    ended: mpsc::Receiver<()>,
}

impl Helper {
    /// Starts the helper on a thread of its own, so that starting it does not hold up the
    /// terminal.
    pub(super) fn start() -> Self {
        let (sender, receiver) = mpsc::sync_channel(32);
        let (ended_sender, ended) = mpsc::channel();
        thread::spawn(move || {
            feed_helper(receiver);
            drop(ended_sender);
        });
        Self {
            sender: Some(sender),
            ended,
        }
    }

    pub(super) fn send(&self, command: WindowCommand) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(command);
        }
    }

    /// Ends the helper once it has carried out what was sent, waiting for that for a moment: a
    /// command still queued when wildgrass exits would be lost with the thread.
    pub(super) fn finish(&mut self) {
        self.sender.take();
        // Only ever answers with a disconnect, or with the timeout
        let _ = self.ended.recv_timeout(HELPER_FINISH_WAIT);
    }
}

/// Feeds `receiver` to the helper until the sender is dropped and everything queued is
/// written, then lets the helper end by closing its input.
fn feed_helper(receiver: mpsc::Receiver<WindowCommand>) {
    use std::io::Write;

    let Some(mut child) = launch_powershell() else {
        return;
    };
    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return;
    };
    while let Ok(command) = receiver.recv() {
        if stdin.write_all(command.wire_command()).is_err() || stdin.flush().is_err() {
            break;
        }
    }
    drop(stdin);
    let _ = child.wait();
}

/// Starts `program` with `args`, reading from a pipe and with its output thrown away.
fn spawn_helper(program: &str, args: &[&str]) -> Option<Child> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Isolate the helper from wildgrass's console so PowerShell cannot alter its console mode
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn().ok()
}

const WINDOWS_HELPER_SCRIPT: &str = include_str!("windows_helper.ps1");

pub(super) fn is_wsl() -> bool {
    env::var_os("WSL_INTEROP").is_some()
        || env::var_os("WSL_DISTRO_NAME").is_some()
        || env::var_os("WT_SESSION").is_some()
        || fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|release| release.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

fn launch_powershell() -> Option<Child> {
    let encoded = encode_powershell(WINDOWS_HELPER_SCRIPT);
    let args = [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-EncodedCommand",
        &encoded,
    ];
    [
        "powershell.exe",
        "/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe",
    ]
    .into_iter()
    .find_map(|program| spawn_helper(program, &args))
}

/// Encodes a PowerShell script as UTF-16LE Base64 for `-EncodedCommand`.
fn encode_powershell(script: &str) -> String {
    use base64::Engine;

    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    STANDARD.encode(utf16)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// Every command wildgrass can send the helper.
    const COMMANDS: [WindowCommand; 9] = [
        WindowCommand::Move(Direction::Left),
        WindowCommand::Move(Direction::Right),
        WindowCommand::Move(Direction::Up),
        WindowCommand::Move(Direction::Down),
        WindowCommand::Layer(Layer::Top),
        WindowCommand::Layer(Layer::Bottom),
        WindowCommand::Layer(Layer::Normal),
        WindowCommand::Resize,
        WindowCommand::UndoResize,
    ];

    #[test]
    fn test_the_script_answers_each_letter_it_is_sent() {
        let mut letters = HashSet::new();
        for command in COMMANDS {
            let &[letter, b'\n'] = command.wire_command() else {
                panic!("{command:?} is not one letter on its own line");
            };
            assert!(letters.insert(letter), "{command:?} reuses a letter");
            let quoted = format!("\"{}\"", letter as char);
            assert!(
                WINDOWS_HELPER_SCRIPT.contains(&quoted),
                "windows_helper.ps1: {quoted}"
            );
        }
    }

    #[test]
    fn test_encode_powershell_uses_utf16le_base64() {
        // "hi" in UTF-16LE is [0x68, 0x00, 0x69, 0x00] -> "aABpAA=="
        assert_eq!(encode_powershell("hi"), "aABpAA==");
        assert!(WINDOWS_HELPER_SCRIPT.contains("\n'@\n"));
        // Earlier layered window is put back before layering a different one
        assert!(WINDOWS_HELPER_SCRIPT.contains("layered != hwnd"));
    }
}
