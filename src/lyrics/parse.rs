//! Lyrics file parsers for `.krc` (Kugou word-timed) and standard `.lrc` formats.

use std::sync::OnceLock;

use flate2::read::ZlibDecoder;
use regex::Regex;

use super::{LyricLine, LyricWord, Lyrics};

const CREDIT_KEYWORDS: &[&str] = &[
    "作词",
    "作曲",
    "编曲",
    "制作人",
    "混音",
    "母带",
    "录音",
    "吉他",
    "贝斯",
    "鼓手",
    "键盘",
    "和声",
    "弦乐",
    "统筹",
    "企划",
    "监制",
    "出品",
    "发行",
    "版权",
    "原唱",
    "翻唱",
    "arranger",
    "producer",
    "mixing",
    "mastering",
    "director",
    "engineer",
    "composer",
    "lyricist",
    "publisher",
];

const KRC_XOR_KEY: [u8; 16] = [
    0x40, 0x47, 0x61, 0x77, 0x5e, 0x32, 0x74, 0x47, 0x51, 0x36, 0x31, 0x2d, 0xce, 0xd2, 0x6e, 0x69,
];

fn is_credit_line(line: &str) -> bool {
    static METADATA_HEADER_REGEX: OnceLock<Regex> = OnceLock::new();
    static CATALOG_ID_REGEX: OnceLock<Regex> = OnceLock::new();
    static SHORT_CREDIT_REGEX: OnceLock<Regex> = OnceLock::new();
    static PUBLISHER_TAG_REGEX: OnceLock<Regex> = OnceLock::new();

    let metadata_regex = METADATA_HEADER_REGEX
        .get_or_init(|| Regex::new(r"^\[[a-zA-Z]{2,}:.*\]$").expect("a valid pattern"));
    let catalog_regex = CATALOG_ID_REGEX
        .get_or_init(|| Regex::new(r"^[A-Z]{2}-[A-Z0-9]{2,}-[0-9-]+$").expect("a valid pattern"));
    let short_credit_regex = SHORT_CREDIT_REGEX
        .get_or_init(|| Regex::new(r"^(?:词|曲)\s*[:：]").expect("a valid pattern"));
    let publisher_tag_regex = PUBLISHER_TAG_REGEX
        .get_or_init(|| Regex::new(r"(?i)\b(?:op|sp)\s*[:：]").expect("a valid pattern"));

    if metadata_regex.is_match(line)
        || catalog_regex.is_match(line)
        || short_credit_regex.is_match(line)
        || publisher_tag_regex.is_match(line)
    {
        return true;
    }

    let lower = line.to_lowercase();
    CREDIT_KEYWORDS
        .iter()
        .any(|keyword| lower.contains(keyword))
}

/// Decodes a Kugou `.krc` binary payload (4-byte `krc1` magic + XOR + zlib).
pub(super) fn decode_krc(data: &[u8]) -> Option<String> {
    use std::io::Read;

    if data.len() < 5 {
        return None;
    }
    let xored: Vec<u8> = data[4..]
        .iter()
        .enumerate()
        .map(|(i, &byte)| byte ^ KRC_XOR_KEY[i % KRC_XOR_KEY.len()])
        .collect();

    let mut decoder = ZlibDecoder::new(&xored[..]);
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).ok()?;
    Some(String::from_utf8_lossy(&decoded).into_owned())
}

/// Parses decoded Kugou `.krc` text into word-timed `Lyrics`.
pub(super) fn parse_krc(text: &str) -> Option<Lyrics> {
    static KRC_LINE_REGEX: OnceLock<Regex> = OnceLock::new();
    static KRC_WORD_REGEX: OnceLock<Regex> = OnceLock::new();

    let line_regex = KRC_LINE_REGEX
        .get_or_init(|| Regex::new(r"^\[(\d+),(\d+)\](.*)$").expect("a valid pattern"));
    let word_regex = KRC_WORD_REGEX
        .get_or_init(|| Regex::new(r"<(\d+),(\d+),\d+>([^<]*)").expect("a valid pattern"));

    let mut lines: Vec<LyricLine> = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim_start_matches('\u{feff}').trim_end();
        let Some(captures) = line_regex.captures(line) else {
            continue;
        };

        let start_milliseconds: u64 = captures[1].parse().unwrap_or(0);
        let body = &captures[3];
        let mut words = Vec::new();

        for word_captures in word_regex.captures_iter(body) {
            let word_text = &word_captures[3];
            if word_text.is_empty() {
                continue;
            }
            let word_offset: u64 = word_captures[1].parse().unwrap_or(0);
            let word_duration: u64 = word_captures[2].parse().unwrap_or(0);
            words.push(LyricWord {
                start: (start_milliseconds + word_offset) as f64 / 1000.0,
                end: (start_milliseconds + word_offset + word_duration) as f64 / 1000.0,
                text: word_text.to_string(),
            });
        }

        // Trim leading and trailing whitespace from boundary words so word character offsets stay
        // aligned with the trimmed plain line text.
        while let Some(first) = words.first_mut() {
            let trimmed = first.text.trim_start();
            if trimmed.is_empty() {
                words.remove(0);
            } else {
                if trimmed.len() != first.text.len() {
                    first.text = trimmed.to_string();
                }
                break;
            }
        }
        while let Some(last) = words.last_mut() {
            let trimmed = last.text.trim_end();
            if trimmed.is_empty() {
                words.pop();
            } else {
                if trimmed.len() != last.text.len() {
                    last.text = trimmed.to_string();
                }
                break;
            }
        }

        let plain = if words.is_empty() {
            body.trim().to_string()
        } else {
            words.iter().map(|word| word.text.as_str()).collect()
        };

        if plain.is_empty() || is_credit_line(&plain) {
            continue;
        }

        lines.push(LyricLine {
            time: start_milliseconds as f64 / 1000.0,
            text: plain,
            words: if words.is_empty() { None } else { Some(words) },
        });
    }

    Lyrics::from_lines(lines)
}

/// Parses `.lrc` text (including multi-timestamp chorus tags) into synchronized `Lyrics`.
pub(super) fn parse_lrc(lrc_text: &str) -> Option<Lyrics> {
    static LRC_TAG_REGEX: OnceLock<Regex> = OnceLock::new();
    let tag_regex = LRC_TAG_REGEX.get_or_init(|| {
        Regex::new(r"\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?\]").expect("a valid pattern")
    });

    // One line for each timestamp of each line of text, in the order of the text
    let mut lines: Vec<LyricLine> = Vec::new();
    for raw_line in lrc_text.lines() {
        let stripped = raw_line.trim();
        let all_captures: Vec<_> = tag_regex.captures_iter(stripped).collect();
        if all_captures.is_empty() {
            continue;
        }

        let text = tag_regex.replace_all(stripped, "").trim().to_string();
        if text.is_empty() || is_credit_line(&text) {
            continue;
        }

        let single_tag = all_captures.len() == 1;
        for captures in all_captures {
            let minutes: u64 = captures[1].parse().unwrap_or(0);
            let seconds: u64 = captures[2].parse().unwrap_or(0);
            let milliseconds: u64 = if let Some(fraction) = captures.get(3) {
                let padded = format!("{:0<3}", fraction.as_str());
                padded[..3].parse().unwrap_or(0)
            } else {
                0
            };
            let timestamp = (minutes * 60 + seconds) as f64 + (milliseconds as f64 / 1000.0);

            // Skip paired same-timestamp translation lines so the primary sung line is preserved
            if single_tag
                && lines
                    .last()
                    .is_some_and(|previous| (previous.time - timestamp).abs() < 1e-6)
            {
                continue;
            }

            lines.push(LyricLine {
                time: timestamp,
                text: text.clone(),
                words: None,
            });
        }
    }
    Lyrics::from_lines(lines)
}

#[cfg(test)]
mod tests {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;

    use super::*;

    fn encode_krc(text: &str) -> Vec<u8> {
        use std::io::Write;

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(text.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut encoded = Vec::with_capacity(4 + compressed.len());
        encoded.extend_from_slice(b"krc1");
        for (i, byte) in compressed.iter().enumerate() {
            encoded.push(byte ^ KRC_XOR_KEY[i % KRC_XOR_KEY.len()]);
        }
        encoded
    }

    #[test]
    fn test_decode_and_parse_krc_with_words() {
        let raw_krc = "\
             [0,1000]<0,500,0>作词：测试\n\
             [2000,1000]<0,400,0>你好<400,600,0>世界\n";

        let decoded = decode_krc(&encode_krc(raw_krc)).expect("failed to decode krc");
        let lyrics = parse_krc(&decoded).expect("expected synced krc lyrics");

        assert_eq!(lyrics.lines.len(), 1);
        let line = &lyrics.lines[0];
        assert!((line.time - 2.0).abs() < 1e-6);
        assert_eq!(line.text, "你好世界");

        let words = line.words.as_ref().expect("missing words");
        assert_eq!(words.len(), 2);
        assert!((words[0].start - 2.0).abs() < 1e-6);
        assert!((words[0].end - 2.4).abs() < 1e-6);
        assert!((words[1].start - 2.4).abs() < 1e-6);
        assert!((words[1].end - 3.0).abs() < 1e-6);
    }

    #[test]
    fn test_parse_lrc_timestamps_chorus_and_translations() {
        let raw_lrc = "\
            [00:00.64]词：杨立德\n\
            [00:01.00]作曲 : 某人\n\
            [00:01.25]曲 : 翁孝良\n\
            [00:01.50]OP : Warner Chappell\n\
            [00:01.75]SP : Sony Music\n\
            [00:05.50]First line\n\
            [00:05.50]第一行\n\
            [00:08.00]Stop: wait a minute\n\
            [00:10][00:20.00]Repeated chorus\n";

        let lyrics = parse_lrc(raw_lrc).expect("expected synced lrc lyrics");
        assert_eq!(lyrics.lines.len(), 4);
        let times: Vec<f64> = lyrics.lines.iter().map(|line| line.time).collect();
        assert_eq!(times, [5.5, 8.0, 10.0, 20.0]);
        assert_eq!(lyrics.lines[0].text, "First line");
        assert_eq!(lyrics.lines[1].text, "Stop: wait a minute");
        assert_eq!(lyrics.lines[2].text, "Repeated chorus");
        assert_eq!(lyrics.lines[3].text, "Repeated chorus");
        assert!(parse_lrc("Plain untimed text only").is_none());

        // Leading space in a KRC word is trimmed from the word too, keeping word lengths aligned
        let padded_krc = "[2000,1000]<0,400,0>  你好<400,600,0>世界 \n";
        let krc = parse_krc(padded_krc).expect("expected krc lyrics");
        assert_eq!(krc.lines[0].text, "你好世界");
        let words = krc.lines[0].words.as_ref().unwrap();
        assert_eq!(words[0].text, "你好");
        assert_eq!(words[1].text, "世界");
    }
}
