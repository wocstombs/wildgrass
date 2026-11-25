//! Persistent disk cache for lyrics: stores JSON files indexed by song artist and title.

use std::path::PathBuf;
use std::{fs, io};

use crc32fast::Hasher;
use serde::Deserialize;

use super::release::LENGTH_TOLERANCE;
use super::{Lyrics, LyricsMatch};
use crate::track::TrackName;
use crate::{files, log, text};

/// Deserialization helper supporting both single-entry legacy caches and modern release arrays.
#[derive(Deserialize)]
#[serde(untagged)]
enum CachedEntries {
    Many(Vec<LyricsMatch>),
    One(LyricsMatch),
}

/// Checks whether two cached entries represent the same musical release.
/// Used to replace existing entries (e.g. updating timing offsets) rather than creating duplicates.
fn same_release(existing: &LyricsMatch, incoming: &LyricsMatch) -> bool {
    if text::version_of(&existing.name.title) != text::version_of(&incoming.name.title) {
        return false;
    }
    match (existing.length, incoming.length) {
        (Some(a), Some(b)) => (a - b).abs() <= LENGTH_TOLERANCE,
        (None, None) | (None, Some(_)) => true,
        (Some(_), None) => false,
    }
}

/// Disk-backed lyrics cache in `directory`.
#[derive(Clone)]
pub struct LyricsCache {
    directory: PathBuf,
}

impl LyricsCache {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn file_path(&self, name: &TrackName) -> PathBuf {
        let mut hasher = Hasher::new();
        hasher.update(name.artist.to_lowercase().as_bytes());
        hasher.update(b"\0");
        hasher.update(name.title.to_lowercase().as_bytes());
        let crc = hasher.finalize();

        let safe = |text: &str| -> String {
            text.chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .take(20)
                .collect()
        };
        let filename = format!(
            "{}_{}_{crc:08x}.json",
            safe(&name.artist),
            safe(&name.title)
        );
        self.directory.join(filename)
    }

    /// Every non-snippet release saved for `name` (empty when nothing is cached there).
    pub fn load(&self, name: &TrackName) -> Vec<LyricsMatch> {
        let Ok(bytes) = fs::read(self.file_path(name)) else {
            return Vec::new();
        };
        let parsed: serde_json::Result<CachedEntries> = serde_json::from_slice(&bytes);
        let entries = match parsed {
            Ok(CachedEntries::Many(entries)) => entries,
            Ok(CachedEntries::One(entry)) => vec![entry],
            Err(error) => {
                log::error(format!("lyrics cache: {}: {error}", name.display()));
                return Vec::new();
            }
        };
        entries
            .into_iter()
            .filter(|matched| !matched.lyrics.as_ref().is_some_and(Lyrics::is_snippet))
            .collect()
    }

    /// Saves `matched` as what the lookup of `track` found: under the catalogue's name for it,
    /// and also under `track` when that differs, which is what the next lookup asks for.
    pub fn save_lookup(&self, track: &TrackName, matched: &LyricsMatch) {
        self.save(&matched.name, matched);
        if matched.name != *track {
            self.save(track, matched);
        }
    }

    /// Merges `matched` into the releases cached for `name`, replacing any existing entry for
    /// the same release and leaving other releases intact.
    fn save(&self, name: &TrackName, matched: &LyricsMatch) {
        let mut entries = self.load(name);
        match entries
            .iter()
            .position(|existing| same_release(existing, matched))
        {
            Some(index) => entries[index] = matched.clone(),
            None => entries.push(matched.clone()),
        }
        let path = self.file_path(name);
        let written = serde_json::to_vec(&entries)
            .map_err(io::Error::from)
            .and_then(|json| files::write_atomically(&path, &json));
        if let Err(error) = written {
            log::error(format!("lyrics cache: saving {}: {error}", path.display()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::parse;
    use crate::test_util::ScratchDir;

    #[test]
    fn test_persistent_lyric_caching() {
        let scratch = ScratchDir::create();
        let cache = LyricsCache::new(scratch.path().join("lyrics"));
        let lrc: String = (1..=8)
            .map(|i| format!("[00:{:02}.00]测试歌词第{i}行\n", i * 8))
            .collect();
        let name = TrackName::new("Test Song", "Test Artist");
        let entry = LyricsMatch {
            name: name.clone(),
            lyrics: parse::parse_lrc(&lrc),
            length: Some(215.9),
            late_by: None,
        };

        assert!(cache.load(&name).is_empty());
        cache.save(&name, &entry);
        assert_eq!(cache.load(&name), vec![entry.clone()]);
        assert!(cache
            .load(&TrackName::new("Other Song", "Test Artist"))
            .is_empty());

        // Filed under another name too, it keeps the catalogue's name
        let reported = TrackName::new("Test Song (Single Version)", "测试歌手");
        cache.save(&reported, &entry);
        assert_eq!(cache.load(&reported)[0].name, name);
    }

    #[test]
    fn test_multiple_releases_of_one_song_do_not_overwrite_each_other() {
        let scratch = ScratchDir::create();
        let cache = LyricsCache::new(scratch.path().join("lyrics"));
        let name = TrackName::new("1-2-3", "Gloria Estefan");
        let album = LyricsMatch {
            name: name.clone(),
            lyrics: None,
            length: Some(208.3),
            late_by: None,
        };
        let single = LyricsMatch {
            name: name.clone(),
            lyrics: None,
            length: Some(215.9),
            late_by: None,
        };

        cache.save(&name, &album);
        cache.save(&name, &single);
        assert_eq!(cache.load(&name), vec![album.clone(), single.clone()]);

        // Updating `late_by` replaces only the matching release
        let timed_single = LyricsMatch {
            late_by: Some(1.2),
            ..single
        };
        cache.save(&name, &timed_single);
        assert_eq!(cache.load(&name), vec![album, timed_single]);
    }

    #[test]
    fn test_entries_saved_before_lengths_were_kept_still_load() {
        let scratch = ScratchDir::create();
        let directory = scratch.path().join("lyrics");
        let cache = LyricsCache::new(directory.clone());
        let name = TrackName::new("Old Song", "Old Artist");
        fs::create_dir_all(&directory).unwrap();
        let old = r#"{"name": {"title": "Old Song", "artist": "Old Artist"}, "lyrics": null}"#;
        fs::write(cache.file_path(&name), old).unwrap();

        let loaded = cache.load(&name);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].length, None);
    }
}
