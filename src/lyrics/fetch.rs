//! Lyrics lookup pipeline: queries local disk cache first, falling back to online catalogues.

use std::sync::OnceLock;

use regex::Regex;
use reqwest::Client;

use super::{catalogue, Lyrics, LyricsCache, LyricsMatch, Release};
use crate::track::TrackName;
use crate::{log, text};

/// Successful lyrics lookup result and release match status.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub entry: LyricsMatch,
    /// True if the entry matches the currently playing release duration/version.
    pub in_time: bool,
}

impl Found {
    /// Returns synchronized lyrics if they align with the active release.
    pub fn lyrics(&self) -> Option<&Lyrics> {
        self.entry.lyrics.as_ref().filter(|_| self.in_time)
    }
}

/// Normalizes song titles and artist names for catalog search queries.
fn search_name(track: &TrackName) -> TrackName {
    static FEATURED_REGEX: OnceLock<Regex> = OnceLock::new();
    let featured_regex = FEATURED_REGEX
        .get_or_init(|| Regex::new(r"(?i)(feat\.|ft\.|&|/).*").expect("a valid pattern"));

    let title = text::strip_display_title(&track.title);
    let artist = featured_regex.replace(&track.artist, "");
    let artist = artist.trim();
    // An artist that is nothing but extras (such as "&ME") is searched as it is
    let artist = if artist.is_empty() {
        track.artist.as_str()
    } else {
        artist
    };
    TrackName::new(&title, artist)
}

/// Finds the catalogue entry for a song, with its synced lyrics unless it is an instrumental.
///
/// `release` is what is known of the release that is playing. The entry for that release is
/// what is looked for, in `cache` (when provided) and then in the catalogues, under the name
/// without its extras and under the full name. An entry for another release is returned only
/// when there is no other, and marked as not in time.
pub async fn fetch_lyrics(
    client: &Client,
    track: &TrackName,
    release: Release,
    cache: Option<&LyricsCache>,
) -> Option<Found> {
    let query = search_name(track);
    // The full name is the last resort: it is what finds a remix or a live cut, but the
    // catalogues also answer it with entries that merely repeat its tags
    let mut full = Some(track.clone()).filter(|full| *full != query);

    let cached: Vec<LyricsMatch> = match cache {
        Some(cache) => [Some(&query), full.as_ref()]
            .into_iter()
            .flatten()
            .flat_map(|name| cache.load(name))
            .collect(),
        None => Vec::new(),
    };
    if let Some(fitting) = cached.iter().find(|cached| cached.fits(release)) {
        return Some(found(track, release, fitting.clone(), "cache"));
    }

    let mut best: Option<catalogue::CatalogueHit> = None;
    let mut searched: Vec<TrackName> = Vec::new();
    let mut next = Some(query);
    while let Some(name) = next.take().or_else(|| full.take()) {
        if searched.contains(&name) {
            continue;
        }
        let result = catalogue::search(client, &name, release).await;
        searched.push(name);
        let is_release = result
            .as_ref()
            .is_some_and(|hit| hit.matched.is_release(release));
        if is_release || best.is_none() {
            best = result;
        }
        if is_release {
            break;
        }
        // Found only another release: the catalogues may list the one that is playing under
        // the name that entry gives, such as "Gloria Estefan" where Shazam reported
        // "葛洛丽亚·伊斯特芬" (a name only some catalogues know)
        if let Some(other) = best.as_ref().filter(|_| searched.len() == 1) {
            next = Some(search_name(&other.matched.name));
        }
    }
    let Some(hit) = best else {
        log::sync(format!("lyrics for {}: none found", track.display()));
        return cached
            .into_iter()
            .next()
            .map(|entry| found(track, release, entry, "cache"));
    };
    let best = hit.matched.simplified();

    if let Some(cache) = cache {
        cache.save_lookup(track, &best);
    }
    Some(found(track, release, best, hit.provider))
}

/// The lookup's result for `entry`, noted in the sync log.
fn found(track: &TrackName, release: Release, entry: LyricsMatch, source: &str) -> Found {
    let in_time = entry.is_release(release);
    let seconds = |length: Option<f64>| {
        length.map_or("unknown length".into(), |length| format!("{length:.1}s"))
    };
    let measured = entry.late_by.map_or(String::new(), |late_by| {
        format!(", lines shown {late_by:+.2}s earlier, as measured against the singing")
    });
    log::sync(format!(
        "lyrics for \"{}\" ({}, {:?}): entry \"{}\" ({}) from {source} is {}{measured}",
        track.title,
        seconds(release.length),
        release.version,
        entry.name.title,
        seconds(entry.length),
        match (in_time, entry.lyrics.is_some()) {
            (true, true) => "the release playing: lyrics shown",
            (true, false) => "the release playing, an instrumental",
            (false, _) => "another release: lyrics hidden",
        }
    ));
    Found { entry, in_time }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::parse;
    use crate::test_util::{self, ScratchDir};

    #[test]
    fn test_lyrics_of_another_release_are_not_shown() {
        let entry = LyricsMatch {
            name: TrackName::new("奉献", "苏芮"),
            lyrics: parse::parse_lrc(&test_util::lrc_lines(8, "长路奉献给远方")),
            length: Some(316.0),
            late_by: None,
        };
        let playing = Release::new("奉献", Some(290.3));
        let other = found(&entry.name, playing, entry.clone(), "QQ Music");
        assert!(!other.in_time);
        assert!(other.lyrics().is_none());
        // The entry itself keeps its lyrics, which tell the language the song is sung in
        assert!(other.entry.lyrics.is_some());

        let same = found(
            &entry.name.clone(),
            Release::new("奉献", Some(316.1)),
            entry,
            "QQ Music",
        );
        assert!(same.lyrics().is_some());
    }

    #[test]
    fn test_search_name_strips_extras_and_featured_artists() {
        let search = |title, artist| search_name(&TrackName::new(title, artist));
        assert_eq!(
            search(
                "Cruel Summer [Radio Edit] (feat. Someone)",
                "Taylor Swift feat. Someone"
            ),
            TrackName::new("Cruel Summer", "Taylor Swift")
        );
        assert_eq!(
            search(
                "My Heart Will Go On (Love Theme from \"Titanic\")",
                "Celine Dion"
            ),
            TrackName::new("My Heart Will Go On", "Celine Dion")
        );
        assert_eq!(
            search("Song - Remastered 2011 (Live)", "Artist"),
            TrackName::new("Song", "Artist")
        );
        // A name that is nothing but extras stays as it is
        assert_eq!(search("(Intro)", "&ME"), TrackName::new("(Intro)", "&ME"));
    }

    #[tokio::test]
    async fn test_fetch_lyrics_uses_the_passed_cache() {
        let scratch = ScratchDir::create();
        let cache = LyricsCache::new(scratch.path().join("lyrics"));
        let track = TrackName::new("奉献", "苏芮");
        let album = LyricsMatch {
            name: track.clone(),
            lyrics: parse::parse_lrc(&test_util::lrc_lines(8, "专辑版歌词")),
            length: Some(316.0),
            late_by: None,
        };
        let single = LyricsMatch {
            name: track.clone(),
            lyrics: parse::parse_lrc(&test_util::lrc_lines(8, "长路奉献给远方")),
            length: Some(290.0),
            late_by: None,
        };
        cache.save_lookup(&track, &album);
        cache.save_lookup(&track, &single);

        let release = Release::new(&track.title, Some(290.3));
        let result = fetch_lyrics(&Client::new(), &track, release, Some(&cache))
            .await
            .expect("should hit the passed cache");
        assert!(result.in_time);
        assert_eq!(result.entry, single);
    }
}
