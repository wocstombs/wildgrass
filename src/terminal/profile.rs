//! Windows Terminal profile relaunching and terminal background control (Windows only).
//!
//! Relaunches wildgrass in its dedicated Windows Terminal profile (`wildgrass.json`) when
//! started in an arbitrary terminal tab, and manages background color escape sequences.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, io, thread};

use windows_sys::Win32::System::Console::{
    GetStdHandle, WriteConsoleInputW, INPUT_RECORD, INPUT_RECORD_0, KEY_EVENT, KEY_EVENT_RECORD,
    KEY_EVENT_RECORD_0, STD_INPUT_HANDLE,
};

use super::size::{OWN_COLUMNS, OWN_ROWS};
use super::{windows_terminal, REOPENED};

/// Sets terminal background to solid black via OSC 11.
pub fn set_background() {
    write_terminal("\x1b]11;rgb:00/00/00\x1b\\");
}

/// Restores default terminal background color via OSC 111.
pub fn reset_background() {
    write_terminal("\x1b]111\x1b\\");
}

/// Sends escape sequence to stdout, ignoring write errors.
fn write_terminal(sequence: &str) {
    use std::io::Write;

    let mut stdout = io::stdout();
    let _ = stdout.write_all(sequence.as_bytes());
    let _ = stdout.flush();
}

/// Relaunches wildgrass in a dedicated Windows Terminal window using the custom profile.
/// Returns true if the new window was launched, signaling the current process to exit.
pub fn reopen_in_profile(args: &[String]) -> bool {
    let in_profile = env::var("WT_PROFILE_ID").is_ok_and(|id| id.eq_ignore_ascii_case(PROFILE_ID));
    if in_profile || !profile_installed() || !windows_terminal::console_owned_by_windows_terminal()
    {
        return false;
    }
    let Some(program) = env::current_exe()
        .ok()
        .and_then(|program| program.to_str().map(str::to_owned))
    else {
        return false;
    };
    let opened = Command::new("wt.exe")
        .args(wt_args(&program, args))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok_and(window_opened);
    if opened {
        close_tab();
    }
    opened
}

/// Whether `wt.exe` reports that it opened the window, waiting for it up to `WT_WAIT`. Waited for
/// so that the tab only closes once wildgrass's window is on its way.
fn window_opened(mut wt_process: Child) -> bool {
    let started = Instant::now();
    loop {
        match wt_process.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < WT_WAIT => {
                thread::sleep(Duration::from_millis(20));
            }
            _ => return false,
        }
    }
}

/// How long `wt.exe` is waited for at most.
const WT_WAIT: Duration = Duration::from_secs(5);

/// What is typed into the tab: `exit 0` and Enter.
const EXIT_LINE: &str = "exit 0\r";

/// One key that types a character into the console.
struct Key {
    /// Virtual key code: ASCII uppercase for letters/digits, 0x0d for Enter, 0x20 for Space.
    virtual_code: u16,
    character: u16,
}

/// The keys that type `EXIT_LINE`.
fn exit_keys() -> Vec<Key> {
    EXIT_LINE
        .chars()
        .map(|c| {
            let virtual_code = if c == '\r' {
                0x0d
            } else {
                c.to_ascii_uppercase() as u16
            };
            Key {
                virtual_code,
                character: c as u16,
            }
        })
        .collect()
}

/// Types `exit 0` and Enter into this console's input, for the shell that started wildgrass to
/// read once wildgrass has ended.
#[expect(
    unsafe_code,
    reason = "only WriteConsoleInputW types into the console, and windows-sys offers it only as \
              an unsafe call"
)]
fn close_tab() {
    let records: Vec<INPUT_RECORD> = exit_keys()
        .into_iter()
        .flat_map(|key| {
            [true, false].map(|down| INPUT_RECORD {
                EventType: KEY_EVENT as u16,
                Event: INPUT_RECORD_0 {
                    KeyEvent: KEY_EVENT_RECORD {
                        bKeyDown: down.into(),
                        wRepeatCount: 1,
                        wVirtualKeyCode: key.virtual_code,
                        wVirtualScanCode: 0,
                        uChar: KEY_EVENT_RECORD_0 {
                            UnicodeChar: key.character,
                        },
                        dwControlKeyState: 0,
                    },
                },
            })
        })
        .collect();
    let mut written = 0;
    // SAFETY: the records outlive the call and their number goes with them; a standard input
    // that is not a console's only makes the call fail
    unsafe {
        WriteConsoleInputW(
            GetStdHandle(STD_INPUT_HANDLE),
            records.as_ptr(),
            records.len() as u32,
            &mut written,
        );
    }
}

/// Deterministic UUIDv5 GUID that Windows Terminal assigns to the `wildgrass` fragment profile.
const PROFILE_ID: &str = "{50c7a309-2541-506a-a323-d655183650b3}";

/// Whether the profile fragment is where Windows Terminal reads it. Without it, `wt.exe -p
/// wildgrass` would open the default profile, which looks no different.
fn profile_installed() -> bool {
    env::var_os("LOCALAPPDATA").is_some_and(|directory| {
        Path::new(&directory)
            .join(r"Microsoft\Windows Terminal\Fragments\wildgrass\wildgrass.json")
            .is_file()
    })
}

/// What `wt.exe` is given: a new window at wildgrass's own size with the wildgrass profile,
/// running this `program` with `args`. `wt.exe` splits its command line into several commands at
/// each `;`, so those are escaped in the command.
fn wt_args(program: &str, args: &[String]) -> Vec<String> {
    let size = format!("{OWN_COLUMNS},{OWN_ROWS}");
    let mut arguments = [
        "-w",
        "new",
        "--size",
        &size,
        "new-tab",
        "-p",
        "wildgrass",
        "--",
    ]
    .map(str::to_owned)
    .to_vec();
    let command = [program, REOPENED]
        .map(str::to_owned)
        .into_iter()
        .chain(args.iter().cloned());
    arguments.extend(command.map(|argument| argument.replace(';', r"\;")));
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wt_args_run_this_exe_in_the_profile_and_escape_semicolons() {
        let args = ["play".to_string(), "jazz;fm".to_string()];
        assert_eq!(
            wt_args(r"C:\wild;grass\wildgrass.exe", &args),
            [
                "-w",
                "new",
                "--size",
                "47,6",
                "new-tab",
                "-p",
                "wildgrass",
                "--",
                r"C:\wild\;grass\wildgrass.exe",
                REOPENED,
                "play",
                r"jazz\;fm",
            ]
        );
    }

    #[test]
    fn test_exit_is_typed_with_each_key_code() {
        let keys: Vec<u16> = exit_keys()
            .into_iter()
            .map(|key| key.virtual_code)
            .collect();
        assert_eq!(keys, [0x45, 0x58, 0x49, 0x54, 0x20, 0x30, 0x0d]);
    }
}
