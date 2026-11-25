//! Kugou lyrics client (.krc word-timed karaoke format, falling back to .lrc).

use std::fmt;

use reqwest::{Client, RequestBuilder};
use serde::Deserialize;

use super::ranking::{self, Candidate};
use super::request;
use crate::log;
use crate::lyrics::{parse, LyricsMatch, Release};
use crate::track::TrackName;

/// How many of the search hits are ranked.
const MAX_HITS: usize = 8;

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchAnswer {
    candidates: Vec<Hit>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Hit {
    id: Id,
    accesskey: String,
    song: Option<String>,
    singer: String,
    /// The recording's length in milliseconds.
    duration: Option<f64>,
}

/// An entry's id. Kugou sends it as text, but has sent it as a number.
#[derive(Deserialize)]
#[serde(untagged)]
enum Id {
    Text(String),
    Number(u64),
}

impl Default for Id {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => f.write_str(text),
            Self::Number(number) => write!(f, "{number}"),
        }
    }
}

/// What is needed to download an entry's lyrics.
struct Entry {
    id: String,
    access_key: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Download {
    /// The lyrics file, base64-encoded.
    content: String,
}

fn request(client: &Client, url: &str) -> RequestBuilder {
    client.get(url).header("User-Agent", "Mozilla/5.0")
}

/// The entries found for `song`. Entries without a name are taken to be named like the song.
fn hits(answer: SearchAnswer, song: &TrackName) -> impl Iterator<Item = Candidate<Entry>> + '_ {
    answer
        .candidates
        .into_iter()
        .take(MAX_HITS)
        .map(|hit| (hit.id.to_string(), hit))
        .filter(|(id, hit)| !id.is_empty() && !hit.accesskey.is_empty())
        .map(|(id, hit)| {
            let title = hit.song.unwrap_or_else(|| song.title.clone());
            let entry = Entry {
                id,
                access_key: hit.accesskey,
            };
            let length = hit
                .duration
                .filter(|&milliseconds| milliseconds > 0.0)
                .map(|milliseconds| milliseconds / 1000.0);
            Candidate {
                name: TrackName::new(&title, &hit.singer),
                length,
                entry,
            }
        })
}

/// Downloads an entry's lyrics file in `format` (`krc` or `lrc`).
async fn download(client: &Client, entry: &Entry, format: &str) -> Option<Vec<u8>> {
    use base64::Engine;

    let url = format!(
        "http://lyrics.kugou.com/download?ver=1&client=pc&id={}&accesskey={}&fmt={format}&charset=utf8",
        entry.id, entry.access_key
    );
    let answer: Download = request::get_json(request(client, &url)).await?;
    if answer.content.is_empty() {
        return None;
    }
    match base64::prelude::BASE64_STANDARD.decode(&answer.content) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            log::error(format!(
                "lyrics: Kugou {format} file for {}: {error}",
                entry.id
            ));
            None
        }
    }
}

pub(super) async fn search(
    client: Client,
    song: TrackName,
    release: Release,
) -> Option<LyricsMatch> {
    let url = format!(
        "http://lyrics.kugou.com/search?ver=1&man=yes&client=pc&keyword={}",
        urlencoding::encode(&request::search_query(&song))
    );
    let answer: SearchAnswer = request::get_json(request(&client, &url)).await?;

    let (client, hits) = (&client, hits(answer, &song));
    ranking::first_match(&song, release, hits, move |name, entry| async move {
        // Word-by-word .krc first
        let krc = download(client, &entry, "krc").await;
        if let Some(text) = krc.and_then(|bytes| parse::decode_krc(&bytes)) {
            if let Some(found) =
                LyricsMatch::new_downloaded(name.clone(), &text, parse::parse_krc(&text))
            {
                return Some(found);
            }
        }
        // Then line-timed .lrc
        let lrc = download(client, &entry, "lrc").await?;
        let text = String::from_utf8_lossy(&lrc);
        LyricsMatch::new_downloaded(name, &text, parse::parse_lrc(&text))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_answer_lists_entries_with_an_id_and_key() {
        let answer = r#"{"status": 200, "candidates": [
            {"id": "248314971", "accesskey": "7642D7", "song": "海阔天空", "singer": "Beyond", "duration": 326000},
            {"id": 218776158, "accesskey": "1EB45A", "singer": "Beyond"},
            {"id": "192002902", "accesskey": "", "song": "No key", "singer": "Beyond"}
        ]}"#;
        let song = TrackName::new("Hai Kuo Tian Kong", "Beyond");
        let found: Vec<_> = hits(serde_json::from_str(answer).unwrap(), &song)
            .map(|candidate| {
                (
                    candidate.name,
                    candidate.length,
                    candidate.entry.id,
                    candidate.entry.access_key,
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (
                    TrackName::new("海阔天空", "Beyond"),
                    Some(326.0),
                    "248314971".into(),
                    "7642D7".into()
                ),
                // A numeric id, and no name: named like the song
                (song.clone(), None, "218776158".into(), "1EB45A".into()),
            ]
        );
    }
}
