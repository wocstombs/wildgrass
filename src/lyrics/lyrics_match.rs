//! Retrieved lyrics match record and associated metadata.

use serde::{Deserialize, Serialize};

use super::{Lyrics, Release};
use crate::text::{self, Version};
use crate::track::TrackName;

/// Text tag returned by Kugou, NetEase, and QQ Music indicating an instrumental track.
const INSTRUMENTAL_NOTICE: &str = "纯音乐";

/// Matched lyrics catalog entry with timing metadata and optional synchronized lyrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricsMatch {
    pub name: TrackName,
    pub lyrics: Option<Lyrics>,
    /// Track duration reported by the lyrics provider (in seconds).
    #[serde(default)]
    pub length: Option<f64>,
    /// Measured timing latency offset in seconds (positive if lyrics lag vocal onsets).
    #[serde(default)]
    pub late_by: Option<f64>,
}

impl LyricsMatch {
    /// Creates an instrumental match without lyrics text.
    pub(super) fn new_instrumental(name: TrackName) -> Self {
        Self {
            name,
            lyrics: None,
            length: None,
            late_by: None,
        }
    }

    /// The match for a catalogue entry whose lyrics downloaded as `text`, which parsed as
    /// `parsed`. `None` when they are missing or only a teaser clip, so the caller moves on to
    /// its next candidate.
    pub(super) fn new_downloaded(
        name: TrackName,
        text: &str,
        parsed: Option<Lyrics>,
    ) -> Option<Self> {
        let parsed = parsed.map(|lyrics| lyrics.without_title_line(&name.title));
        match parsed.filter(|lyrics| !lyrics.is_snippet()) {
            Some(lyrics) => Some(Self {
                name,
                lyrics: Some(lyrics),
                length: None,
                late_by: None,
            }),
            // An instrumental's notice is all the text there is, so it never passes for lyrics
            None if text.contains(INSTRUMENTAL_NOTICE) => Some(Self::new_instrumental(name)),
            None => None,
        }
    }

    /// Whether this entry is `release`.
    pub(crate) fn is_release(&self, release: Release) -> bool {
        if release.version == Version::Instrumental && self.lyrics.is_some() {
            return false;
        }
        release.is(&self.name.title, self.length)
    }

    /// Whether this cached entry can stand in for a new lookup of `release`. One of unknown
    /// length (saved before lengths were kept) cannot when the release's length is known, so
    /// it is looked up again.
    pub(super) fn fits(&self, release: Release) -> bool {
        self.is_release(release) && (self.length.is_some() || release.length.is_none())
    }

    /// The match with its lyrics in Simplified Chinese, unless the song is Japanese or Korean,
    /// whose Han characters are not Chinese ones.
    pub(super) fn simplified(self) -> Self {
        let mut lines = self.lyrics.iter().flat_map(|lyrics| &lyrics.lines);
        let foreign = text::has_kana_or_hangul(&self.name.title)
            || text::has_kana_or_hangul(&self.name.artist)
            || lines.any(|line| text::has_kana_or_hangul(&line.text));
        if foreign {
            return self;
        }
        Self {
            lyrics: self.lyrics.map(Lyrics::simplified),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::parse;
    use crate::test_util;

    fn downloaded(text: &str) -> Option<LyricsMatch> {
        let name = TrackName::new("River Flows in You", "Yiruma");
        LyricsMatch::new_downloaded(name, text, parse::parse_lrc(text))
    }

    #[test]
    fn test_instrumental_notice_means_no_lyrics() {
        // NetEase's actual lyrics for Yiruma's "River Flows in You"
        let netease = "[00:00.00] 作曲 : Yiruma\n[00:05.00]纯音乐，请欣赏\n";
        assert_eq!(downloaded(netease).unwrap().lyrics, None);
        // QQ Music times its notice at the very start, so it does not parse as synced lyrics
        let qq = "[00:00.00]此歌曲为没有填词的纯音乐，请您欣赏\n";
        assert!(parse::parse_lrc(qq).is_none());
        assert_eq!(downloaded(qq).unwrap().lyrics, None);

        // Full lyrics are lyrics, even with the word in them; a short clip is no match at all
        let sung = format!("{}[00:30.00]纯音乐\n", test_util::lrc_lines(8, "Sung line"));
        assert!(downloaded(&sung).unwrap().lyrics.is_some());
        assert!(downloaded(&test_util::lrc_lines(3, "Clip")).is_none());
    }

    #[test]
    fn test_title_line_at_the_start_is_not_a_lyric() {
        // How QQ Music's lyrics for Gloria Estefan's "1-2-3" begin
        let qq = format!(
            "[00:00.00]1-2-3 - Gloria Estefan (格洛丽娅·艾斯特芬)\n{}",
            test_util::lrc_lines(8, "They tell me")
        );
        let name = TrackName::new("1-2-3", "Gloria Estefan");
        let found = LyricsMatch::new_downloaded(name, &qq, parse::parse_lrc(&qq)).unwrap();
        let lines = found.lyrics.unwrap().lines;
        assert_eq!(lines.len(), 8);
        assert_eq!(lines[0].text, "They tell me 1");

        // A sung line that merely starts with the title stays
        let sung = format!(
            "[00:00.50]Hello - is it me\n{}",
            test_util::lrc_lines(8, "Line")
        );
        let name = TrackName::new("Hello", "Adele");
        let found = LyricsMatch::new_downloaded(name, &sung, parse::parse_lrc(&sung)).unwrap();
        assert_eq!(found.lyrics.unwrap().lines.len(), 9);
    }

    #[test]
    fn test_lyrics_are_simplified_unless_the_song_is_japanese_or_korean() {
        let simplified = |title, artist, lrc: &str| {
            let found = LyricsMatch {
                name: TrackName::new(title, artist),
                lyrics: parse::parse_lrc(lrc),
                length: None,
                late_by: None,
            };
            found.simplified().lyrics.unwrap().lines[0].text.clone()
        };
        let beyond = "[00:01.00]今天我 寒夜裏看雪飄過\n[00:05.00]懷着冷卻了的心窩飄遠方\n";
        assert_eq!(
            simplified("海闊天空", "Beyond", beyond),
            "今天我 寒夜里看雪飘过"
        );

        // Japanese lyrics keep their characters, even under a title written in kanji alone
        let japanese = "[00:01.00]愛を歌う\n";
        assert_eq!(simplified("愛", "Singer", japanese), "愛を歌う");
    }

    #[test]
    fn test_entries_of_another_length_are_another_release() {
        let entry = |length| LyricsMatch {
            name: TrackName::new("1-2-3", "Gloria Estefan"),
            lyrics: None,
            length,
            late_by: None,
        };
        // The album version (3:28) is not the single (3:36); a re-encode a second off is
        let single = Release::new("1-2-3 (Single Version)", Some(215.9));
        assert!(!single.is("1-2-3", Some(208.3)));
        assert!(single.is("1-2-3", Some(216.3)));
        assert!(single.is("1-2-3", None));
        let unknown = Release::new("1-2-3", None);
        assert!(unknown.is("1-2-3", Some(208.3)));

        assert!(entry(Some(216.3)).fits(single));
        assert!(!entry(Some(208.3)).fits(single));
        // A cached entry saved before lengths were kept is looked up again
        assert!(!entry(None).fits(single));
        assert!(entry(None).fits(unknown));
    }

    #[test]
    fn test_sung_lyrics_never_match_an_instrumental_release() {
        let sung_lines = parse::parse_lrc(&test_util::lrc_lines(8, "冷雨夜我在你身边"));
        let instrumental = Release::new("冷雨夜 (雨夜之后(演奏版))", Some(301.6));

        let mistagged = LyricsMatch {
            name: TrackName::new("冷雨夜 (演奏版)", "Beyond"),
            lyrics: sung_lines,
            length: Some(302.0),
            late_by: None,
        };
        assert!(!mistagged.is_release(instrumental));

        let pure = LyricsMatch {
            lyrics: None,
            ..mistagged
        };
        assert!(pure.is_release(instrumental));
    }
}
