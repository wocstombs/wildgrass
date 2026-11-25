//! Song release version classification and track duration comparison.

use crate::text::{self, Version};

/// Duration tolerance (in seconds) for considering two recordings the same release.
pub(super) const LENGTH_TOLERANCE: f64 = 2.0;

/// Identifies the specific musical release of a track (version type and duration).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Release {
    /// Track duration in seconds if known.
    pub length: Option<f64>,
    /// Release performance type (original, live, remix, acoustic, instrumental).
    pub version: Version,
}

impl Release {
    /// Creates a release definition for `title` with duration `length`.
    pub fn new(title: &str, length: Option<f64>) -> Self {
        Self {
            length,
            version: text::version_of(title),
        }
    }

    /// Checks if a candidate entry with `title` and `length` matches this release.
    pub fn is(&self, title: &str, length: Option<f64>) -> bool {
        let version = text::version_of(title);
        // A demo or an instrumental is another recording whatever its length: 陈粒's
        // "奇妙能力歌Demo" is 2 seconds longer than the released song, and an instrumental
        // backing track such as Beyond's "冷雨夜 (雨夜之后(演奏版))" has the same length as
        // the sung song.
        if (version == Version::Demo) != (self.version == Version::Demo)
            || (version == Version::Instrumental) != (self.version == Version::Instrumental)
        {
            return false;
        }
        match (length, self.length) {
            (Some(entry), Some(song)) => (entry - song).abs() <= LENGTH_TOLERANCE,
            _ => version == self.version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_title_picks_the_release_only_when_lengths_cannot() {
        // 陈慧琳's remix, 342.8 seconds long: the DJ cut is 2.4 seconds longer, QQ Music's
        // entry for the remix (whole seconds) 0.8 shorter
        let remix = Release::new("记事本(Remix)", Some(342.83));
        assert!(!remix.is("记事本(DJ版)", Some(345.26)));
        assert!(remix.is("记事本 (Remix)", Some(342.0)));
        assert!(!remix.is("记事本", Some(251.0)));

        // 林子祥's live recordings differ in length; without lengths, only the kind is known
        let live = Release::new("敢爱敢做 (Live)", Some(381.2));
        assert!(live.is("敢爱敢做 (Live)", Some(381.0)));
        assert!(!live.is("敢爱敢做 (Live)", Some(254.0)));
        let studio = Release::new("敢爱敢做", None);
        assert!(!studio.is("敢爱敢做 (Live)", Some(381.0)));
        assert!(studio.is("敢爱敢做", Some(299.0)));
    }

    #[test]
    fn test_a_demo_is_not_the_released_song_even_at_the_same_length() {
        // 陈粒's "奇妙能力歌": the released song is 253 seconds long, the demo upload 255
        let released = Release::new("奇妙能力歌", Some(253.2));
        assert!(!released.is("奇妙能力歌Demo", Some(255.0)));
        assert!(!released.is("奇妙能力歌 (Demo)", Some(253.0)));
        assert!(released.is("奇妙能力歌", Some(253.0)));
        // Heard as the demo, the demo is the release
        let demo = Release::new("奇妙能力歌 (Demo)", Some(253.2));
        assert!(demo.is("奇妙能力歌Demo", Some(253.0)));
        assert!(!demo.is("奇妙能力歌", Some(253.0)));
    }

    #[test]
    fn test_an_instrumental_is_not_the_sung_song_even_at_the_same_length() {
        // Beyond's "冷雨夜 (雨夜之后(演奏版))" (301.6s) is an instrumental bumper sharing the
        // length of the sung track "冷雨夜" (302.0s)
        let instrumental = Release::new("冷雨夜 (雨夜之后(演奏版))", Some(301.6));
        assert!(!instrumental.is("冷雨夜", Some(302.0)));
        assert!(instrumental.is("冷雨夜 (演奏版)", Some(302.0)));

        let sung = Release::new("冷雨夜", Some(302.0));
        assert!(!sung.is("冷雨夜 (雨夜之后(演奏版))", Some(301.6)));
        assert!(!sung.is("冷雨夜 (伴奏)", Some(302.0)));
        assert!(sung.is("冷雨夜", Some(302.0)));
    }
}
