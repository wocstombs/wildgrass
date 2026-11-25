//! Track metadata representation and display formatting.

use serde::{Deserialize, Serialize};

use crate::lyrics::Lyrics;
use crate::{chinese, text};

/// Normalized song title and artist information.
/// Always stored trimmed, converted to Simplified Chinese, with romanized aliases mapped.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TrackName {
    pub title: String,
    pub artist: String,
}

impl TrackName {
    pub fn new(title: &str, artist: &str) -> Self {
        chinese::simplified_name(title.trim(), artist.trim())
    }

    /// Formats as `Artist - Title` (or `Title` if artist is unknown).
    /// Strips bracketed descriptors, shortens performer lists, standardizes numbering, and spells
    /// out a key's flat or sharp (`E♭` → `E-flat`, `C♯` → `C-sharp`).
    pub fn display(&self) -> String {
        let title = text::strip_display_title(&self.title);
        let artist = display_artist(&self.artist);
        let name = if artist.is_empty() {
            title
        } else {
            format!("{artist} - {title}")
        };
        text::spell_out_flats_and_sharps(&text::tidy_numbering(&name))
    }

    /// Checks if `other` refers to the same underlying song title across versions or releases.
    pub fn same_song(&self, other: &TrackName) -> bool {
        let title = text::normalize_text(&text::strip_display_title(&self.title));
        !title.is_empty() && title == text::normalize_text(&text::strip_display_title(&other.title))
    }
}

/// Picks the name to display so it is written the way the song is sung: Chinese, Japanese and
/// Korean songs show their names in their own script, everyone else in Latin letters.
///
/// `reported` is what ICY metadata or Shazam reported; `canonical` is the matching entry from
/// the lyrics catalogue (Kugou / QQ / NetEase / LRCLIB), when one was found, and `lyrics` are
/// its lyrics. Shazam is asked for Chinese names, so it reports Western artists transliterated
/// ("葛洛丽亚·伊斯特芬" for Gloria Estefan); the lyrics tell that the song is not Chinese.
pub fn choose_display_name(
    reported: &TrackName,
    canonical: Option<&TrackName>,
    lyrics: Option<&Lyrics>,
) -> TrackName {
    let Some(canonical) = canonical.filter(|c| !c.title.is_empty() && !c.artist.is_empty()) else {
        return reported.clone();
    };
    // Without lyrics, a title in CJK characters is the best sign of the language
    let sung_in_cjk = lyrics.map_or_else(
        || text::contains_cjk(&reported.title) || text::contains_cjk(&canonical.title),
        Lyrics::is_cjk,
    );
    // A name in the song's script beats one in the other; otherwise the catalogue's spelling wins
    let pick = |canonical: &str, reported: &str| {
        let use_reported = text::contains_cjk(canonical) != sung_in_cjk
            && text::contains_cjk(reported) == sung_in_cjk;
        if use_reported { reported } else { canonical }.to_string()
    };
    // Both names are already in `TrackName` form, so no need to go through `new` again
    TrackName {
        title: pick(repaired_title(canonical, &reported.title), &reported.title),
        artist: pick(&canonical.artist, &reported.artist),
    }
}

/// Repairs a catalogue title tagged `"{artist} - {title}"`, as some LRCLIB uploads are: drops
/// the artist, and returns `reported_title` instead when the rest is a cut-off start of it.
fn repaired_title<'a>(canonical: &'a TrackName, reported_title: &'a str) -> &'a str {
    let artist = text::normalize_text(&canonical.artist);
    let Some(title) = text::TAG_DASHES
        .iter()
        .filter_map(|dash| canonical.title.split_once(dash))
        .find(|(head, _)| text::normalize_text(head) == artist)
        .map(|(_, title)| title.trim())
    else {
        return &canonical.title;
    };
    // ID3v1 holds 30 bytes of title: "Michael Jackson - You Are Not Alone" ends at "Not"
    let reported = text::normalize_text(&text::strip_display_title(reported_title));
    let is_cut_off = reported
        .strip_prefix(&text::normalize_text(title))
        .is_some_and(|rest| !rest.is_empty());
    if is_cut_off {
        reported_title
    } else {
        title
    }
}

fn split_artists(artist: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    for semicolon_part in artist.split(';') {
        for dunhao_part in semicolon_part.split('、') {
            for comma_part in dunhao_part.split(", ") {
                for slash_part in comma_part.split(" / ") {
                    let trimmed = slash_part.trim();
                    if !trimmed.is_empty() {
                        parts.push(trimmed);
                    }
                }
            }
        }
    }
    parts
}

/// Keeps single artists and duets intact while collapsing 3+ credit-listed performers (such as
/// `"CoCo李玟;马毓芬;谢文德;..."`) down to the primary artist for display.
fn display_artist(artist: &str) -> String {
    let parts = split_artists(artist);
    if parts.len() > 2 {
        return parts[0].to_string();
    }
    if artist.contains(';') && parts.len() == 2 {
        return join_artists(&parts);
    }
    artist.to_string()
}

/// Joins artist names with the separator customary for their script: "、" when any of them
/// is written in CJK characters, ", " otherwise.
pub fn join_artists(names: &[&str]) -> String {
    let separator = if names.iter().any(|name| text::contains_cjk(name)) {
        "、"
    } else {
        ", "
    };
    names.join(separator)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::LyricLine;

    #[test]
    fn test_choose_display_name_keeps_artist_and_title_in_one_language() {
        let icy_romanized = TrackName::new("Sunny Day", "Jay Chou");
        let kugou = TrackName::new("晴天", "周杰伦");
        assert_eq!(
            choose_display_name(&icy_romanized, Some(&kugou), None),
            kugou
        );

        // Chinese source beats a romanized catalogue entry and is simplified
        let shazam_cjk = TrackName::new("晴天", "周杰倫");
        let lrclib_romanized = TrackName::new("Qing Tian", "Jay Chou");
        let simplified = TrackName::new("晴天", "周杰伦");
        assert_eq!(
            choose_display_name(&shazam_cjk, Some(&lrclib_romanized), None),
            simplified
        );

        // English artists use the catalogue's English name and title
        let icy_lower = TrackName::new("shape of you", "ed sheeran");
        let netease = TrackName::new("Shape of You", "Ed Sheeran");
        assert_eq!(
            choose_display_name(&icy_lower, Some(&netease), None),
            netease
        );

        assert_eq!(choose_display_name(&icy_lower, None, None), icy_lower);
        assert_eq!(icy_lower.display(), "ed sheeran - shape of you");

        // Pinyin song title with English band name translates title to Simplified Chinese while keeping English artist
        let pinyin_beyond = TrackName::new(
            "Hui Se Gui Ji (Dian Ying \"Tian Ruo You Qing\" Ge Qu)",
            "Beyond",
        );
        assert_eq!(
            choose_display_name(&pinyin_beyond, None, None),
            TrackName::new("灰色轨迹", "Beyond")
        );
    }

    #[test]
    fn test_songs_not_sung_in_chinese_keep_latin_artist_names() {
        let lyrics = |line: &str| {
            let lines = (1..=8)
                .map(|i| LyricLine {
                    time: f64::from(i) * 8.0,
                    text: format!("{line} {i}"),
                    words: None,
                })
                .collect();
            Lyrics::from_lines(lines).unwrap()
        };
        let english = lyrics("They tell me you're shy boy");

        // Shazam (asked for Chinese names) transliterates the artist; the catalogue does not
        let shazam = TrackName::new("1-2-3 (Single Version)", "葛洛丽亚·伊斯特芬");
        let qq = TrackName::new("1-2-3", "Gloria Estefan, Miami Sound Machine");
        assert_eq!(choose_display_name(&shazam, Some(&qq), Some(&english)), qq);
        // And the other way round: a catalogue listing the artist in Chinese
        let icy = TrackName::new("1-2-3", "Gloria Estefan");
        let kugou = TrackName::new("1-2-3", "葛洛丽亚·伊斯特芬");
        assert_eq!(choose_display_name(&icy, Some(&kugou), Some(&english)), icy);

        // A Chinese song keeps its Chinese artist name, even under an English title
        let sung = lyrics("故事的小黄花");
        let reported = TrackName::new("Mojito", "某歌手");
        let lrclib = TrackName::new("Mojito", "Some Singer");
        assert_eq!(
            choose_display_name(&reported, Some(&lrclib), Some(&sung)),
            reported
        );
    }

    #[test]
    fn test_catalogue_title_tagged_with_its_artist_is_repaired() {
        // LRCLIB entry truncated at the 30-byte ID3v1 limit with the artist prefixed
        let shazam = TrackName::new("You Are Not Alone (Radio Edit)", "Michael Jackson");
        let lrclib = TrackName::new("Michael Jackson - You Are Not", "Michael Jackson");
        assert_eq!(
            choose_display_name(&shazam, Some(&lrclib), None).display(),
            "Michael Jackson - You Are Not Alone"
        );

        // A complete title keeps the catalogue's spelling
        let icy = TrackName::new("you are not alone", "michael jackson");
        let lrclib = TrackName::new("michael jackson – You Are Not Alone", "Michael Jackson");
        assert_eq!(
            choose_display_name(&icy, Some(&lrclib), None).display(),
            "Michael Jackson - You Are Not Alone"
        );

        // A title naming someone else keeps its dash
        let reported = TrackName::new("Song", "Artist");
        let catalogue = TrackName::new("Other Artist - Song", "Artist");
        assert_eq!(
            choose_display_name(&reported, Some(&catalogue), None),
            catalogue
        );
    }

    #[test]
    fn test_display_strips_brackets_and_long_credit_lists() {
        let shazam = TrackName::new("1-2-3 (Single Version)", "Gloria Estefan");
        assert_eq!(shazam.display(), "Gloria Estefan - 1-2-3");
        assert_eq!(TrackName::new("Song (Live)", "").display(), "Song");
        assert_eq!(
            TrackName::new("无心睡眠 (Instrumental)", "张国荣").display(),
            "张国荣 - 无心睡眠"
        );
        assert_eq!(
            TrackName::new("记事本 (DJ版)", "陈慧琳").display(),
            "陈慧琳 - 记事本"
        );
        assert_eq!(
            TrackName::new(
                "月光爱人",
                "CoCo李玟;马毓芬;谢文德;Dan Warner;Julio Hernández;Archie Pena;马友友"
            )
            .display(),
            "CoCo李玟 - 月光爱人"
        );
        assert_eq!(
            TrackName::new("1-2-3", "Gloria Estefan, Miami Sound Machine").display(),
            "Gloria Estefan, Miami Sound Machine - 1-2-3"
        );
    }

    #[test]
    fn test_display_writes_catalogue_numbers_the_usual_way() {
        // As Classic FM sends it
        let classic_fm = TrackName::new(
            "Piano Concerto No.1 in E minor Opus 11 (2)",
            "Frederic Chopin, Daniil Trifonov",
        );
        assert_eq!(
            classic_fm.display(),
            "Frederic Chopin, Daniil Trifonov - Piano Concerto No. 1 in E minor Op. 11"
        );
        // Only how it shows: the name itself, and so what is searched and compared, is unchanged
        assert_eq!(
            classic_fm.title,
            "Piano Concerto No.1 in E minor Opus 11 (2)"
        );
        assert!(classic_fm.same_song(&TrackName::new(
            "Piano Concerto No.1 in E minor Opus 11",
            "Chopin"
        )));
    }

    #[test]
    fn test_same_song_tolerates_spelling_but_not_different_titles() {
        let icy = TrackName::new("Shape of You", "Ed Sheeran");
        assert!(icy.same_song(&TrackName::new("shape of you!", "ED SHEERAN")));
        // Traditional vs simplified artist name from Shazam vs Kugou
        assert!(TrackName::new("晴天", "周杰伦").same_song(&TrackName::new("晴天", "周杰倫")));
        // Pinyin title matches Simplified Chinese title
        assert!(TrackName::new(
            "Hui Se Gui Ji (Dian Ying \"Tian Ruo You Qing\" Ge Qu)",
            "Beyond"
        )
        .same_song(&TrackName::new("灰色轨迹", "Beyond")));
        assert!(!icy.same_song(&TrackName::new("Perfect", "Ed Sheeran")));
        // Releases and versions of one song are that song
        let plain = TrackName::new("奉献", "苏芮");
        assert!(plain.same_song(&TrackName::new("奉献 (Remastered)", "苏芮")));
        assert!(plain.same_song(&TrackName::new("奉献 (Live)", "苏芮")));
        assert!(TrackName::new("(Intro)", "&ME").same_song(&TrackName::new("(Intro)", "&ME")));
        assert!(!TrackName::new("", "Ed Sheeran").same_song(&TrackName::new("", "Ed Sheeran")));
    }
}
