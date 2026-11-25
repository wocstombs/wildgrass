//! QQ Music lyrics client (.lrc format with HTML entity decoding).

use reqwest::{Client, RequestBuilder};
use serde::Deserialize;

use super::ranking::{self, Candidate};
use super::request::{self, Artist};
use crate::lyrics::{parse, LyricsMatch, Release};
use crate::track::TrackName;

/// The answer to a search: `{"data": {"song": {"list": [...]}}}`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchAnswer {
    data: SearchData,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchData {
    song: SongList,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SongList {
    list: Vec<Hit>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Hit {
    /// The id its lyrics are downloaded by.
    songmid: String,
    songname: Option<String>,
    title: Option<String>,
    singer: Vec<Artist>,
    /// The recording's length in seconds.
    interval: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LyricAnswer {
    lyric: String,
}

/// A GET carrying the headers every QQ Music request here is sent with.
fn request(client: &Client, url: &str) -> RequestBuilder {
    client
        .get(url)
        .header("Referer", "https://y.qq.com/")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        )
}

/// The entries found, with the id to download each one's lyrics by.
fn hits(answer: SearchAnswer) -> impl Iterator<Item = Candidate<String>> {
    answer
        .data
        .song
        .list
        .into_iter()
        .filter(|hit| !hit.songmid.is_empty())
        .map(|hit| {
            let title = hit.songname.or(hit.title).unwrap_or_default();
            Candidate {
                name: TrackName::new(&title, &request::join_artists(&hit.singer)),
                length: hit.interval.filter(|&length| length > 0.0),
                entry: hit.songmid,
            }
        })
}

pub(super) async fn search(
    client: Client,
    song: TrackName,
    release: Release,
) -> Option<LyricsMatch> {
    let url = format!(
        "https://c.y.qq.com/soso/fcgi-bin/client_search_cp?format=json&w={}&p=1&n=10&t=0",
        urlencoding::encode(&request::search_query(&song))
    );
    let answer: SearchAnswer = request::get_json(request(&client, &url)).await?;

    let client = &client;
    ranking::first_match(&song, release, hits(answer), move |name, songmid| async move {
        let url = format!(
            "https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg?songmid={songmid}&format=json&nobase64=1&g_tk=5381"
        );
        let answer: LyricAnswer = request::get_json(request(client, &url)).await?;
        let text = unescape_html_entities(&answer.lyric);
        LyricsMatch::new_downloaded(name, &text, parse::parse_lrc(&text))
    })
    .await
}

fn unescape_html_entities(text: &str) -> String {
    let mut unescaped = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(c) = characters.next() {
        if c == '&' {
            let mut entity = String::new();
            let mut closed = false;
            for next in characters.by_ref() {
                if next == ';' {
                    closed = true;
                    break;
                }
                if entity.len() > 10 || next.is_whitespace() {
                    entity.push(next);
                    break;
                }
                entity.push(next);
            }
            if closed {
                if let Some(decoded) = decode_entity(&entity) {
                    unescaped.push(decoded);
                } else {
                    unescaped.push('&');
                    unescaped.push_str(&entity);
                    unescaped.push(';');
                }
            } else {
                unescaped.push('&');
                unescaped.push_str(&entity);
            }
        } else {
            unescaped.push(c);
        }
    }
    unescaped
}

fn decode_entity(entity: &str) -> Option<char> {
    if let Some(number) = entity.strip_prefix('#') {
        let code: u32 = if let Some(hex) = number
            .strip_prefix('x')
            .or_else(|| number.strip_prefix('X'))
        {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            number.parse().ok()?
        };
        char::from_u32(code)
    } else {
        match entity {
            "apos" => Some('\''),
            "quot" => Some('"'),
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "nbsp" => Some(' '),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unescape_html_entities() {
        let input = "[00&#58;01&#46;23]&#32;Hello&#10;[00&#58;05&#46;00]&apos;World&apos;";
        let output = unescape_html_entities(input);
        assert_eq!(output, "[00:01.23] Hello\n[00:05.00]'World'");
    }

    #[test]
    fn test_search_answer_lists_entries_with_an_id() {
        let answer = r#"{"code": 0, "data": {"song": {"list": [
            {"songmid": "0039MnYb0qxYhV", "songname": "晴天", "singer": [{"name": "周杰伦", "mid": "x"}], "interval": 269},
            {"songmid": "001naPHz2ER9qv", "songname": "晴天 (Live)", "singer": [{"name": "林俊杰"}, {"name": "周杰伦"}]},
            {"songmid": "", "songname": "No id"}
        ]}}}"#;
        let found: Vec<_> = hits(serde_json::from_str(answer).unwrap())
            .map(|candidate| (candidate.name, candidate.length, candidate.entry))
            .collect();
        assert_eq!(
            found,
            [
                (
                    TrackName::new("晴天", "周杰伦"),
                    Some(269.0),
                    "0039MnYb0qxYhV".into()
                ),
                (
                    TrackName::new("晴天 (Live)", "林俊杰、周杰伦"),
                    None,
                    "001naPHz2ER9qv".into()
                ),
            ]
        );
        // An answer without any songs is an empty list, not an error
        assert_eq!(
            hits(serde_json::from_str(r#"{"code": 0}"#).unwrap()).count(),
            0
        );
    }
}
