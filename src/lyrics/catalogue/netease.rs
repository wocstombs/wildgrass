//! NetEase Cloud Music lyrics client (.lrc format with instrumental detection).

use reqwest::{Client, RequestBuilder};
use serde::Deserialize;

use super::ranking::{self, Candidate};
use super::request::{self, Artist};
use crate::lyrics::{parse, LyricsMatch, Release};
use crate::track::TrackName;

/// How many of the search hits are ranked.
const MAX_HITS: usize = 8;

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchAnswer {
    result: SearchResult,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResult {
    songs: Vec<Hit>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Hit {
    id: Option<i64>,
    name: String,
    artists: Vec<Artist>,
    /// The recording's length in milliseconds.
    duration: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct LyricAnswer {
    pure_music: Option<bool>,
    lrc: Lrc,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Lrc {
    lyric: String,
}

/// A GET carrying the headers every NetEase request here is sent with.
fn request(client: &Client, url: &str) -> RequestBuilder {
    client
        .get(url)
        .header("Referer", "https://music.163.com")
        .header("User-Agent", "Mozilla/5.0")
}

/// The entries found, with the id to download each one's lyrics by.
fn hits(answer: SearchAnswer) -> impl Iterator<Item = Candidate<i64>> {
    answer
        .result
        .songs
        .into_iter()
        .take(MAX_HITS)
        .filter_map(|hit| {
            let length = hit
                .duration
                .filter(|&milliseconds| milliseconds > 0.0)
                .map(|milliseconds| milliseconds / 1000.0);
            Some(Candidate {
                name: TrackName::new(&hit.name, &request::join_artists(&hit.artists)),
                length,
                entry: hit.id?,
            })
        })
}

pub(super) async fn search(
    client: Client,
    song: TrackName,
    release: Release,
) -> Option<LyricsMatch> {
    let url = format!(
        "https://music.163.com/api/search/get?s={}&type=1&limit={MAX_HITS}",
        urlencoding::encode(&request::search_query(&song))
    );
    let answer: SearchAnswer = request::get_json(request(&client, &url)).await?;

    let client = &client;
    ranking::first_match(&song, release, hits(answer), move |name, id| async move {
        let url = format!("https://music.163.com/api/song/lyric?id={id}&lv=-1&tv=-1");
        let answer: LyricAnswer = request::get_json(request(client, &url)).await?;
        if answer.pure_music == Some(true) {
            return Some(LyricsMatch::new_instrumental(name));
        }
        let text = answer.lrc.lyric;
        LyricsMatch::new_downloaded(name, &text, parse::parse_lrc(&text))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_answers_parse_into_entries_and_the_instrumental_flag() {
        let search = r#"{"code": 200, "result": {"songs": [
            {"id": 3440441479, "name": "晴天", "artists": [{"id": 1, "name": "Jay"}], "album": {}, "duration": 269000},
            {"name": "No id", "artists": []}
        ]}}"#;
        let found: Vec<_> = hits(serde_json::from_str(search).unwrap())
            .map(|candidate| (candidate.name, candidate.length, candidate.entry))
            .collect();
        assert_eq!(
            found,
            [(TrackName::new("晴天", "Jay"), Some(269.0), 3440441479)]
        );

        // NetEase's actual answer for Yiruma's "River Flows in You"
        let lyric = r#"{"pureMusic": true, "briefDesc": null, "code": 200,
            "lrc": {"version": 1, "lyric": "[00:00.00] 作曲 : Yiruma\n[00:05.00]纯音乐，请欣赏\n"}}"#;
        let answer: LyricAnswer = serde_json::from_str(lyric).unwrap();
        assert_eq!(answer.pure_music, Some(true));
        assert!(answer.lrc.lyric.contains("纯音乐"));
    }
}
