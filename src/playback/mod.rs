//! Station playback session: song recognition, lyrics synchronization, and audio capture.
//!
//! Coordinates stream metadata, acoustic fingerprinting (Shazam), lyrics lookups,
//! and live audio visualization. Dropping a `Playback` session cancels background tasks.

mod capture;
mod playback;
mod schedule;
mod song;

pub use self::playback::{Playback, ShownLyrics};
