//! Shared HTTP request and query formatting helpers for lyrics catalog clients.

use reqwest::RequestBuilder;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::http;
use crate::track::{self, TrackName};

/// Dispatches an HTTP request and deserializes the JSON response for lyrics queries.
pub(super) async fn get_json<T: DeserializeOwned>(request: RequestBuilder) -> Option<T> {
    http::get_json(request, "lyrics").await
}

/// Formats the search query string (`Artist Title`) for catalogue lookups.
pub(super) fn search_query(song: &TrackName) -> String {
    format!("{} {}", song.artist, song.title).trim().to_string()
}

/// An artist as QQ Music and NetEase list them.
#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct Artist {
    pub(super) name: String,
}

/// The artists' names, joined with the separator customary for their script (see
/// `track::join_artists`).
pub(super) fn join_artists(artists: &[Artist]) -> String {
    let names: Vec<&str> = artists
        .iter()
        .map(|artist| artist.name.trim())
        .filter(|name| !name.is_empty())
        .collect();
    track::join_artists(&names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_artists_uses_script_appropriate_separator() {
        let artists = |names: [&str; 2]| names.map(|name| Artist { name: name.into() });
        assert_eq!(
            join_artists(&artists(["周杰伦", "费玉清"])),
            "周杰伦、费玉清"
        );
        assert_eq!(
            join_artists(&artists(["Lady Gaga", "Bruno Mars"])),
            "Lady Gaga, Bruno Mars"
        );
    }
}
