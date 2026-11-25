//! Synchronized lyrics data structures with line and word-level timing.

use serde::{Deserialize, Serialize};

use crate::{chinese, text};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricWord {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricLine {
    pub time: f64,
    pub text: String,
    pub words: Option<Vec<LyricWord>>,
}

/// Synchronized lyrics container with chronologically ordered lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
}

impl Lyrics {
    /// Constructs a `Lyrics` instance from lines, sorting by timestamp.
    /// Returns `None` if no lines have non-zero timestamps.
    pub fn from_lines(mut lines: Vec<LyricLine>) -> Option<Self> {
        if !lines.iter().any(|line| line.time > 0.0) {
            return None;
        }
        // Stable sort preserves original ordering for lines sharing timestamps
        lines.sort_by(|a, b| a.time.total_cmp(&b.time));
        Some(Self { lines })
    }

    pub(super) fn simplified(mut self) -> Self {
        for line in &mut self.lines {
            line.text = chinese::to_simplified(&line.text);
            if let Some(words) = &mut line.words {
                for word in words {
                    word.text = chinese::to_simplified(&word.text);
                }
            }
        }
        self
    }

    /// Whether the song is sung in Chinese, Japanese or Korean: at least a third of its lines
    /// are written in those scripts. (Some catalogues add a line or two in Chinese, such as
    /// credits, to the lyrics of Western songs.)
    pub fn is_cjk(&self) -> bool {
        let cjk_lines = self
            .lines
            .iter()
            .filter(|line| text::contains_cjk(&line.text))
            .count();
        cjk_lines * 3 >= self.lines.len()
    }

    /// The lyrics without the "Title - Artist" line some catalogues put at 0:00,
    /// such as QQ Music's "1-2-3 - Gloria Estefan (格洛丽娅·艾斯特芬)" for the song `title`.
    pub(super) fn without_title_line(mut self, title: &str) -> Self {
        let title = text::normalize_text(title);
        let is_title_line = |line: &LyricLine| {
            line.time < 0.5
                && line.text.contains(" - ")
                && text::normalize_text(&line.text).starts_with(&title)
        };
        if !title.is_empty() && self.lines.first().is_some_and(is_title_line) {
            self.lines.remove(0);
        }
        self
    }

    /// When the last line starts, in seconds into the song.
    pub fn last_line_time(&self) -> f64 {
        self.lines.last().map_or(0.0, |line| line.time)
    }

    /// Returns true if the lyrics appear to be an incomplete teaser, clip, or ringtone
    /// (e.g. lasting less than 45 seconds or having fewer than 6 lines).
    pub fn is_snippet(&self) -> bool {
        self.lines.len() < 6 || self.last_line_time() < 45.0
    }

    /// Binary search (`O(log n)`) for the last line whose timestamp is `<= position`.
    pub fn index_at(&self, position: f64) -> Option<usize> {
        let index = self.lines.partition_point(|line| line.time <= position);
        index.checked_sub(1)
    }

    /// Computes how many Unicode characters (`char`s) of `lines[line_index].text`
    /// have been sung at `position` seconds.
    pub fn sung_characters(&self, line_index: usize, position: f64) -> usize {
        let Some(line) = self.lines.get(line_index) else {
            return 0;
        };
        let total_characters = line.text.chars().count();
        let Some(words) = &line.words else {
            return total_characters;
        };

        let mut count = 0usize;
        for word in words {
            let word_characters = word.text.chars().count();
            if position >= word.end {
                count += word_characters;
            } else if position > word.start {
                let duration = (word.end - word.start).max(0.001);
                let progress = (position - word.start) / duration;
                let partial = ((word_characters as f64) * progress).round() as usize;
                count += partial.max(1);
                break;
            } else {
                break;
            }
        }
        count.min(total_characters)
    }
}

#[cfg(test)]
mod tests {
    use crate::lyrics::parse;
    use crate::test_util;

    #[test]
    fn test_binary_search_and_cjk_word_progress() {
        let raw_krc = "\
            [1000,1000]<0,500,0>你好<500,500,0>世界\n\
            [3000,1000]<0,1000,0>再见\n";
        let lyrics = parse::parse_krc(raw_krc).expect("expected synced krc lyrics");

        assert_eq!(lyrics.index_at(0.5), None);
        assert_eq!(lyrics.index_at(1.25), Some(0));
        assert_eq!(lyrics.sung_characters(0, 1.25), 1);
        assert_eq!(lyrics.sung_characters(0, 1.5), 2);
        assert_eq!(lyrics.sung_characters(0, 2.2), 4);
        assert_eq!(lyrics.index_at(3.1), Some(1));
    }

    #[test]
    fn test_lyrics_in_cjk_scripts_mark_a_cjk_song() {
        let lyrics = |lines: &[&str]| {
            let lrc: String = lines
                .iter()
                .enumerate()
                .map(|(i, line)| format!("[00:{:02}.00]{line}\n", (i + 1) * 8))
                .collect();
            parse::parse_lrc(&lrc).unwrap()
        };
        let english = [
            "They tell me you're shy boy",
            "But I want you just the same",
        ];
        assert!(!lyrics(&english.repeat(4)).is_cjk());
        // One line of Chinese credits among English ones
        let mut credited = english.repeat(4);
        credited.push("翻译：某人");
        assert!(!lyrics(&credited).is_cjk());
        assert!(lyrics(&["故事的小黄花", "从出生那年就飘着"].repeat(4)).is_cjk());
        assert!(lyrics(&["夜に駆ける", "沈むように溶けてゆくように"].repeat(4)).is_cjk());
    }

    #[test]
    fn test_lyrics_is_snippet() {
        let short_lrc = "[00:01.00]Line 1\n[00:05.00]Line 2\n[00:10.00]Line 3\n[00:15.00]Line 4\n";
        let parsed = parse::parse_lrc(short_lrc).unwrap();
        assert!(parsed.is_snippet());

        let full_parsed = parse::parse_lrc(&test_util::lrc_lines(10, "Line")).unwrap();
        assert!(!full_parsed.is_snippet());
    }
}
