//! Ranking and scoring algorithms for lyrics catalog search results.

use std::future::Future;

use crate::embedding::{self, MIN_CANDIDATE_SCORE};
use crate::lyrics::{LyricsMatch, Release};
use crate::text;
use crate::track::TrackName;

/// Checks if downloaded lyrics are in a Western script when the song title is CJK
/// (e.g. English covers or adaptations filed under a Chinese title).
pub(super) fn wrong_script(song: &TrackName, matched: &LyricsMatch) -> bool {
    text::contains_cjk(&song.title)
        && matched
            .lyrics
            .as_ref()
            .is_some_and(|lyrics| !lyrics.is_cjk())
}

/// Minimum embedding similarity threshold for same-script artist match.
const MIN_ARTIST_SIMILARITY: f32 = 0.25;

fn similar_names(a: &str, b: &str) -> bool {
    let similarity = embedding::cosine_similarity(&embedding::embed(a), &embedding::embed(b));
    similarity >= MIN_ARTIST_SIMILARITY
}

/// Whether a catalogue entry's artist could be the song's. Names in different scripts
/// (e.g. "Jay Chou" and "周杰伦") can't be compared, so then the title has to carry the match.
fn plausible_artist(song: &TrackName, candidate: &str) -> bool {
    if song.artist.is_empty()
        || candidate.trim().is_empty()
        || text::contains_cjk(&song.artist) != text::contains_cjk(candidate)
    {
        return true;
    }
    // The title check covers ICY titles broadcast as "Title - Artist", which parse swapped
    similar_names(&song.artist, candidate) || similar_names(&song.title, candidate)
}

/// A catalogue search hit.
#[derive(Debug, PartialEq)]
pub(super) struct Candidate<T> {
    /// The entry's name.
    pub(super) name: TrackName,
    /// How long its recording is (seconds), when the catalogue says.
    pub(super) length: Option<f64>,
    /// What its client needs to download the entry's lyrics.
    pub(super) entry: T,
}

/// A hit that scored well enough to be downloaded.
struct Scored<T> {
    /// Whether the entry is another release than the one playing.
    other_release: bool,
    score: f32,
    candidate: Candidate<T>,
}

/// How many of the best-ranked hits a catalogue client tries to download lyrics for.
const MAX_DOWNLOADS: usize = 3;

/// Scores hits against the song, drops weak matches (`< MIN_CANDIDATE_SCORE`) and entries by
/// a different artist (a cover or an unrelated song sharing the title), and returns the
/// `MAX_DOWNLOADS` best of the rest, best first. Entries for another release than `release`
/// come after all the others, whatever their score.
fn rank<T>(
    song: &TrackName,
    release: Release,
    hits: impl IntoIterator<Item = Candidate<T>>,
) -> Vec<Candidate<T>> {
    let mut scored: Vec<Scored<T>> = hits
        .into_iter()
        .filter(|candidate| plausible_artist(song, &candidate.name.artist))
        .filter_map(|candidate| {
            let name = &candidate.name;
            let score =
                embedding::score_candidate(&song.title, &song.artist, &name.title, &name.artist);
            let other_release = !release.is(&name.title, candidate.length);
            (score >= MIN_CANDIDATE_SCORE).then_some(Scored {
                other_release,
                score,
                candidate,
            })
        })
        .collect();
    scored.sort_by(|a, b| {
        a.other_release
            .cmp(&b.other_release)
            .then(b.score.total_cmp(&a.score))
    });
    scored
        .into_iter()
        .take(MAX_DOWNLOADS)
        .map(|scored| scored.candidate)
        .collect()
}

/// Ranks a catalogue's `hits` for `song` (playing as `release`) and returns the first match
/// that `download` makes of the best of them, trying the next hit whenever one yields no
/// usable lyrics or non-CJK lyrics for a CJK-titled song.
pub(super) async fn first_match<T, F, Fut>(
    song: &TrackName,
    release: Release,
    hits: impl IntoIterator<Item = Candidate<T>>,
    mut download: F,
) -> Option<LyricsMatch>
where
    F: FnMut(TrackName, T) -> Fut,
    Fut: Future<Output = Option<LyricsMatch>>,
{
    let mut fallback = None;
    for candidate in rank(song, release, hits) {
        if let Some(found) = download(candidate.name, candidate.entry).await {
            let matched = LyricsMatch {
                length: candidate.length,
                ..found
            };
            if !wrong_script(song, &matched) {
                return Some(matched);
            }
            fallback.get_or_insert(matched);
        }
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::parse;

    fn candidate<T>(name: TrackName, length: Option<f64>, entry: T) -> Candidate<T> {
        Candidate {
            name,
            length,
            entry,
        }
    }

    #[test]
    fn test_candidates_by_a_different_artist_are_rejected() {
        let plausible =
            |title, artist, candidate| plausible_artist(&TrackName::new(title, artist), candidate);
        assert!(!plausible("Lofi", "Lofi Girl", "siick"));
        assert!(!plausible(
            "Merry Christmas Mr. Lawrence",
            "Ryuichi Sakamoto",
            "echo"
        ));
        // (An artist with a known Chinese name, like "Hebe Tien", arrives as 田馥甄)
        assert!(!plausible("小幸运", "Hebe", "i"));
        assert!(plausible(
            "Perfect",
            "Ed Sheeran",
            "Ed Sheeran, Justin Bieber"
        ));
        assert!(plausible("晴天", "周杰倫", "周杰伦"));
        // Names in different scripts can't be compared
        assert!(plausible("晴天", "Jay", "周杰伦"));

        let lofi_cover = candidate(TrackName::new("Lofi", "siick"), None, ());
        let any = Release::new("", None);
        assert!(rank(&TrackName::new("Lofi", "Lofi Girl"), any, [lofi_cover]).is_empty());
        // ICY broadcast as "Title - Artist" parses with the fields swapped
        let swapped = candidate(TrackName::new("大鱼", "周深"), None, ());
        assert_eq!(
            rank(&TrackName::new("周深", "大鱼"), any, [swapped]).len(),
            1
        );
    }

    #[test]
    fn test_entries_for_the_release_that_is_playing_rank_first() {
        // QQ Music's hits for Gloria Estefan's "1-2-3", as Shazam (asked for Chinese names)
        // reports it: the album version (3:28) is the closest name, but the single (3:36) plays
        let song = TrackName::new("1-2-3", "葛洛丽亚·伊斯特芬");
        let hits = || {
            [
                candidate(
                    TrackName::new("1-2-3", "Gloria Estefan, Miami Sound Machine"),
                    Some(208.0),
                    "album",
                ),
                candidate(
                    TrackName::new("1-2-3", "Gloria Estefan"),
                    Some(215.0),
                    "single",
                ),
                candidate(TrackName::new("1-2-3", "Gloria Estefan"), None, "unknown"),
            ]
        };
        let order = |length| -> Vec<&str> {
            rank(&song, Release::new(&song.title, length), hits())
                .into_iter()
                .map(|candidate| candidate.entry)
                .collect()
        };
        assert_eq!(order(Some(215.9)), ["single", "unknown", "album"]);
        assert_eq!(order(Some(208.3))[0], "album");
    }

    #[test]
    fn test_entries_for_another_performance_rank_last_when_lengths_are_unknown() {
        // When no recording length is known, the tags have to tell the studio cut from the rest
        let song = TrackName::new("敢爱敢做", "林子祥");
        let hits = [
            candidate(TrackName::new("敢爱敢做 (Live)", "林子祥"), None, "live"),
            candidate(TrackName::new("敢爱敢做 (DJ慢摇版)", "林子祥"), None, "dj"),
            candidate(TrackName::new("敢爱敢做", "林子祥"), None, "studio"),
        ];
        let ranked = rank(&song, Release::new(&song.title, None), hits);
        assert_eq!(ranked[0].entry, "studio");
    }

    #[tokio::test]
    async fn test_cjk_titled_song_prefers_cjk_lyrics_over_english_version() {
        let song = TrackName::new("月光爱人", "李玟");
        let english = "\
            [00:10.00]If the sky opened up for me\n\
            [00:20.00]And the mountains disappeared\n\
            [00:30.00]If the seas can dry turned to dust\n\
            [00:40.00]And the sun refused to rise\n\
            [00:50.00]I would still find my way\n\
            [01:00.00]By the light I see in your eyes\n";
        let chinese = "\
            [00:10.00]我醒来 睡在月光里\n\
            [00:20.00]下弦月 让我想起你\n\
            [00:30.00]不想醒过来 谁明白\n\
            [00:40.00]怕睁开眼睛 你不在\n\
            [00:50.00]爱人心 深入海\n\
            [01:00.00]带我去 把它找回来\n";

        let hits = [
            candidate(TrackName::new("月光爱人", "李玟"), Some(218.0), english),
            candidate(TrackName::new("月光爱人", "李玟"), Some(218.0), chinese),
        ];
        let release = Release::new(&song.title, Some(218.0));
        let matched = first_match(&song, release, hits, |name, text| async move {
            Some(LyricsMatch {
                name,
                lyrics: parse::parse_lrc(text),
                length: None,
                late_by: None,
            })
        })
        .await
        .unwrap();
        assert!(matched.lyrics.unwrap().is_cjk());
    }
}
