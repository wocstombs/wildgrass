//! Synchronized lyrics retrieval, parsing, caching, and timing adjustment.
//!
//! Provides the data structures for synchronized karaoke lyrics, disk caching,
//! multi-provider catalogue search (QQ Music, Kugou, NetEase, LRCLIB), and vocal timing alignment.

mod cache;
mod catalogue;
mod fetch;
mod lyrics;
mod lyrics_match;
mod parse;
mod release;
mod timing;

pub use self::cache::LyricsCache;
pub use self::fetch::{fetch_lyrics, Found};
pub use self::lyrics::{LyricLine, LyricWord, Lyrics};
pub use self::lyrics_match::LyricsMatch;
pub use self::release::Release;
pub use self::timing::Singing;
