//! Parses ICY stream metadata ("now playing" radio tags) into normalized track names.

use std::borrow::Cow;
use std::str;
use std::sync::OnceLock;

use regex::Regex;

use crate::text;
use crate::track::TrackName;

/// Repairs double-encoded UTF-8 strings misinterpreted as Latin-1 (`U+0080..=U+00FF`).
fn repair_latin1_utf8(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut run_start = 0usize;

    for (index, character) in input.char_indices() {
        if !matches!(character, '\u{0080}'..='\u{00ff}') {
            out.push_str(&decode_latin1_slice(&input[run_start..index]));
            out.push(character);
            run_start = index + character.len_utf8();
        }
    }
    out.push_str(&decode_latin1_slice(&input[run_start..]));
    without_control_characters(&out)
}

fn decode_latin1_slice(slice: &str) -> Cow<'_, str> {
    if slice.is_empty() {
        return Cow::Borrowed(slice);
    }
    let bytes: Vec<u8> = slice.chars().map(|c| c as u8).collect();
    match str::from_utf8(&bytes) {
        Ok(valid) => Cow::Owned(valid.to_owned()),
        Err(_) => Cow::Borrowed(slice),
    }
}

fn without_control_characters(text: &str) -> String {
    let visible: String = text.chars().filter(|c| !c.is_control()).collect();
    let words: Vec<&str> = visible.split_whitespace().collect();
    words.join(" ")
}

/// Strips radio broadcast noise (bitrate tags, `[Radio Edit]`, `Now Playing:`, etc.)
/// and extracts the song's title and artist from an ICY metadata string.
/// Returns `None` if the ICY string is empty or looks like a station bumper/ad rather than a song.
pub fn parse_icy_title(raw_icy: &str, station_name: &str) -> Option<TrackName> {
    static PREFIX_REGEX: OnceLock<Regex> = OnceLock::new();
    static BRACKET_NOISE_REGEX: OnceLock<Regex> = OnceLock::new();
    static BOOK_TITLE_REGEX: OnceLock<Regex> = OnceLock::new();
    static BUMPER_REGEX: OnceLock<Regex> = OnceLock::new();

    // Repaired, it is already trimmed, with its spaces tidied up
    let repaired = repair_latin1_utf8(raw_icy);
    if repaired.len() < 2 {
        return None;
    }

    let bumper_regex = BUMPER_REGEX.get_or_init(|| {
        Regex::new(
            r"(?i)^(welcome to|listen live|commercial|advertisement|station id|tuning in|https?://|www\.|广告|电台|广播|正在播出|欢迎收听)",
        )
        .expect("a valid pattern")
    });
    if bumper_regex.is_match(&repaired) {
        return None;
    }

    let normalized_title = text::normalize_text(&repaired);
    let normalized_station = text::normalize_text(station_name);
    if !normalized_station.is_empty() && normalized_title == normalized_station {
        return None;
    }

    let prefix_regex = PREFIX_REGEX.get_or_init(|| {
        Regex::new(r"(?i)^(?:now playing|playing|on air|当前播放|正在播放)\s*[:：\-]\s*")
            .expect("a valid pattern")
    });
    let without_prefix = prefix_regex.replace(&repaired, "");

    // Bracketed broadcast extras; others, such as "(feat. …)" or "(Remix)", are part of the name
    let noise_regex = BRACKET_NOISE_REGEX.get_or_init(|| {
        Regex::new(
            r"(?i)radio edit|clean|explicit|official|audio|video|mv|hd|hq|remaster|version|live|kbps|\d{4}",
        )
        .expect("a valid pattern")
    });
    let cleaned = text::strip_brackets(&without_prefix, |inner| noise_regex.is_match(inner));
    if cleaned.is_empty() {
        return None;
    }

    // Remove trailing "| Station Name" if present
    let main_part = if let Some((left, right)) = cleaned.rsplit_once(" | ") {
        if !normalized_station.is_empty()
            && text::normalize_text(right).contains(&normalized_station)
        {
            left.trim()
        } else {
            cleaned.as_str()
        }
    } else {
        cleaned.as_str()
    };

    // Case 1: Chinese book-title brackets, e.g. "周深《大鱼》" or "《大鱼》- 周深"
    let book_regex = BOOK_TITLE_REGEX
        .get_or_init(|| Regex::new(r"^(.*?)《([^》]+)》(.*)$").expect("a valid pattern"));
    if let Some(captures) = book_regex.captures(main_part) {
        let title = captures[2].trim().to_string();
        let is_separator = |c: char| c.is_whitespace() || "-–—:：/".contains(c);
        let before = captures[1].trim_matches(is_separator);
        let after = captures[3].trim_matches(is_separator);
        let artist = if !before.is_empty() {
            before.to_string()
        } else {
            after.to_string()
        };
        if !title.is_empty() {
            return Some(TrackName::new(&title, &artist));
        }
    }

    // Case 2: Standard separator ("Artist - Title", "Artist — Title", "Artist / Title").
    // Bare "-" without spaces only splits strings with CJK text or spaces (such as "周杰伦-晴天"),
    // avoiding splitting single hyphenated titles like "Anti-Hero" or "1-2-3".
    for separator in [" - ", " – ", " — ", " / ", "-"] {
        if separator == "-"
            && !text::contains_cjk(main_part)
            && !main_part.chars().any(char::is_whitespace)
        {
            continue;
        }
        if let Some((left, right)) = main_part.split_once(separator) {
            let left = left.trim().trim_matches('"').trim();
            let right = right.trim().trim_matches('"').trim();
            if !left.is_empty() && !right.is_empty() {
                // Reject if either side just repeats the station name
                if !normalized_station.is_empty()
                    && (text::normalize_text(left) == normalized_station
                        || text::normalize_text(right) == normalized_station)
                {
                    return None;
                }
                return Some(TrackName::new(right, left));
            }
        }
    }

    // Case 3: Single unseparated title (only accept if it doesn't look like a generic slogan)
    if main_part.chars().count() <= 48 && !main_part.to_lowercase().contains("radio") {
        return Some(TrackName::new(main_part, ""));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repair_latin1_mojibake_and_parse_icy() {
        // UTF-8 bytes for "邝美云 - 雪花" misinterpreted as Latin-1
        let mojibake = "é\u{82}\u{9d}ç¾\u{8e}äº\u{91} - é\u{9b}ªè\u{8a}±";
        let parsed = parse_icy_title(mojibake, "AsiaFM").expect("should parse ICY track");
        assert_eq!(parsed.artist, "邝美云");
        assert_eq!(parsed.title, "雪花");

        // Noisy Western radio ICY string
        let western = "Now Playing: Taylor Swift - Cruel Summer [Radio Edit] (128kbps) | Hit FM";
        let parsed_western = parse_icy_title(western, "Hit FM").expect("should parse western ICY");
        assert_eq!(parsed_western.artist, "Taylor Swift");
        assert_eq!(parsed_western.title, "Cruel Summer");

        // Station bumper should be rejected
        assert!(parse_icy_title("Welcome to SomaFM Groove Salad", "SomaFM Groove Salad").is_none());
        assert!(parse_icy_title("SomaFM Groove Salad", "SomaFM Groove Salad").is_none());

        // Single hyphenated English title stays a title; CJK without spaces still splits
        let hyphenated =
            parse_icy_title("Anti-Hero", "Hit FM").expect("should keep hyphenated title");
        assert_eq!(hyphenated.artist, "");
        assert_eq!(hyphenated.title, "Anti-Hero");
        let compact_cjk =
            parse_icy_title("周杰伦-晴天", "Hit FM").expect("should split CJK hyphen");
        assert_eq!(compact_cjk.artist, "周杰伦");
        assert_eq!(compact_cjk.title, "晴天");
    }
}
