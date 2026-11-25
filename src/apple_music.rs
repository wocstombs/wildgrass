//! Apple Music catalog integration for retrieving track durations.
//!
//! Queries Apple Music to determine recording duration for release matching, ensuring
//! synchronized lyrics correspond to the specific track release (single, album, or remaster).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use std::{fs, io};

use reqwest::Client;
use serde::Deserialize;

use crate::{files, http, log};

#[derive(Deserialize, Default)]
#[serde(default)]
struct LookupAnswer {
    results: Vec<Track>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Track {
    track_time_millis: Option<f64>,
}

/// Retrieves the duration in seconds for Apple Music track `id`.
/// Returns `None` if the track is not found or the request fails.
pub async fn track_length(client: &Client, id: &str) -> Option<f64> {
    // Queries the Apple Music China storefront matching Shazam Discovery API localization
    let url = format!(
        "https://itunes.apple.com/lookup?id={}&country=cn",
        urlencoding::encode(id)
    );
    // Short timeout to avoid blocking lyrics fetch if iTunes lookup is slow
    let request = client.get(&url).timeout(Duration::from_secs(2));
    let answer: LookupAnswer = http::get_json(request, "apple music").await?;
    length(answer)
}

fn length(answer: LookupAnswer) -> Option<f64> {
    let milliseconds = answer.results.first()?.track_time_millis?;
    (milliseconds > 0.0).then(|| milliseconds / 1000.0)
}

/// Persistent cache of Apple Music track durations.
#[derive(Debug, Default)]
pub struct TrackLengths {
    /// Storage path for track lengths cache; `None` retains in memory only.
    path: Option<PathBuf>,
    known: HashMap<String, f64>,
}

impl TrackLengths {
    /// The lengths saved at `path`; empty when the file is missing or unreadable.
    pub fn load(path: PathBuf) -> Self {
        let known = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            known,
        }
    }

    pub fn get(&self, id: &str) -> Option<f64> {
        self.known.get(id).copied()
    }

    /// Saves `seconds` as the length of Apple Music recording `id`, writing to disk when new or
    /// changed.
    pub fn remember(&mut self, id: String, seconds: f64) {
        if seconds <= 0.0 {
            return;
        }
        if self
            .get(&id)
            .is_some_and(|known| (known - seconds).abs() < 0.05)
        {
            return;
        }
        self.known.insert(id, seconds);
        if let Err(error) = self.save() {
            log::error(format!("apple music lengths: {error}"));
        }
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let json = serde_json::to_vec(&self.known)?;
        files::write_atomically(path, &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ScratchDir;

    #[test]
    fn test_lookup_answer_gives_the_track_length() {
        // Apple's answer for the single version of Gloria Estefan's "1-2-3", shortened
        let answer = r#"{"resultCount": 1, "results": [{"wrapperType": "track", "kind": "song",
            "trackId": 198676212, "trackName": "1-2-3 (Single Version)", "trackTimeMillis": 215907}]}"#;
        let length = length(serde_json::from_str(answer).unwrap());
        assert_eq!(length, Some(215.907));

        let none = r#"{"resultCount": 0, "results": []}"#;
        assert_eq!(super::length(serde_json::from_str(none).unwrap()), None);
    }

    #[test]
    fn test_track_lengths_persist_and_reload() {
        let scratch = ScratchDir::create();
        let path = scratch.path().join("track_lengths.json");

        let mut lengths = TrackLengths::load(path.clone());
        assert_eq!(lengths.get("198676212"), None);
        lengths.remember("198676212".into(), 215.907);
        assert_eq!(lengths.get("198676212"), Some(215.907));

        let reloaded = TrackLengths::load(path);
        assert_eq!(reloaded.get("198676212"), Some(215.907));
    }
}
