//! Logging utilities for runtime errors and song synchronization diagnostics.
//!
//! Because the TUI captures the terminal, diagnostics are written to disk:
//! - `wildgrass.log`: General errors and warnings, cleared on each launch.
//! - `sync.log`: Song recognition, alignments, and lyric timing measurements, preserved across runs.

use std::fmt::Display;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

struct Log {
    file: Mutex<File>,
    started: Instant,
}

static LOG: OnceLock<Log> = OnceLock::new();

/// Initializes the error log file at `path`, overwriting any log from a previous session.
pub fn init(path: &Path) -> io::Result<()> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    let log = Log {
        file: Mutex::new(File::create(path)?),
        started: Instant::now(),
    };
    let _ = LOG.set(log);
    Ok(())
}

/// Appends `message` to the error log with an elapsed timestamp prefix.
#[expect(
    clippy::disallowed_methods,
    reason = "a thread that panicked while writing to the log must not stop the others from logging"
)]
pub fn error(message: impl Display) {
    use std::io::Write;

    let Some(log) = LOG.get() else {
        return;
    };
    let elapsed = log.started.elapsed().as_secs_f64();
    let mut file = log.file.lock().unwrap_or_else(PoisonError::into_inner);
    // Ignore write failures since stderr is unavailable inside the TUI
    let _ = writeln!(file, "[{elapsed:9.3}s] {message}");
}

/// Maximum size for `sync.log` before rotating (2 MB).
const SYNC_LOG_LIMIT: u64 = 2 * 1024 * 1024;

static SYNC: OnceLock<Mutex<File>> = OnceLock::new();

/// Opens `sync.log` at `path` for tracking recognition events, position sync, and lyrics matching.
/// Unlike the error log, `sync.log` persists across sessions until reaching `SYNC_LOG_LIMIT`.
pub fn init_sync(path: &Path) -> io::Result<()> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    let outgrown = fs::metadata(path).is_ok_and(|metadata| metadata.len() > SYNC_LOG_LIMIT);
    let file = OpenOptions::new()
        .create(true)
        .append(!outgrown)
        .write(true)
        .truncate(outgrown)
        .open(path)?;
    let _ = SYNC.set(Mutex::new(file));
    Ok(())
}

/// Appends `message` to the sync log, stamped with the date and time (UTC).
#[expect(
    clippy::disallowed_methods,
    reason = "a thread that panicked while writing to the log must not stop the others from logging"
)]
pub fn sync(message: impl Display) {
    use std::io::Write;

    let Some(file) = SYNC.get() else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut file = file.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = writeln!(file, "[{}] {message}", utc_stamp(now));
}

/// `seconds` after 1970 as `YYYY-MM-DD HH:MM:SS`.
fn utc_stamp(seconds: u64) -> String {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Days to a calendar date (Howard Hinnant's `civil_from_days`)
    let shifted = days + 719_468;
    let (era, day_of_era) = (shifted / 146_097, shifted % 146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_utc_stamp_gives_the_calendar_date() {
        assert_eq!(utc_stamp(0), "1970-01-01 00:00:00");
        assert_eq!(utc_stamp(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(utc_stamp(1_794_600_265), "2026-11-13 20:04:25");
    }
}
