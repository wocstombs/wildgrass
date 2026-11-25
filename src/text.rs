//! Text normalization and sanitization utilities for song title comparison.

use std::sync::OnceLock;

use regex::Regex;

/// Opening and closing brackets enclosing song title extras (ASCII and full-width).
const OPENING_BRACKETS: [char; 5] = ['(', '[', '{', '（', '【'];
const CLOSING_BRACKETS: [char; 5] = [')', ']', '}', '）', '】'];

/// Hyphens separating parts of a tagged name: a trailing release tag ("Song - Remastered 2011")
/// or a leading artist ("Artist - Song").
pub const TAG_DASHES: [&str; 2] = [" - ", " – "];

/// Normalizes text for comparison: lowercases, converts full-width ASCII to half-width,
/// collapses punctuation runs into single spaces, and preserves alphanumeric and CJK characters.
pub fn normalize_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_was_space = true;

    for character in input.chars() {
        let folded = match character {
            '\u{FF01}'..='\u{FF5E}' => {
                char::from_u32(character as u32 - 0xfee0).unwrap_or(character)
            }
            c => c,
        };
        let lower = folded.to_lowercase().next().unwrap_or(folded);
        if lower.is_alphanumeric() || is_cjk_char(lower) {
            out.push(lower);
            last_was_space = false;
        } else if !last_was_space {
            out.push(' ');
            last_was_space = true;
        }
    }
    out.trim().to_string()
}

/// Returns true if `c` is a CJK character (Han ideograph, Japanese kana, or Korean hangul).
pub fn is_cjk_char(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}') || is_kana_or_hangul(c)
}

/// Returns true if `c` is Japanese kana or Korean hangul.
fn is_kana_or_hangul(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30FF}' | '\u{AC00}'..='\u{D7AF}')
}

/// Returns `true` if the text contains any Chinese/Japanese/Korean character.
pub fn contains_cjk(text: &str) -> bool {
    text.chars().any(is_cjk_char)
}

/// Whether `text` has Japanese kana or Korean hangul. Their Han characters are Japanese or
/// Korean ones, which must not be converted to Simplified Chinese.
pub fn has_kana_or_hangul(text: &str) -> bool {
    text.chars().any(is_kana_or_hangul)
}

/// `text` without the bracketed groups whose contents `drop` picks, with spaces tidied up.
/// Groups may nest; a bracket that is never closed runs to the end of the text.
pub fn strip_brackets(text: &str, mut drop: impl FnMut(&str) -> bool) -> String {
    // Each piece is a run of text outside brackets, or one whole bracketed group
    let mut pieces: Vec<(&str, bool)> = Vec::new();
    let mut depth = 0usize;
    let mut piece_start = 0;
    for (i, c) in text.char_indices() {
        if OPENING_BRACKETS.contains(&c) {
            if depth == 0 {
                pieces.push((&text[piece_start..i], false));
                piece_start = i;
            }
            depth += 1;
        } else if CLOSING_BRACKETS.contains(&c) && depth > 0 {
            depth -= 1;
            if depth == 0 {
                let end = i + c.len_utf8();
                pieces.push((&text[piece_start..end], true));
                piece_start = end;
            }
        }
    }
    pieces.push((&text[piece_start..], depth > 0));

    let mut kept = String::with_capacity(text.len());
    for (piece, is_group) in pieces {
        if !(is_group && drop(group_contents(piece))) {
            kept.push_str(piece);
        }
    }
    let words: Vec<&str> = kept.split_whitespace().collect();
    words.join(" ")
}

/// Words that name which release a recording is. A release tag names at least one of them:
/// "(Version)" or "(Original)" alone says nothing.
const RELEASE_TAG_KINDS: &[&str] = &[
    "single",
    "album",
    "radio",
    "edit",
    "remaster",
    "remastered",
    "mono",
    "stereo",
    "lp",
    "explicit",
    "clean",
    "bonus",
];
/// The other words a release tag may have, besides years: "Single Version", "Digitally
/// Remastered".
const RELEASE_TAG_FILLERS: &[&str] = &["version", "digitally", "digital", "track", "original"];

/// Whether `tag` only says which release of a recording this is, such as "Single Version",
/// "Radio Edit" or "2011 Remaster", rather than naming a different performance ("Live",
/// "Acoustic Version", "Remix") or a featured artist.
fn is_release_tag(tag: &str) -> bool {
    let normalized = normalize_text(tag);
    let words: Vec<&str> = normalized
        .split(' ')
        .filter(|word| !(word.len() == 4 && word.bytes().all(|byte| byte.is_ascii_digit())))
        .collect();
    let is_kind = |word: &&str| RELEASE_TAG_KINDS.contains(word);
    words
        .iter()
        .all(|word| is_kind(word) || RELEASE_TAG_FILLERS.contains(word))
        && words.iter().any(is_kind)
}

/// `title` without the tags that only say which release it is: bracketed ones such as
/// "1-2-3 (Single Version)" and trailing ones such as "Song - Remastered 2011".
pub fn strip_release_tags(title: &str) -> String {
    let stripped = strip_brackets(title, is_release_tag);
    let head = TAG_DASHES
        .iter()
        .filter_map(|dash| stripped.rsplit_once(dash))
        .find(|(head, tail)| !head.trim().is_empty() && is_release_tag(tail))
        .map(|(head, _)| head.trim().to_string());
    match head {
        Some(head) => head,
        None if stripped.is_empty() => title.trim().to_string(),
        None => stripped,
    }
}

/// `title` without a trailing "Demo" that is not in brackets, as in "奇妙能力歌Demo" or
/// "Song - Demo": the head, or `None` when there is no such word. "Demo" alone is a title, and
/// the end of a longer Latin word is not the word.
fn without_trailing_demo(title: &str) -> Option<&str> {
    let title = title.trim_end();
    let split = title.char_indices().rev().nth(3)?.0;
    let (head, tail) = title.split_at(split);
    if !tail.eq_ignore_ascii_case("demo") || head.chars().next_back()?.is_ascii_alphanumeric() {
        return None;
    }
    let head = head.trim_end_matches([' ', '-', '–', '_']).trim_end();
    (!head.is_empty()).then_some(head)
}

/// `title` without extras: without any bracketed groups (`(...)`, `[...]`, `（...）`,
/// `【...】`, etc.), without trailing release tags such as `" - Remastered 2011"`, and
/// without a trailing "Demo". Used for display, song comparison, and bare catalogue search.
pub fn strip_display_title(title: &str) -> String {
    let without_brackets = strip_brackets(title, |_| true);
    let cleaned = strip_release_tags(&without_brackets);
    let cleaned = match without_trailing_demo(&cleaned) {
        Some(head) => head.to_string(),
        None => cleaned,
    };
    if cleaned.is_empty() {
        title.trim().to_string()
    } else {
        cleaned
    }
}

/// `text` with catalogue numbers written the usual way, for display only:
/// - `Opus` is written `Op.`, when a number follows (`Opus 11` → `Op. 11`). A title such as
///   "Magnum Opus" has no number after it and stays as it is.
/// - A letter's dot stuck to a number gets a space (`No.1` → `No. 1`, `Op.11` → `Op. 11`).
///   A dot after a digit is left alone, so `2.5` stays one number.
///
/// Comparing and searching use the names as the sources spell them, never this form.
pub fn tidy_numbering(text: &str) -> String {
    static OPUS_REGEX: OnceLock<Regex> = OnceLock::new();
    static DOT_REGEX: OnceLock<Regex> = OnceLock::new();
    // `posth.` is "Opus posthumous", numbered by its own list (`Op. posth. 66`)
    let opus_regex = OPUS_REGEX
        .get_or_init(|| Regex::new(r"(?i)\bopus\b\.?(\s*(?:\d|posth))").expect("a valid pattern"));
    let dot_regex =
        DOT_REGEX.get_or_init(|| Regex::new(r"([A-Za-z])\.(\d)").expect("a valid pattern"));
    let text = opus_regex.replace_all(text, "Op.$1");
    dot_regex.replace_all(&text, "$1. $2").into_owned()
}

/// `text` with a key's flat and sharp signs spelled out, for display only: `E♭ major` →
/// `E-flat major`, `C♯ minor` → `C-sharp minor`. Only a sign right after a note letter (A to G)
/// standing on its own is a key's; any other ♭ or ♯ stays.
///
/// Comparing and searching use the names as the sources spell them, never this form.
pub fn spell_out_flats_and_sharps(text: &str) -> String {
    static SIGN_REGEX: OnceLock<Regex> = OnceLock::new();
    let sign_regex =
        SIGN_REGEX.get_or_init(|| Regex::new(r"\b([A-Ga-g])([♭♯])").expect("a valid pattern"));
    sign_regex
        .replace_all(text, |caps: &regex::Captures| {
            let word = if &caps[2] == "♭" { "flat" } else { "sharp" };
            format!("{}-{word}", &caps[1])
        })
        .into_owned()
}

/// Words that mark a backing track with no singing, rather than the sung recording.
const INSTRUMENTAL_KEYWORDS: &[&str] = &[
    "instrumental",
    "karaoke",
    "off vocal",
    "伴奏",
    "纯音乐",
    "纯音版",
    "演奏版",
    "演奏曲",
    "消音",
];

/// Whether `text` contains a word marking a backing track without singing (such as `"伴奏"`,
/// `"纯音乐"`, `"演奏版"`, `"instrumental"`, `"karaoke"`, `"off vocal"`, or `"消音"`).
pub fn has_instrumental_keyword(text: &str) -> bool {
    let normalized = normalize_text(text);
    INSTRUMENTAL_KEYWORDS
        .iter()
        .any(|keyword| normalized.contains(keyword))
}

/// Which kind of performance a title's tags name. Two kinds are two recordings with their own
/// lyric timing, whereas release tags ("Single Version", "2011 Remaster") name the same one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    /// No tag, or only release tags and extras such as "(feat. …)".
    Plain,
    Live,
    /// A remix, DJ version, extended mix or other rearrangement.
    Remix,
    /// A backing track without the singing.
    Instrumental,
    /// A demo recording, which is not the released one even when its length is close.
    Demo,
    /// Another performance: acoustic, a cover, another language.
    Other,
}

/// The kind of performance `title` names in its bracketed tags or after a trailing dash,
/// such as "敢爱敢做 (Live)", "记事本 (DJ版)" or "Song - Live at Wembley".
pub fn version_of(title: &str) -> Version {
    let mut tags = Vec::new();
    let outside = strip_brackets(title, |group| {
        tags.push(group.to_string());
        true
    });
    if without_trailing_demo(&outside).is_some() {
        tags.push("demo".to_string());
    }
    let tail = TAG_DASHES
        .iter()
        .find_map(|dash| outside.rsplit_once(dash))
        .map(|(_, tail)| tail.to_string());
    tags.extend(tail);

    let kind = |tag: &String| {
        if is_release_tag(tag) {
            return Version::Plain;
        }
        let tag = normalize_text(tag);
        let has = |needles: &[&str]| needles.iter().any(|needle| tag.contains(needle));
        let word = |words: &[&str]| tag.split(' ').any(|tag_word| words.contains(&tag_word));
        if has(INSTRUMENTAL_KEYWORDS) || word(&["inst"]) {
            Version::Instrumental
        } else if word(&["demo"]) || has(&["小样"]) {
            Version::Demo
        } else if word(&["live", "concert", "unplugged"]) || has(&["现场", "演唱会", "音乐会"])
        {
            Version::Live
        } else if has(&["mix", "dj", "混音", "舞曲", "慢摇"]) || word(&["extended", "dub"]) {
            Version::Remix
        } else if word(&["acoustic", "cover", "ver", "version"]) || has(&["版"]) {
            Version::Other
        } else {
            Version::Plain
        }
    };
    // The most telling tag decides: "(Live) (Instrumental)" is an instrumental
    let rank = |version: &Version| match version {
        Version::Instrumental => 5,
        Version::Demo => 4,
        Version::Live => 3,
        Version::Remix => 2,
        Version::Other => 1,
        Version::Plain => 0,
    };
    tags.iter()
        .map(kind)
        .max_by_key(rank)
        .unwrap_or(Version::Plain)
}

/// The text inside a bracketed group, without its own opening and closing bracket.
fn group_contents(group: &str) -> &str {
    let mut chars = group.chars();
    chars.next();
    let inner = chars.as_str();
    inner.strip_suffix(CLOSING_BRACKETS).unwrap_or(inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_of_reads_the_kind_of_performance_from_the_tags() {
        assert_eq!(version_of("奉献"), Version::Plain);
        assert_eq!(version_of("1-2-3 (Single Version)"), Version::Plain);
        assert_eq!(version_of("Song - Remastered 2011"), Version::Plain);
        assert_eq!(version_of("奉献 (《夜市人生》电视剧插曲)"), Version::Plain);
        assert_eq!(
            version_of("Don't Stop the Party (feat. TJR)"),
            Version::Plain
        );
        // A word in the title itself is not a tag
        assert_eq!(version_of("Live and Let Die"), Version::Plain);
        assert_eq!(version_of("敢爱敢做 (Live)"), Version::Live);
        assert_eq!(
            version_of("无心睡眠 (Live in Hong Kong / 1989-1990)"),
            Version::Live
        );
        assert_eq!(version_of("对的人 (2007超级星光大道现场)"), Version::Live);
        assert_eq!(version_of("Song - Live at Wembley"), Version::Live);
        assert_eq!(version_of("记事本(Remix)"), Version::Remix);
        assert_eq!(version_of("记事本 (DJ版)"), Version::Remix);
        assert_eq!(version_of("无心睡眠 (Whoo-Oh-O Mix)"), Version::Remix);
        assert_eq!(version_of("I'm Good (Blue) [Extended]"), Version::Remix);
        assert_eq!(version_of("无心睡眠 (Instrumental)"), Version::Instrumental);
        assert_eq!(version_of("海阔天空 (伴奏)"), Version::Instrumental);
        assert_eq!(version_of("奉献 (纯音乐)"), Version::Instrumental);
        assert_eq!(version_of("奉献 (纯音版)"), Version::Instrumental);
        assert_eq!(
            version_of("冷雨夜 (雨夜之后(演奏版))"),
            Version::Instrumental
        );
        assert_eq!(version_of("Song (Inst.)"), Version::Instrumental);
        assert_eq!(version_of("Instant Crush"), Version::Plain);
        assert_eq!(version_of("Song (Acoustic Version)"), Version::Other);
        assert_eq!(version_of("断点 (国语版)"), Version::Other);
        // The unbracketed "Demo" of a catalogue upload, which is not part of a word
        assert_eq!(version_of("奇妙能力歌Demo"), Version::Demo);
        assert_eq!(version_of("奇妙能力歌 (Demo)"), Version::Demo);
        assert_eq!(version_of("Song - Demo"), Version::Demo);
        assert_eq!(version_of("Pandemo"), Version::Plain);
        assert_eq!(version_of("Demo"), Version::Plain);
    }

    #[test]
    fn test_tidy_numbering_spaces_numbers_and_shortens_opus() {
        // A dot stuck to its number gets a space
        assert_eq!(tidy_numbering("Symphony No.4"), "Symphony No. 4");
        assert_eq!(
            tidy_numbering("Sonata K.331, Vol.2"),
            "Sonata K. 331, Vol. 2"
        );
        assert_eq!(tidy_numbering("Symphony No. 4"), "Symphony No. 4");
        // Not an abbreviation's dot: a decimal number, a title's own dots
        assert_eq!(tidy_numbering("Take 2.5"), "Take 2.5");
        assert_eq!(tidy_numbering("v1.2.3"), "v1.2.3");
        assert_eq!(tidy_numbering("1-2-3"), "1-2-3");

        // Opus becomes Op., in any case, with or without its own dot
        assert_eq!(
            tidy_numbering("Piano Concerto Opus 11"),
            "Piano Concerto Op. 11"
        );
        assert_eq!(tidy_numbering("sonata opus 27 no.2"), "sonata Op. 27 no. 2");
        assert_eq!(tidy_numbering("Etude OPUS.10"), "Etude Op. 10");
        assert_eq!(tidy_numbering("Opus.11"), "Op. 11");
        assert_eq!(
            tidy_numbering("Fantaisie-Impromptu, Opus posth. 66"),
            "Fantaisie-Impromptu, Op. posth. 66"
        );

        // Opus is only the catalogue number's word when a number follows it
        assert_eq!(tidy_numbering("Magnum Opus"), "Magnum Opus");
        assert_eq!(tidy_numbering("Opus Pocus"), "Opus Pocus");
        assert_eq!(tidy_numbering("Opuses 3"), "Opuses 3");

        // Writing it twice changes nothing more
        let once = tidy_numbering("Concerto No.1 in E minor Opus 11");
        assert_eq!(once, "Concerto No. 1 in E minor Op. 11");
        assert_eq!(tidy_numbering(&once), once);
    }

    #[test]
    fn test_spell_out_flats_and_sharps_writes_a_keys_sign_as_a_word() {
        assert_eq!(
            spell_out_flats_and_sharps("Symphony No. 3 in E♭ major"),
            "Symphony No. 3 in E-flat major"
        );
        assert_eq!(
            spell_out_flats_and_sharps("Concerto in B♭, Op. 83"),
            "Concerto in B-flat, Op. 83"
        );
        assert_eq!(
            spell_out_flats_and_sharps("Prelude in d♭ minor"),
            "Prelude in d-flat minor"
        );
        assert_eq!(spell_out_flats_and_sharps("(A♭)"), "(A-flat)");
        assert_eq!(
            spell_out_flats_and_sharps("Sonata No. 14 in C♯ minor"),
            "Sonata No. 14 in C-sharp minor"
        );
        assert_eq!(
            spell_out_flats_and_sharps("Prelude in f♯"),
            "Prelude in f-sharp"
        );
        // Not a key: a sign after a longer word, or standing alone
        assert_eq!(spell_out_flats_and_sharps("Love♭"), "Love♭");
        assert_eq!(spell_out_flats_and_sharps("♭ song"), "♭ song");
        assert_eq!(spell_out_flats_and_sharps("Song♯"), "Song♯");
        // Writing it twice changes nothing more
        assert_eq!(spell_out_flats_and_sharps("E-flat major"), "E-flat major");
        assert_eq!(spell_out_flats_and_sharps("C-sharp minor"), "C-sharp minor");
    }

    #[test]
    fn test_display_title_leaves_out_a_trailing_demo() {
        assert_eq!(strip_display_title("奇妙能力歌Demo"), "奇妙能力歌");
        assert_eq!(strip_display_title("奇妙能力歌 (Demo)"), "奇妙能力歌");
        assert_eq!(strip_display_title("Song - demo"), "Song");
        assert_eq!(strip_display_title("Song Demo (Live)"), "Song");
        assert_eq!(strip_display_title("Song - Remastered 2011 (Live)"), "Song");
        // Not a tag: the title itself, or the end of a word
        assert_eq!(strip_display_title("Demo"), "Demo");
        assert_eq!(strip_display_title("Pandemo"), "Pandemo");
        assert_eq!(strip_display_title("Demolition"), "Demolition");
    }

    #[test]
    fn test_strip_brackets_drops_the_groups_it_is_asked_to() {
        let all = |_: &str| true;
        assert_eq!(strip_brackets("Song (Live) [2011 Remaster]", all), "Song");
        assert_eq!(strip_brackets("Song (Live (Tokyo)) tail", all), "Song tail");
        assert_eq!(strip_brackets("晴天（Live）【官方】", all), "晴天");
        assert_eq!(strip_brackets("Song (unclosed", all), "Song");
        assert_eq!(strip_brackets("Stray ) bracket", all), "Stray ) bracket");

        let live_only = |inner: &str| inner.eq_ignore_ascii_case("live");
        assert_eq!(
            strip_brackets("Song (feat. X) (Live)", live_only),
            "Song (feat. X)"
        );
    }

    #[test]
    fn test_strip_release_tags_keeps_what_names_a_different_performance() {
        assert_eq!(strip_release_tags("1-2-3 (Single Version)"), "1-2-3");
        assert_eq!(strip_release_tags("1-2-3 - Single Version"), "1-2-3");
        assert_eq!(strip_release_tags("Song [2011 Remaster]"), "Song");
        assert_eq!(strip_release_tags("Song - Remastered 2009"), "Song");
        assert_eq!(
            strip_release_tags("Song (Radio Edit) (feat. X)"),
            "Song (feat. X)"
        );
        assert_eq!(strip_release_tags("Song (Digitally Remastered)"), "Song");

        for kept in [
            "Song (Live)",
            "Song (Acoustic Version)",
            "Song (Single Remix)",
            "Song (English Version)",
            "Song (Original)",
            "Song - Live at Wembley",
            "1-2-3",
            "(Single Version)",
        ] {
            assert_eq!(strip_release_tags(kept), kept);
        }
    }

    #[test]
    fn test_kana_and_hangul_mark_japanese_and_korean_text() {
        assert!(has_kana_or_hangul("夜に駆ける"));
        assert!(has_kana_or_hangul("사랑"));
        assert!(!has_kana_or_hangul("周杰倫 晴天"));
        assert!(!has_kana_or_hangul("Shape of You"));
    }
}
