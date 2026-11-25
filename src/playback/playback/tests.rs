use std::time::Duration;

use tokio::sync::oneshot;

use super::*;
use crate::lyrics::{Found, LyricLine, LyricsMatch};
use crate::playback::schedule::{
    SHAZAM_CONFIRM, SHAZAM_NO_AUDIO_RETRY, SHAZAM_RECHECK, SHAZAM_RETRY,
};
use crate::playback::song::{FetchedLength, Lookup, LookupOutput, TimingCheck, TriedRelease};
use crate::test_util::ScratchDir;
use crate::timeline::{Alignment, Timeline};

/// A playback session on "Love Radio 103.7" that captures no audio.
fn test_playback(song: Option<Song>) -> Playback {
    let mut playback = Playback::new(
        "Love Radio 103.7".into(),
        Client::new(),
        Shifts::default(),
        TrackLengths::default(),
        Capture::silent(),
        None,
    );
    playback.song = song;
    playback
}

/// A timeline following `key` on which two answers agreed: playback is `position` seconds
/// into the song right now.
fn timeline_at(key: &str, position: f64) -> Timeline {
    let mut timeline = Timeline::new(key);
    let now = Instant::now();
    for _ in 0..2 {
        let alignment = Alignment {
            recording: key.to_string(),
            offset: position,
        };
        timeline.observe(now, &[alignment], &mut Shifts::default());
    }
    timeline
}

/// The song reported as `reported` (release "key"), which two Shazam answers have named,
/// and whose lyrics lookup is `lookup` (and has been taken in, unless it is still running).
fn test_song_named(reported: TrackName, lookup: Lookup) -> Song {
    let mut song = Song {
        tried: vec![TriedRelease {
            key: "key".into(),
            track: reported.clone(),
            apple_music_id: None,
        }],
        reported,
        lookup,
        known_length: None,
        timeline: timeline_at("key", 0.0),
        from_icy: false,
        heard: 2,
        shown_name: None,
        outro_checked: false,
        timing: TimingCheck::Waiting,
    };
    if !matches!(song.lookup, Lookup::Running(_)) {
        song.show_result();
    }
    song
}

/// "Artist - Song", whose lyrics lookup ended in `lookup`.
fn test_song(lookup: Lookup) -> Song {
    test_song_named(TrackName::new("Song", "Artist"), lookup)
}

/// A lookup that found the entry for the release that is playing.
fn found(lyrics: Option<Lyrics>) -> Lookup {
    Lookup::Found(Found {
        entry: LyricsMatch {
            name: TrackName::new("Song", "Artist"),
            lyrics,
            length: None,
            late_by: None,
        },
        in_time: true,
    })
}

/// Eight lines of English lyrics, one every 8 seconds.
fn english_lyrics() -> Option<Lyrics> {
    let lines = (1..=8)
        .map(|i| LyricLine {
            time: f64::from(i) * 8.0,
            text: format!("They tell me you're shy boy {i}"),
            words: None,
        })
        .collect();
    Lyrics::from_lines(lines)
}

/// Shazam's answer naming `title` as release `key`, heard `position` seconds into it now.
fn shazam(title: &str, key: &str, position: f64) -> SongMatch {
    SongMatch {
        track: TrackName::new(title, "Artist"),
        key: key.to_string(),
        apple_music_id: None,
        alignments: vec![Alignment {
            recording: key.to_string(),
            offset: position,
        }],
        heard_at: Instant::now(),
        exact_timing: true,
    }
}

/// Runs one pass of the main loop with `result` as Shazam's answer.
async fn answer(playback: &mut Playback, result: ShazamResult) {
    playback.shazam = Some(Task::spawn(async { result }));
    tokio::task::yield_now().await;
    playback.tick().await;
}

/// How long until the next Shazam check, in whole seconds.
fn next_check_in(playback: &Playback) -> u64 {
    let wait = playback
        .schedule
        .next_at
        .saturating_duration_since(Instant::now());
    (wait.as_secs_f64()).round() as u64
}

#[tokio::test]
async fn test_empty_or_bumper_icy_title_clears_active_song() {
    let mut playback = test_playback(Some(test_song(found(None))));

    assert!(playback.display_name().is_some());
    // Station bumper or empty ICY title arrives
    playback.on_icy_title("Love Radio 103.7");
    assert!(playback.display_name().is_none());
}

#[tokio::test]
async fn test_one_shazam_nomatch_clears_active_song() {
    let mut playback = test_playback(Some(test_song(found(None))));
    assert!(playback.display_name().is_some());

    answer(&mut playback, ShazamResult::NoMatch { retry_after: None }).await;
    assert!(playback.display_name().is_none());
    assert!(playback.lyrics_at().is_none());
}

#[tokio::test]
async fn test_no_lyrics_found_keeps_reported_artist_songname() {
    let reported = TrackName::new("Unrecognized Song", "Unknown Artist");
    let song = test_song_named(reported, Lookup::NotFound);
    let playback = test_playback(Some(song));

    assert_eq!(
        playback.display_name(),
        Some("Unknown Artist - Unrecognized Song")
    );
    assert!(playback.lyrics_at().is_none());
}

#[tokio::test]
async fn test_name_shows_cleaned_up_and_together_with_the_lyrics() {
    // Shazam's name for the song, and the catalogue entry the lookup will find
    let reported = TrackName::new("1-2-3 (Single Version)", "葛洛丽亚·伊斯特芬");
    let found = Lookup::Found(Found {
        entry: LyricsMatch {
            name: TrackName::new("1-2-3", "Gloria Estefan"),
            lyrics: english_lyrics(),
            length: Some(215.0),
            late_by: None,
        },
        in_time: true,
    });
    let (release, wait) = oneshot::channel();
    let lookup = Lookup::Running(Task::spawn(async move {
        wait.await.ok();
        LookupOutput {
            lookup: found,
            fetched_apple_length: None,
            length: Some(215.0),
            fallback_name: None,
        }
    }));
    let mut playback = test_playback(Some(test_song_named(reported, lookup)));

    // However long the lookup takes, neither row shows before it has finished
    playback.tick().await;
    assert_eq!(playback.display_name(), None);
    assert!(playback.lyrics_at().is_none());

    release.send(()).unwrap();
    tokio::task::yield_now().await;
    playback.tick().await;
    assert_eq!(playback.display_name(), Some("Gloria Estefan - 1-2-3"));
    assert!(playback.lyrics_at().is_some());
}

#[tokio::test]
async fn test_lyrics_of_another_release_stay_hidden_but_name_the_song() {
    // 苏芮's "奉献" plays as the 290-second release; only the 316-second one was found
    let other_release = Lookup::Found(Found {
        entry: LyricsMatch {
            name: TrackName::new("奉献", "苏芮"),
            lyrics: english_lyrics(),
            length: Some(316.0),
            late_by: None,
        },
        in_time: false,
    });
    let song = test_song_named(TrackName::new("奉献", "苏芮"), other_release);
    let playback = test_playback(Some(song));
    assert_eq!(playback.display_name(), Some("苏芮 - 奉献"));
    assert!(playback.lyrics_at().is_none());
}

#[tokio::test]
async fn test_a_song_takes_two_answers_to_appear() {
    let mut playback = test_playback(None);
    playback.on_shazam_match(shazam("Song", "key", 30.0));
    // The lookup has finished, but one answer alone can be a wrong guess
    let song = playback.song.as_mut().unwrap();
    song.lookup = found(english_lyrics());
    song.show_result();
    assert_eq!(playback.display_name(), None);
    assert!(playback.lyrics_at().is_none());
    assert!(playback.awaits_confirmation());

    playback.on_shazam_match(shazam("Song", "key", 30.0));
    assert_eq!(playback.display_name(), Some("Artist - Song"));
    let position = playback.lyrics_at().unwrap().position;
    assert!((position - 30.0).abs() < 0.5, "{position}");
    assert!(!playback.awaits_confirmation());
}

#[tokio::test]
async fn test_lyrics_wait_for_two_answers_to_agree_on_the_position() {
    let mut playback = test_playback(None);
    playback.on_shazam_match(shazam("Song", "key", 30.0));
    let song = playback.song.as_mut().unwrap();
    song.lookup = found(english_lyrics());
    song.show_result();
    // The second answer names the song again, but a minute further in (a repeated chorus)
    playback.on_shazam_match(shazam("Song", "key", 90.0));
    assert_eq!(playback.display_name(), Some("Artist - Song"));
    assert!(playback.lyrics_at().is_none());
    assert!(playback.awaits_confirmation());

    playback.on_shazam_match(shazam("Song", "key", 30.0));
    let position = playback.lyrics_at().unwrap().position;
    assert!((position - 30.0).abs() < 0.5, "{position}");
}

#[tokio::test]
async fn test_another_song_replaces_the_one_showing_after_two_answers_in_a_row() {
    let mut playback = test_playback(Some(test_song(found(english_lyrics()))));

    // One stray answer changes nothing
    playback.on_shazam_match(shazam("Other", "other", 5.0));
    assert_eq!(playback.display_name(), Some("Artist - Song"));
    assert!(playback.awaits_confirmation());
    playback.on_shazam_match(shazam("Song", "key", 8.0));
    assert!(playback.candidate.is_none());
    assert_eq!(playback.display_name(), Some("Artist - Song"));

    // Two in a row naming the other song make it the song
    playback.on_shazam_match(shazam("Other", "other", 5.0));
    playback.on_shazam_match(shazam("Other", "other", 5.0));
    let song = playback.song.as_ref().unwrap();
    assert_eq!(song.reported.title, "Other");
    assert!(song.is_confirmed());
    assert!(playback.candidate.is_none());
}

#[tokio::test]
async fn test_a_song_taking_over_shows_its_name_and_lyrics_on_that_frame() {
    let mut playback = test_playback(Some(test_song(found(english_lyrics()))));
    answer(
        &mut playback,
        ShazamResult::Match(shazam("Other", "other", 5.0)),
    )
    .await;

    // The candidate's lookup finishes before the second answer comes
    let candidate = playback
        .candidate
        .as_mut()
        .expect("first answer stages a candidate");
    candidate.lookup = Lookup::Running(Task::spawn(async {
        LookupOutput {
            lookup: found(english_lyrics()),
            fetched_apple_length: None,
            length: None,
            fallback_name: None,
        }
    }));
    tokio::task::yield_now().await;
    answer(
        &mut playback,
        ShazamResult::Match(shazam("Other", "other", 5.0)),
    )
    .await;

    // No frame between the two songs without a name or lyrics
    assert_eq!(playback.song.as_ref().unwrap().reported.title, "Other");
    assert!(playback.display_name().is_some());
    assert!(playback.lyrics_at().is_some());
}

#[tokio::test]
async fn test_candidate_with_disagreeing_positions_does_not_replace_the_song() {
    let mut playback = test_playback(Some(test_song(found(english_lyrics()))));

    // Two false matches naming the same wrong song at unrelated positions (90 s and 160 s)
    playback.on_shazam_match(shazam("Endless Summer", "false", 90.0));
    playback.on_shazam_match(shazam("Endless Summer", "false", 160.0));
    assert_eq!(playback.display_name(), Some("Artist - Song"));
    assert!(playback.candidate.is_some());

    // When the real song returns, the candidate is dropped without flickering the screen
    playback.on_shazam_match(shazam("Song", "key", 8.0));
    assert_eq!(playback.display_name(), Some("Artist - Song"));
    assert!(playback.candidate.is_none());

    // When a third answer DOES agree with the second position, the candidate takes over
    playback.on_shazam_match(shazam("Next Song", "next", 10.0));
    playback.on_shazam_match(shazam("Next Song", "next", 80.0));
    assert_eq!(playback.display_name(), Some("Artist - Song"));
    playback.on_shazam_match(shazam("Next Song", "next", 80.0));
    assert_eq!(playback.song.as_ref().unwrap().reported.title, "Next Song");
    assert!(playback.candidate.is_none());
}

#[tokio::test]
async fn test_guesses_that_never_agree_end_the_song_and_show_nothing() {
    // An instrumental passage, guessed at afresh on every check
    let mut playback = test_playback(Some(test_song(found(None))));
    playback.on_shazam_match(shazam("Guess A", "a", 5.0));
    playback.on_shazam_match(shazam("Guess B", "b", 50.0));
    assert_eq!(playback.display_name(), Some("Artist - Song"));
    playback.on_shazam_match(shazam("Guess C", "c", 9.0));
    assert_eq!(playback.display_name(), None);
    for (title, key) in [("Guess D", "d"), ("Guess E", "e"), ("Guess F", "f")] {
        playback.on_shazam_match(shazam(title, key, 20.0));
        let song = playback.song.as_mut().unwrap();
        song.lookup = Lookup::NotFound;
        song.show_result();
        assert_eq!(playback.display_name(), None);
    }
}

#[tokio::test]
async fn test_versions_of_the_song_showing_do_not_restart_it() {
    let mut playback = test_playback(Some(test_song(found(english_lyrics()))));
    // Shazam names a remaster and a live cut of the same song from one check to the next
    playback.on_shazam_match(shazam("Song (Remastered)", "remaster", 500.0));
    playback.on_shazam_match(shazam("Song (Live)", "live", 700.0));
    let song = playback.song.as_ref().unwrap();
    assert_eq!(
        song.tried.last().map(|tried| tried.key.as_str()),
        Some("key")
    );
    assert!(matches!(song.lookup, Lookup::Found(_)));
    // Their positions are other recordings' and leave the lyrics where they are
    let position = playback.lyrics_at().unwrap().position;
    assert!(position < 1.0, "{position}");
}

#[tokio::test]
async fn test_song_without_lyrics_is_looked_up_once_as_each_other_release() {
    // 张国荣's "无心睡眠", first named as its instrumental
    let reported = TrackName::new("无心睡眠 (Instrumental)", "张国荣");
    let mut playback = test_playback(Some(test_song_named(reported, Lookup::NotFound)));

    // Shazam keeps naming the release that was already looked up, or another key with the
    // exact same title, artist and Apple Music id
    let mut same_inputs = shazam("无心睡眠 (Instrumental)", "key-2", 30.0);
    same_inputs.track = TrackName::new("无心睡眠 (Instrumental)", "张国荣");
    playback.on_shazam_match(shazam("无心睡眠 (Instrumental)", "key", 30.0));
    playback.on_shazam_match(same_inputs);
    assert!(matches!(
        playback.song.as_ref().unwrap().lookup,
        Lookup::NotFound
    ));

    // A different Apple Music id (and thus a different release length) IS worth a lookup
    let mut other_apple = shazam("无心睡眠 (Instrumental)", "key-3", 30.0);
    other_apple.track = TrackName::new("无心睡眠 (Instrumental)", "张国荣");
    other_apple.apple_music_id = Some("999999".into());
    playback.on_shazam_match(other_apple);
    assert!(matches!(
        playback.song.as_ref().unwrap().lookup,
        Lookup::Running(_)
    ));
    playback.song.as_mut().unwrap().lookup = Lookup::NotFound;

    // The sung release is worth a lookup, and the name from the lookup before stays
    playback.on_shazam_match(shazam("无心睡眠", "sung", 30.0));
    let song = playback.song.as_mut().unwrap();
    assert!(matches!(song.lookup, Lookup::Running(_)));
    assert_eq!(
        song.tried.last().map(|tried| tried.key.as_str()),
        Some("sung")
    );
    assert_eq!(song.reported.title, "无心睡眠");
    assert_eq!(playback.display_name(), Some("张国荣 - 无心睡眠"));
    assert!(playback.lyrics_at().is_none());

    // Neither release is looked up a second time
    playback.song.as_mut().unwrap().lookup = Lookup::NotFound;
    playback.on_shazam_match(shazam("无心睡眠 (Instrumental)", "key", 30.0));
    playback.on_shazam_match(shazam("无心睡眠", "sung", 30.0));
    assert!(matches!(
        playback.song.as_ref().unwrap().lookup,
        Lookup::NotFound
    ));
}

#[tokio::test]
async fn test_icy_song_shows_its_name_at_once_and_lyrics_once_shazam_places_them() {
    let mut playback = test_playback(None);
    playback.on_icy_title("Artist - Song");
    // No catalogue lookup is started until Shazam names the release
    assert!(matches!(
        playback.song.as_ref().unwrap().lookup,
        Lookup::Idle
    ));
    let song = playback.song.as_mut().unwrap();
    song.lookup = found(english_lyrics());
    song.show_result();
    // The station's word is enough for the name, but says nothing of the position
    assert_eq!(playback.display_name(), Some("Artist - Song"));
    assert!(playback.lyrics_at().is_none());

    // Shazam's first answer tells the release, which is looked up for its own lyrics
    playback.on_shazam_match(shazam("Song", "key", 40.0));
    let song = playback.song.as_mut().unwrap();
    assert!(matches!(song.lookup, Lookup::Running(_)));
    assert_eq!(
        song.tried.last().map(|tried| tried.key.as_str()),
        Some("key")
    );
    song.lookup = found(english_lyrics());
    assert!(playback.lyrics_at().is_none());

    playback.on_shazam_match(shazam("Song", "key", 40.0));
    let position = playback.lyrics_at().unwrap().position;
    assert!((position - 40.0).abs() < 0.5, "{position}");
}

#[tokio::test]
async fn test_next_check_follows_what_shazam_answered() {
    let mut playback = test_playback(None);
    // A first answer wants a second one soon; once they agree, checks go back to normal
    answer(
        &mut playback,
        ShazamResult::Match(shazam("Song", "key", 30.0)),
    )
    .await;
    assert_eq!(next_check_in(&playback), SHAZAM_CONFIRM.as_secs());
    answer(
        &mut playback,
        ShazamResult::Match(shazam("Song", "key", 30.0)),
    )
    .await;
    assert_eq!(next_check_in(&playback), SHAZAM_RECHECK.as_secs());

    // No song: Shazam says when to ask again
    let retry_after = Some(Duration::from_secs(9));
    answer(&mut playback, ShazamResult::NoMatch { retry_after }).await;
    assert_eq!(next_check_in(&playback), 9);
    answer(&mut playback, ShazamResult::NoMatch { retry_after: None }).await;
    assert_eq!(next_check_in(&playback), SHAZAM_RETRY.as_secs());

    // Failed requests back off, and a song that is showing stays meanwhile
    playback.song = Some(test_song(found(None)));
    for wait in [3, 6, 12] {
        answer(&mut playback, ShazamResult::Error { retry_after: None }).await;
        assert_eq!(next_check_in(&playback), wait);
    }
    assert!(playback.display_name().is_some());
    let retry_after = Some(Duration::from_secs(40));
    answer(&mut playback, ShazamResult::Error { retry_after }).await;
    assert_eq!(next_check_in(&playback), 40);
}

#[tokio::test]
async fn test_answers_that_never_agree_do_not_keep_checks_quick() {
    let mut playback = test_playback(None);
    let mut waits = Vec::new();
    for i in 0..6 {
        // The same song, at a different place every time
        let matched = shazam("Song", "key", f64::from(i) * 40.0);
        answer(&mut playback, ShazamResult::Match(matched)).await;
        waits.push(next_check_in(&playback));
    }
    assert_eq!(waits, [3, 3, 3, 3, 8, 3]);
}

#[tokio::test]
async fn test_finished_lookup_settles_into_its_result() {
    let mut lookup = Lookup::Running(Task::spawn(async {
        LookupOutput {
            lookup: found(None),
            fetched_apple_length: Some(FetchedLength {
                id: "198676212".into(),
                seconds: 215.9,
            }),
            length: Some(215.9),
            fallback_name: None,
        }
    }));
    tokio::task::yield_now().await;
    let settled = lookup.settle().await.expect("should settle once");
    assert_eq!(settled.length, Some(215.9));
    assert!(matches!(lookup, Lookup::Found(_)));
    // Only once
    assert!(lookup.settle().await.is_none());
}

#[tokio::test]
async fn test_settled_lookup_remembers_fetched_apple_music_length() {
    let lookup = Lookup::Running(Task::spawn(async {
        LookupOutput {
            lookup: found(None),
            fetched_apple_length: Some(FetchedLength {
                id: "198676212".into(),
                seconds: 215.9,
            }),
            length: Some(215.9),
            fallback_name: None,
        }
    }));
    let mut playback = test_playback(Some(test_song(lookup)));
    tokio::task::yield_now().await;
    playback.tick().await;
    assert_eq!(playback.track_lengths.get("198676212"), Some(215.9));
    assert_eq!(playback.song.as_ref().unwrap().known_length, Some(215.9));
}

#[tokio::test]
async fn test_english_fallback_replaces_transliterated_artist_when_no_catalogue_matches() {
    let reported = TrackName::new("Heart of God", "新颂敬拜");
    let lookup = Lookup::Running(Task::spawn(async {
        LookupOutput {
            lookup: Lookup::NotFound,
            fetched_apple_length: None,
            length: None,
            fallback_name: Some(TrackName::new("Heart of God", "Hillsong Worship")),
        }
    }));
    let mut playback = test_playback(Some(test_song_named(reported, lookup)));
    tokio::task::yield_now().await;
    playback.tick().await;
    assert_eq!(
        playback.display_name(),
        Some("Hillsong Worship - Heart of God")
    );
}

#[tokio::test]
async fn test_song_clears_past_its_length_only_while_shazam_is_failing() {
    let mut song = test_song(found(english_lyrics()));
    song.known_length = Some(180.0);
    // Playback is 190 s in (past 180 + 5 s grace)
    song.timeline = timeline_at("key", 190.0);
    let mut playback = test_playback(Some(song));
    playback.schedule.next_at = Instant::now() + Duration::from_secs(8);

    // While Shazam is healthy, normal checks decide when the song ends
    playback.tick().await;
    assert!(playback.display_name().is_some());

    // While paused, a failing Shazam never clears the frozen frame
    playback.set_paused(true);
    answer(&mut playback, ShazamResult::Error { retry_after: None }).await;
    assert!(playback.display_name().is_some());

    // Once unpaused and Shazam is backing off, the finished song clears
    playback.set_paused(false);
    playback.tick().await;
    assert!(playback.display_name().is_none());
    assert!(playback.lyrics_at().is_none());
}

#[tokio::test]
async fn test_song_without_length_clears_well_past_last_lyric_while_shazam_fails() {
    let mut song = test_song(found(english_lyrics()));
    // Last line is at 64 s; playback is 130 s in (past 64 + 60 s fallback outro)
    song.timeline = timeline_at("key", 130.0);
    let mut playback = test_playback(Some(song));
    playback.schedule.next_at = Instant::now() + Duration::from_secs(8);

    answer(&mut playback, ShazamResult::Error { retry_after: None }).await;
    assert!(playback.display_name().is_none());
    assert!(playback.lyrics_at().is_none());
}

#[tokio::test]
async fn test_idle_false_does_not_unfreeze_paused_playback() {
    let mut playback = test_playback(None);
    playback.set_paused(true);
    assert!(playback.is_paused());
    assert!(playback.is_idle());

    playback.set_idle(false);
    assert!(playback.is_paused());
    assert!(playback.is_idle());
}

#[tokio::test]
async fn test_empty_icy_title_clears_station_track_without_disturbing_shazam_only_song() {
    // Classic FM style: station names a piece, then sends empty StreamTitle during an ad break
    let mut classic = test_playback(None);
    classic.on_icy_title(CLASSIC_FM_TITLE);
    assert_eq!(classic.display_name(), Some(CLASSIC_FM_SHOWN));
    classic.on_icy_title("");
    assert_eq!(classic.display_name(), None);
    assert!(classic.song.is_none());

    // Love Radio style: station never names songs; an empty StreamTitle on reconnect keeps Shazam's song
    let mut shazam_only = test_playback(Some(test_song(found(english_lyrics()))));
    shazam_only.on_icy_title("");
    assert_eq!(shazam_only.display_name(), Some("Artist - Song"));
    assert!(shazam_only.lyrics_at().is_some());
}

#[tokio::test]
async fn test_playback_pause_state() {
    let mut playback = test_playback(None);

    assert!(!playback.is_paused());
    playback.set_paused(true);
    assert!(playback.is_paused());
    playback.set_paused(false);
    assert!(!playback.is_paused());
}

#[tokio::test]
async fn test_lyrics_ended_advances_shazam_check() {
    let lines = [10.0, 20.0]
        .into_iter()
        .map(|time| LyricLine {
            time,
            text: format!("Line at {time}"),
            words: None,
        })
        .collect();
    let mut song = test_song(found(Lyrics::from_lines(lines)));
    // Song position is 30.0s (past the last lyric timestamp 20.0 + 4.0 outro)
    song.timeline = timeline_at("key", 30.0);
    let mut playback = test_playback(Some(song));
    // Next check originally scheduled 8s out
    playback.schedule.next_at = Instant::now() + Duration::from_secs(8);

    playback.tick().await;
    // Shazam was requested immediately because the lyrics ended
    assert!(playback.shazam.is_some());
}

/// A song whose lyrics ended long ago (last line at 20 s, playback at 30 s).
fn song_past_its_lyrics() -> Song {
    let lines = [10.0, 20.0]
        .into_iter()
        .map(|time| LyricLine {
            time,
            text: format!("Line at {time}"),
            words: None,
        })
        .collect();
    let mut song = test_song(found(Lyrics::from_lines(lines)));
    song.timeline = timeline_at("key", 30.0);
    song
}

#[tokio::test]
async fn test_lyrics_ended_checks_once_and_then_keeps_the_usual_pace() {
    let mut playback = test_playback(Some(song_past_its_lyrics()));
    playback.schedule.next_at = Instant::now() + Duration::from_secs(8);

    playback.tick().await;
    assert!(
        playback.shazam.is_some(),
        "the end of the lyrics asks at once"
    );

    // Same song still playing: return to the normal recheck interval rather than polling rapidly
    answer(
        &mut playback,
        ShazamResult::Match(shazam("Song", "key", 30.0)),
    )
    .await;
    assert_eq!(next_check_in(&playback), SHAZAM_RECHECK.as_secs());
    assert!(playback.shazam.is_none());
}

#[tokio::test]
async fn test_lyrics_ended_does_not_cut_a_backoff_short() {
    let mut playback = test_playback(Some(song_past_its_lyrics()));
    playback.schedule.next_at = Instant::now() + Duration::from_secs(8);
    playback.tick().await;
    assert!(playback.shazam.is_some());

    // Refused requests back off, 3 s and then 6 s, instead of being repeated at once
    for wait in [3, 6] {
        answer(&mut playback, ShazamResult::Error { retry_after: None }).await;
        assert_eq!(next_check_in(&playback), wait);
        assert!(
            playback.shazam.is_none(),
            "asked again before the {wait} s were up"
        );
    }
}

#[tokio::test]
async fn test_a_new_station_title_waits_for_a_running_backoff() {
    let mut playback = test_playback(None);
    answer(&mut playback, ShazamResult::Error { retry_after: None }).await;
    answer(&mut playback, ShazamResult::Error { retry_after: None }).await;
    assert_eq!(next_check_in(&playback), 6);

    // A new station title shows at once, but does not cut the running backoff short
    playback.on_icy_title("Artist - Song");
    assert_eq!(playback.display_name(), Some("Artist - Song"));
    assert_eq!(next_check_in(&playback), 6);
    playback.tick().await;
    assert!(playback.shazam.is_none());

    // Without a backoff, a new station title asks Shazam right away
    for _ in 0..2 {
        answer(
            &mut playback,
            ShazamResult::Match(shazam("Song", "key", 30.0)),
        )
        .await;
    }
    assert_eq!(next_check_in(&playback), SHAZAM_RECHECK.as_secs());
    playback.on_icy_title("Someone - Another Song");
    playback.tick().await;
    assert!(playback.shazam.is_some());
}

#[tokio::test]
async fn test_no_audio_yet_is_not_a_failed_request() {
    let mut playback = test_playback(None);
    answer(&mut playback, ShazamResult::Error { retry_after: None }).await;
    assert_eq!(playback.schedule.failures, 1);

    // Nothing was sent to Shazam, so the backoff does not grow; the capture is looked at
    // again soon
    answer(&mut playback, ShazamResult::NoAudio).await;
    assert_eq!(playback.schedule.failures, 1);
    assert_eq!(next_check_in(&playback), SHAZAM_NO_AUDIO_RETRY.as_secs());

    // A stream that is slow to start leaves no failures behind however long it takes
    let mut playback = test_playback(None);
    for _ in 0..6 {
        answer(&mut playback, ShazamResult::NoAudio).await;
    }
    assert_eq!(playback.schedule.failures, 0);
    assert_eq!(next_check_in(&playback), SHAZAM_NO_AUDIO_RETRY.as_secs());
}

const CLASSIC_FM_TITLE: &str =
    "Johann Sebastian Bach, Giuliano Carmignola - Concerto in D minor for 2 Violins (2)";
const CLASSIC_FM_SHOWN: &str =
    "Johann Sebastian Bach, Giuliano Carmignola - Concerto in D minor for 2 Violins";

#[tokio::test]
async fn test_station_name_shows_at_once_and_not_the_catalogues() {
    let mut playback = test_playback(None);
    playback.on_icy_title(CLASSIC_FM_TITLE);
    // The name needs no cleaning up, so it does not wait for the lookup
    assert_eq!(playback.display_name(), Some(CLASSIC_FM_SHOWN));

    // The catalogues only know another piece by Bach: the name stays the station's
    let song = playback.song.as_mut().unwrap();
    song.lookup = Lookup::Found(Found {
        entry: LyricsMatch {
            name: TrackName::new(
                "Brandenburg Concerto No. 3:Allegro",
                "Johann Sebastian Bach",
            ),
            lyrics: None,
            length: Some(398.0),
            late_by: None,
        },
        in_time: true,
    });
    song.show_result();
    assert_eq!(playback.display_name(), Some(CLASSIC_FM_SHOWN));
}

#[tokio::test]
async fn test_lyrics_follow_shazam_while_the_station_name_stays() {
    let mut playback = test_playback(None);
    playback.on_icy_title("Artist - Sunny Day");

    // Shazam knows the song under another title, so it becomes the song the lyrics are
    // looked up and timed for (as it did before station names were kept)
    playback.on_shazam_match(shazam("晴天", "qing", 30.0));
    playback.on_shazam_match(shazam("晴天", "qing", 30.0));
    let song = playback.song.as_mut().unwrap();
    assert_eq!(
        song.tried.last().map(|tried| tried.key.as_str()),
        Some("qing")
    );
    song.lookup = found(english_lyrics());
    let position = playback.lyrics_at().unwrap().position;
    assert!((position - 30.0).abs() < 0.5, "{position}");
    assert_eq!(playback.display_name(), Some("Artist - Sunny Day"));

    // The station repeating its title (the stream reconnected) does not undo that
    playback.on_icy_title("Artist - Sunny Day");
    assert_eq!(
        playback
            .song
            .as_ref()
            .unwrap()
            .tried
            .last()
            .map(|tried| tried.key.as_str()),
        Some("qing")
    );
    assert!(playback.lyrics_at().is_some());
}

#[tokio::test]
async fn test_no_match_clears_the_lyrics_but_not_the_station_name() {
    let mut playback = test_playback(None);
    playback.on_icy_title(CLASSIC_FM_TITLE);
    answer(&mut playback, ShazamResult::NoMatch { retry_after: None }).await;

    // Shazam rarely knows classical music; the station still does
    assert!(playback.song.is_none());
    assert!(playback.lyrics_at().is_none());
    assert_eq!(playback.display_name(), Some(CLASSIC_FM_SHOWN));
}

#[tokio::test]
async fn test_the_station_naming_another_song_or_none_changes_the_name() {
    let mut playback = test_playback(None);
    playback.on_icy_title("Artist - Song");
    playback.on_icy_title("Someone - Another Song");
    assert_eq!(playback.display_name(), Some("Someone - Another Song"));
    assert_eq!(
        playback.song.as_ref().unwrap().reported.title,
        "Another Song"
    );

    // A bumper (the station's own name) means no song
    playback.on_icy_title("Love Radio 103.7");
    assert_eq!(playback.display_name(), None);
    assert!(playback.song.is_none());
}

#[tokio::test]
async fn test_a_station_title_for_the_song_already_known_leaves_its_lyrics_alone() {
    let mut playback = test_playback(Some(test_song(found(english_lyrics()))));
    assert_eq!(playback.display_name(), Some("Artist - Song"));

    // Shazam named the song first; the station spells the artist its own way
    playback.on_icy_title("Another Artist - Song");
    assert_eq!(playback.display_name(), Some("Another Artist - Song"));
    assert!(playback.lyrics_at().is_some());
    assert!(matches!(
        playback.song.as_ref().unwrap().lookup,
        Lookup::Found(_)
    ));
}

/// A song now 80 s in, whose lyrics are `english_lyrics` (a line every 8 s from 8 s to 64 s),
/// and the instant each position in it was audible.
fn song_80_seconds_in(lookup: Lookup) -> (Song, impl Fn(f64) -> Instant) {
    let mut song = test_song(lookup);
    song.timeline = timeline_at("key", 80.0);
    let now = Instant::now();
    (song, move |position: f64| {
        now - Duration::from_secs_f64(80.0 - position)
    })
}

/// The entry the song's lookup found.
fn entry(song: &Song) -> &LyricsMatch {
    match &song.lookup {
        Lookup::Found(found) => &found.entry,
        _ => panic!("no entry"),
    }
}

#[test]
fn test_singing_heard_ahead_of_every_line_moves_the_lyrics_earlier() {
    let scratch = ScratchDir::create();
    let cache = LyricsCache::new(scratch.path().join("lyrics"));
    let (mut song, at) = song_80_seconds_in(found(english_lyrics()));
    // Listening from 2 s into the song
    song.hear_singing(&[], at(2.0), Some(&cache));
    assert!(matches!(song.timing, TimingCheck::Listening(_)));

    // Every line is timed 1.5 s after its singing starts
    let starts: Vec<Instant> = (1..=8).map(|i| at(f64::from(i) * 8.0 - 1.5)).collect();
    song.hear_singing(&starts, at(70.0), Some(&cache));
    let late_by = entry(&song).late_by.expect("measured");
    assert!((late_by - 1.5).abs() < 0.01, "{late_by}");
    assert!(matches!(song.timing, TimingCheck::Done));
    // The line timed at 16 s shows 14.5 s in, when its singing starts
    let shown = song.lyrics_position(at(14.5)).unwrap();
    assert!((shown - 16.0).abs() < 0.01, "{shown}");

    // Saved, so the next time the song plays its lyrics are in time from the start
    let saved = cache
        .load(&song.reported)
        .into_iter()
        .next()
        .expect("saved");
    let saved_late_by = saved.late_by.expect("saved late_by");
    assert!(
        (saved_late_by - late_by).abs() < 1e-6,
        "{saved_late_by} vs {late_by}"
    );
}

#[test]
fn test_singing_that_does_not_line_up_leaves_the_lyrics_as_they_are() {
    let (mut song, at) = song_80_seconds_in(found(english_lyrics()));
    song.hear_singing(&[], at(2.0), None);
    let offsets = [-2.2, 1.9, -0.4, 2.4, -1.6, 0.9, -2.4, 1.3];
    let starts: Vec<Instant> = (1..=8)
        .zip(offsets)
        .map(|(i, offset)| at(f64::from(i) * 8.0 + offset))
        .collect();
    song.hear_singing(&starts, at(67.0), None);
    assert!(matches!(song.timing, TimingCheck::Listening(_)));

    // Once the lyrics have ended, the check gives up
    song.hear_singing(&[], at(68.0), None);
    assert!(matches!(song.timing, TimingCheck::Done));
    assert_eq!(entry(&song).late_by, None);
    let shown = song.lyrics_position(at(16.0)).unwrap();
    assert!((shown - 16.0).abs() < 0.01, "{shown}");
}

#[test]
fn test_a_timing_measured_before_is_used_and_not_measured_again() {
    let mut lookup = found(english_lyrics());
    if let Lookup::Found(found) = &mut lookup {
        found.entry.late_by = Some(-0.5);
    }
    let (mut song, at) = song_80_seconds_in(lookup);
    song.hear_singing(&[], at(2.0), None);
    assert!(matches!(song.timing, TimingCheck::Done));
    // Lines timed early are shown later
    let shown = song.lyrics_position(at(16.0)).unwrap();
    assert!((shown - 15.5).abs() < 0.01, "{shown}");
}
