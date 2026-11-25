//! Online lyrics catalog clients (QQ Music, Kugou, NetEase, LRCLIB) and ranking algorithms.
//!
//! Each client parses search results into candidates, scores candidates against song metadata,
//! and downloads the best matching lyrics via `ranking::first_match`.

mod kugou;
mod lrclib;
mod netease;
mod qq;
mod ranking;
mod request;
mod search;

pub(super) use self::search::{search, CatalogueHit};
