//! Concurrent search orchestrator across all supported lyrics catalogs.

use reqwest::Client;

use super::{kugou, lrclib, netease, qq, ranking};
use crate::lyrics::{LyricsMatch, Release};
use crate::task::Task;
use crate::track::TrackName;

/// Evaluation penalties for candidate lyrics matches.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Shortcomings {
    wrong_script: bool,
    other_release: bool,
}

/// A catalogue match paired with the provider that supplied it.
pub(in crate::lyrics) struct CatalogueHit {
    pub(in crate::lyrics) provider: &'static str,
    pub(in crate::lyrics) matched: LyricsMatch,
}

/// Queries all lyrics catalogs concurrently and returns the best match according to provider priority:
/// QQ Music -> Kugou -> NetEase -> LRCLIB. Cancels remaining background tasks once an optimal match is found.
pub(in crate::lyrics) async fn search(
    client: &Client,
    song: &TrackName,
    release: Release,
) -> Option<CatalogueHit> {
    let searches = [
        (
            "QQ Music",
            Task::spawn(qq::search(client.clone(), song.clone(), release)),
        ),
        (
            "Kugou",
            Task::spawn(kugou::search(client.clone(), song.clone(), release)),
        ),
        (
            "NetEase",
            Task::spawn(netease::search(client.clone(), song.clone(), release)),
        ),
        (
            "LRCLIB",
            Task::spawn(lrclib::search(client.clone(), song.clone(), release)),
        ),
    ];
    let mut fallback: Option<(Shortcomings, CatalogueHit)> = None;
    for (provider, search) in searches {
        if let Some(matched) = search.join().await {
            let shortcomings = Shortcomings {
                wrong_script: ranking::wrong_script(song, &matched),
                other_release: !matched.is_release(release),
            };
            let hit = CatalogueHit { provider, matched };
            if shortcomings == Shortcomings::default() {
                return Some(hit);
            }
            if fallback
                .as_ref()
                .is_none_or(|(best, _)| shortcomings < *best)
            {
                fallback = Some((shortcomings, hit));
            }
        }
    }
    fallback.map(|(_, hit)| hit)
}
