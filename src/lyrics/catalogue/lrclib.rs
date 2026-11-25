//! LRCLIB lyrics client (.lrc format with embedded synchronized text).

use reqwest::Client;
use serde::Deserialize;

use super::ranking::{self, Candidate};
use super::request;
use crate::lyrics::{parse, LyricsMatch, Release};
use crate::track::TrackName;

/// How many of the search hits are ranked.
const MAX_HITS: usize = 8;

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Hit {
    track_name: Option<String>,
    name: Option<String>,
    artist_name: Option<String>,
    synced_lyrics: Option<String>,
    instrumental: Option<bool>,
    /// The recording's length in seconds.
    duration: Option<f64>,
}

/// The entries found, with their synced lyrics (`None` for an instrumental). Entries with
/// neither are left out.
fn hits(answer: Vec<Hit>) -> impl Iterator<Item = Candidate<Option<String>>> {
    answer.into_iter().take(MAX_HITS).filter_map(|hit| {
        let synced = hit
            .synced_lyrics
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty());
        if synced.is_none() && hit.instrumental != Some(true) {
            return None;
        }
        let title = hit.track_name.or(hit.name).unwrap_or_default();
        let name = TrackName::new(&title, &hit.artist_name.unwrap_or_default());
        let length = hit.duration.filter(|&length| length > 0.0);
        Some(Candidate {
            name,
            length,
            entry: synced,
        })
    })
}

pub(super) async fn search(
    client: Client,
    song: TrackName,
    release: Release,
) -> Option<LyricsMatch> {
    let url = format!(
        "https://lrclib.net/api/search?q={}",
        urlencoding::encode(&request::search_query(&song))
    );
    let answer: Vec<Hit> = request::get_json(client.get(&url)).await?;

    // The lyrics came with the search results, so there is nothing more to download
    ranking::first_match(&song, release, hits(answer), |name, synced| async move {
        match synced {
            Some(text) => LyricsMatch::new_downloaded(name, &text, parse::parse_lrc(&text)),
            None => Some(LyricsMatch::new_instrumental(name)),
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_answer_keeps_synced_entries_and_instrumentals() {
        let answer = r#"[
            {"id": 1, "trackName": "晴天", "artistName": "周杰伦", "duration": 269.0, "instrumental": false, "syncedLyrics": "[00:29.36]故事的小黄花"},
            {"id": 2, "trackName": "River Flows in You", "artistName": "Yiruma", "instrumental": true, "syncedLyrics": null},
            {"id": 3, "trackName": "Plain only", "artistName": "X", "instrumental": false, "syncedLyrics": null, "plainLyrics": "words"}
        ]"#;
        let found: Vec<_> = hits(serde_json::from_str(answer).unwrap())
            .map(|candidate| (candidate.name, candidate.length, candidate.entry))
            .collect();
        assert_eq!(
            found,
            [
                (
                    TrackName::new("晴天", "周杰伦"),
                    Some(269.0),
                    Some("[00:29.36]故事的小黄花".into())
                ),
                (TrackName::new("River Flows in You", "Yiruma"), None, None),
            ]
        );
    }
}
