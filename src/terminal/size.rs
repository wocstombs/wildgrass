//! Automatic window resizing for Windows Terminal (native and WSL).
//!
//! Uses xterm resize escape sequences (`CSI 8 ; rows ; columns t`) to set wildgrass's
//! compact window dimensions (47 columns × 6 rows) on startup and restore previous dimensions on exit.
//! Automatically compensates for Windows Terminal frame-offset quirks.

use std::time::{Duration, Instant};
use std::{io, thread};

use crossterm::terminal;

use super::windows_terminal;

pub(super) const OWN_COLUMNS: u16 = 47;
pub(super) const OWN_ROWS: u16 = 6;
const OWN_SIZE: Size = Size {
    columns: OWN_COLUMNS,
    rows: OWN_ROWS,
};

const SETTLED_AFTER: Duration = Duration::from_millis(60);
const ANSWER_WAIT: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Size {
    columns: u16,
    rows: u16,
}

/// Difference between requested and actual dimensions reported by the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Offset {
    columns: i32,
    rows: i32,
}

pub(super) enum Shrink {
    /// Not running in Windows Terminal, or already at `OWN_SIZE`.
    Unneeded,
    Done(Resized),
    /// Resize escape was ignored (for example, a multi-tab Windows Terminal window).
    Refused,
}

pub(super) struct Resized {
    before: Size,
    off_by: Offset,
}

impl Size {
    fn current() -> Option<Self> {
        let (columns, rows) = terminal::size().ok()?;
        Some(Self { columns, rows })
    }

    /// Computes target dimensions compensated by `off_by`.
    fn request_for(self, off_by: Offset) -> Self {
        let less =
            |value: u16, by: i32| (i32::from(value) - by).clamp(1, i32::from(u16::MAX)) as u16;
        Self {
            columns: less(self.columns, off_by.columns),
            rows: less(self.rows, off_by.rows),
        }
    }
}

impl Offset {
    fn between(asked: Size, got: Size) -> Self {
        Self {
            columns: i32::from(got.columns) - i32::from(asked.columns),
            rows: i32::from(got.rows) - i32::from(asked.rows),
        }
    }
}

/// Shrinks Windows Terminal to wildgrass's compact dimensions.
pub(super) fn shrink() -> Shrink {
    if !windows_terminal::in_windows_terminal() {
        return Shrink::Unneeded;
    }
    let Some(before) = Size::current().filter(|&size| size != OWN_SIZE) else {
        return Shrink::Unneeded;
    };
    let Some(first) = ask(OWN_SIZE, before) else {
        return Shrink::Refused;
    };
    let mut off_by = Offset::between(OWN_SIZE, first);
    if first != OWN_SIZE {
        let asked = OWN_SIZE.request_for(off_by);
        if let Some(second) = ask(asked, first) {
            off_by = Offset::between(asked, second);
        }
    }
    Shrink::Done(Resized { before, off_by })
}

/// Restores terminal dimensions to their state prior to running.
pub(super) fn restore(resized: &Resized) {
    request(resized.before.request_for(resized.off_by));
}

/// Asks the terminal for `size` and returns the size it settles on, starting `from` the size it
/// has now; none when it does not change at all.
fn ask(size: Size, from: Size) -> Option<Size> {
    request(size);
    let start = Instant::now();
    let mut current = from;
    let mut changed_at = None;
    loop {
        thread::sleep(Duration::from_millis(2));
        let now = Instant::now();
        if let Some(size) = Size::current().filter(|&size| size != current) {
            current = size;
            changed_at = Some(now);
        }
        match changed_at {
            Some(at) if now - at >= SETTLED_AFTER => return Some(current),
            _ if now - start >= ANSWER_WAIT => return changed_at.map(|_| current),
            _ => {}
        }
    }
}

fn request(size: Size) {
    use std::io::Write;

    let mut stdout = io::stdout();
    let _ = write!(stdout, "\x1b[8;{};{}t", size.rows, size.columns);
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_request_is_lowered_by_what_the_answer_was_off_by() {
        let asked = OWN_SIZE;
        let got = Size {
            columns: 47,
            rows: 8,
        };
        let off_by = Offset::between(asked, got);
        assert_eq!(
            off_by,
            Offset {
                columns: 0,
                rows: 2
            }
        );
        assert_eq!(
            OWN_SIZE.request_for(off_by),
            Size {
                columns: 47,
                rows: 4
            }
        );
        // The size from before comes back the same way
        let before = Size {
            columns: 120,
            rows: 30,
        };
        assert_eq!(
            before.request_for(off_by),
            Size {
                columns: 120,
                rows: 28
            }
        );
    }

    #[test]
    fn test_a_request_is_raised_when_the_answer_was_short_and_never_below_one() {
        let off_by = Offset {
            columns: 0,
            rows: -1,
        };
        assert_eq!(OWN_SIZE.request_for(off_by).rows, 7);
        let tiny = Size {
            columns: 1,
            rows: 1,
        };
        let way_off = Offset {
            columns: 5,
            rows: 5,
        };
        assert_eq!(tiny.request_for(way_off), tiny);
    }
}
