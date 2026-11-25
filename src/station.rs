//! Station catalog persistence, Qingting FM stream optimization, and Radio Browser search.

use std::fs;
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::{files, http};

const RADIO_BROWSER_MIRRORS: &[&str] = &[
    "https://de1.api.radio-browser.info",
    "https://fi1.api.radio-browser.info",
    "https://at1.api.radio-browser.info",
    "https://all.api.radio-browser.info",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "StationRecord")]
pub struct Station {
    pub stationuuid: String,
    pub name: String,
    /// Playable stream URL.
    pub url: String,
}

impl Station {
    #[cfg(test)]
    pub fn new(name: &str, url: &str) -> Self {
        Self {
            stationuuid: String::new(),
            name: name.to_string(),
            url: url.to_string(),
        }
    }

    /// Checks if `other` matches this station by UUID, URL, or name.
    /// `StationStore` enforces this uniqueness constraint.
    pub fn same_as(&self, other: &Station) -> bool {
        let shared = |a: &str, b: &str| !a.is_empty() && a == b;
        shared(&self.stationuuid, &other.stationuuid)
            || shared(&self.url, &other.url)
            || shared(&self.name, &other.name)
    }

    /// Returns true if both name and stream URL are non-empty.
    pub fn is_complete(&self) -> bool {
        !self.name.is_empty() && !self.url.is_empty()
    }

    /// Returns candidate stream URLs in priority order (CDN/HLS mirrors first, then raw URL).
    pub fn stream_urls(&self) -> Vec<String> {
        let mut urls = Vec::with_capacity(2);
        urls.extend(qingting_hls_url(&self.url));
        urls.push(self.url.clone());
        urls
    }
}

/// Hosts serving Qingting FM's direct MP3 streams. All of them are the same server in China.
const QINGTING_MP3_HOSTS: &[&str] = &["lhttp.qingting.fm", "lhttp.qtfm.cn", "lhttp-hw.qtfm.cn"];

/// Maps a Qingting FM direct MP3 link (`…/live/<id>/64k.mp3`) to the same station's HLS link.
///
/// The MP3 link is one long-lived connection to a single server. Measured from Europe, it
/// delivers only 0.6–0.9 seconds of audio per second (40% of pings to that server are lost),
/// so playback stalls every 20–40 seconds however much is buffered first. HLS publishes the
/// same audio as short files on a content delivery network, which download several times
/// faster than they play.
fn qingting_hls_url(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let (host, path) = rest.split_once('/')?;
    if !QINGTING_MP3_HOSTS.contains(&host.to_ascii_lowercase().as_str()) {
        return None;
    }
    let path = path.split(['?', '#']).next()?;
    let mut parts = path.strip_suffix(".mp3")?.split('/');
    let (Some("live"), Some(id), Some(quality), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let is_id = !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit());
    (is_id && !quality.is_empty())
        .then(|| format!("{scheme}://ls.qingting.fm/live/{id}/{quality}.m3u8"))
}

/// A station as stored in `stations.json` or returned by Radio Browser, which may carry both
/// the listed `url` and the playable `url_resolved` it redirects to.
#[derive(Deserialize, Default)]
#[serde(default)]
struct StationRecord {
    stationuuid: String,
    #[serde(alias = "station")]
    name: String,
    url: String,
    url_resolved: String,
}

impl From<StationRecord> for Station {
    fn from(record: StationRecord) -> Self {
        let url = if record.url_resolved.trim().is_empty() {
            record.url
        } else {
            record.url_resolved
        };
        Self {
            stationuuid: record.stationuuid,
            name: record.name.trim().to_string(),
            url: url.trim().to_string(),
        }
    }
}

/// Built-in default station list shipped with wildgrass.
pub fn default_stations() -> Vec<Station> {
    vec![
        Station {
            stationuuid: "preset-love-1037".into(),
            name: "Love Radio 103.7".into(),
            url: "http://lhttp.qingting.fm/live/273/64k.mp3".into(),
        },
        Station {
            stationuuid: "preset-lofi".into(),
            name: "lofi".into(),
            url: "https://www.youtube.com/watch?v=rFZHOHl-L8A".into(),
        },
        Station {
            stationuuid: "7ba4c184-fc2b-11e9-bbf2-52543be04c81".into(),
            name: "NPR 24 Hours Program".into(),
            url: "http://npr-ice.streamguys1.com/live.mp3".into(),
        },
    ]
}

fn is_legacy_preset(uuid: &str) -> bool {
    uuid.starts_with("preset-") && uuid != "preset-love-1037" && uuid != "preset-lofi"
}

/// Reads the stations saved at `path`, leaving out the presets that older versions shipped.
/// `Ok(None)` when nothing is saved there yet (no file, or an empty one).
fn read_saved(path: &Path) -> io::Result<Option<Vec<Station>>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if bytes.trim_ascii().is_empty() {
        return Ok(None);
    }
    let saved: Vec<Station> = serde_json::from_slice(&bytes)?;
    Ok(Some(
        saved
            .into_iter()
            .filter(|station| !is_legacy_preset(&station.stationuuid))
            .collect(),
    ))
}

/// The user's station list. Every change is written straight back to the file the list was
/// loaded from, and no two listed stations are ever the same station (`Station::same_as`).
///
/// Reads go through `Deref<Target = [Station]>`; changes only through the methods here, so the
/// list in memory and the list on disk cannot drift apart.
pub struct StationStore {
    stations: Vec<Station>,
    /// Where the list is saved; `None` keeps it in memory only.
    path: Option<PathBuf>,
    /// Why the most recent save failed, if it did.
    save_error: Option<io::Error>,
}

impl StationStore {
    /// Lists `stations` (minus incomplete entries and repeats of an earlier one), saving later
    /// changes to `path`.
    pub fn new(stations: Vec<Station>, path: Option<PathBuf>) -> Self {
        let mut store = Self {
            stations: Vec::with_capacity(stations.len()),
            path,
            save_error: None,
        };
        for station in stations {
            if station.is_complete() && store.position_of(&station).is_none() {
                store.stations.push(station);
            }
        }
        store
    }

    /// Loads the list saved at `path`, or `default_stations()` when nothing is saved there.
    ///
    /// A file that cannot be read, or is not a valid station list, is an error rather than an
    /// empty list: carrying on would overwrite it, and the stations in it, on the next change.
    pub fn load(path: Option<PathBuf>) -> io::Result<Self> {
        let saved = match &path {
            Some(path) => read_saved(path).map_err(|error| {
                io::Error::new(error.kind(), format!("{}: {error}", path.display()))
            })?,
            None => None,
        };
        let mut store = Self::new(saved.unwrap_or_default(), path);
        if store.stations.is_empty() {
            store.stations = default_stations();
        }
        Ok(store)
    }

    /// Position of the listed station that is the same station as `station`.
    pub fn position_of(&self, station: &Station) -> Option<usize> {
        self.stations
            .iter()
            .position(|listed| listed.same_as(station))
    }

    /// Whether a listed station other than the one at `index` is the same station as `station`,
    /// so that storing `station` at `index` would list it twice.
    pub fn conflicts(&self, index: usize, station: &Station) -> bool {
        self.stations
            .iter()
            .enumerate()
            .any(|(other_index, other)| other_index != index && other.same_as(station))
    }

    /// Adds `station` unless it is already listed, and returns its position either way.
    pub fn add(&mut self, station: Station) -> usize {
        if let Some(index) = self.position_of(&station) {
            return index;
        }
        self.stations.push(station);
        self.save();
        self.stations.len() - 1
    }

    /// Replaces the station at `index`. Callers rule out `conflicts` first.
    pub fn replace(&mut self, index: usize, station: Station) {
        debug_assert!(
            !self.conflicts(index, &station),
            "callers rule out conflicts before replacing a station"
        );
        self.stations[index] = station;
        self.save();
    }

    pub fn remove(&mut self, index: usize) {
        self.stations.remove(index);
        self.save();
    }

    /// Why the most recent change could not be written to disk, if it could not.
    pub fn save_error(&self) -> Option<&io::Error> {
        self.save_error.as_ref()
    }

    fn save(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        let written = serde_json::to_vec_pretty(&self.stations)
            .map_err(io::Error::from)
            .and_then(|json| files::write_atomically(path, &json));
        self.save_error = written.err();
    }
}

impl Deref for StationStore {
    type Target = [Station];

    fn deref(&self) -> &[Station] {
        &self.stations
    }
}

/// Searches the global Radio Browser directory for live radio stations, trying each mirror in
/// turn until one answers with results.
pub async fn search_radio_browser(client: &Client, query: &str, limit: usize) -> Vec<Station> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    for mirror in RADIO_BROWSER_MIRRORS {
        let url = format!(
            "{mirror}/json/stations/search?name={}&hidebroken=true&order=clickcount&reverse=true&limit={}",
            urlencoding::encode(trimmed),
            limit.clamp(1, 50)
        );
        let answer: Option<Vec<Station>> = http::get_json(client.get(&url), "station search").await;
        let Some(rows) = answer else {
            continue;
        };
        let valid: Vec<Station> = rows
            .into_iter()
            .filter(|station| {
                !station.name.is_empty()
                    && (station.url.starts_with("http://") || station.url.starts_with("https://"))
            })
            .collect();
        if !valid.is_empty() {
            return valid;
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ScratchDir;

    #[test]
    fn test_default_stations_match_shipped_presets() {
        let stations = default_stations();
        assert_eq!(stations.len(), 3);
        assert_eq!(stations[0].name, "Love Radio 103.7");
        assert_eq!(stations[0].url, "http://lhttp.qingting.fm/live/273/64k.mp3");
        assert_eq!(stations[1].name, "lofi");
        assert_eq!(
            stations[1].url,
            "https://www.youtube.com/watch?v=rFZHOHl-L8A"
        );
        assert_eq!(stations[2].name, "NPR 24 Hours Program");
        assert_eq!(stations[2].url, "http://npr-ice.streamguys1.com/live.mp3");
    }

    #[test]
    fn test_stream_urls_prefer_hls_for_qingting_mp3_links() {
        let station = |url: &str| Station {
            url: url.into(),
            ..Station::default()
        };
        assert_eq!(
            default_stations()[0].stream_urls(),
            [
                "http://ls.qingting.fm/live/273/64k.m3u8",
                "http://lhttp.qingting.fm/live/273/64k.mp3"
            ]
        );
        assert_eq!(
            station("https://lhttp.qtfm.cn/live/4915/24k.mp3?app=web").stream_urls()[0],
            "https://ls.qingting.fm/live/4915/24k.m3u8"
        );

        // Anything else plays from its own address only
        for url in [
            "https://www.youtube.com/live/jfKfPfyJRdk",
            "http://ice1.somafm.com/groovesalad-128-mp3",
            "http://lhttp.qingting.fm/vod/273/64k.mp3",
            "http://lhttp.qingting.fm/live/abc/64k.mp3",
            "http://example.com/live/273/64k.mp3",
            "not a url",
        ] {
            assert_eq!(station(url).stream_urls(), [url]);
        }
    }

    #[test]
    fn test_station_records_prefer_resolved_url_and_trim() {
        let saved = r#"[
            {"stationuuid": "a", "name": " Jazz ", "url": "http://x/pls", "url_resolved": "http://x/stream "},
            {"stationuuid": "b", "station": "Legacy", "url": "http://y/stream"}
        ]"#;
        let stations: Vec<Station> = serde_json::from_str(saved).unwrap();
        assert_eq!(stations[0].name, "Jazz");
        assert_eq!(stations[0].url, "http://x/stream");
        assert_eq!(stations[1].name, "Legacy");
        assert_eq!(stations[1].url, "http://y/stream");

        // Saving and loading again round-trips
        let json = serde_json::to_string(&stations).unwrap();
        let reloaded: Vec<Station> = serde_json::from_str(&json).unwrap();
        assert_eq!(reloaded, stations);
    }

    #[test]
    fn test_same_station_shares_a_uuid_a_link_or_a_name() {
        let jazz = Station {
            stationuuid: "uuid-1".into(),
            ..Station::new("Jazz", "http://jazz/stream")
        };
        let renamed = Station {
            stationuuid: "uuid-1".into(),
            ..Station::new("Jazz FM", "http://jazz/hq")
        };
        assert!(jazz.same_as(&renamed));
        assert!(jazz.same_as(&Station::new("Smooth", "http://jazz/stream")));
        assert!(jazz.same_as(&Station::new("Jazz", "http://mirror/stream")));
        assert!(!jazz.same_as(&Station::new("Rock", "http://rock/stream")));
        // Fields that are still empty identify nothing
        assert!(!Station::default().same_as(&Station::default()));
    }

    fn names(store: &StationStore) -> Vec<&str> {
        store.iter().map(|station| station.name.as_str()).collect()
    }

    #[test]
    fn test_store_saves_every_change_and_loads_it_back() {
        let scratch = ScratchDir::create();
        let path = scratch.path().join("wildgrass").join("stations.json");

        // Nothing saved yet: the built-in stations, and nothing written until a change
        let mut store = StationStore::load(Some(path.clone())).unwrap();
        assert_eq!(
            names(&store),
            ["Love Radio 103.7", "lofi", "NPR 24 Hours Program"]
        );
        assert!(!path.exists());

        assert_eq!(store.add(Station::new("Jazz", "http://jazz")), 3);
        // Adding a station that is already listed points at the listed one
        assert_eq!(store.add(Station::new("Jazz", "http://mirror")), 3);
        store.replace(0, Station::new("Love", "http://love"));
        store.remove(1);
        assert!(store.save_error().is_none());

        let reloaded = StationStore::load(Some(path.clone())).unwrap();
        assert_eq!(names(&reloaded), ["Love", "NPR 24 Hours Program", "Jazz"]);
        assert_eq!(reloaded[2].url, "http://jazz");
        // The temporary file was renamed into place
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn test_store_load_drops_repeats_incomplete_entries_and_legacy_presets() {
        let scratch = ScratchDir::create();
        let path = scratch.path().join("stations.json");
        let saved = r#"[
            {"stationuuid": "preset-bbc", "name": "Old preset", "url": "http://old"},
            {"name": "Jazz", "url": "http://jazz"},
            {"name": "Jazz", "url": "http://other"},
            {"name": "Mirror", "url": "http://jazz"},
            {"name": "No link", "url": ""},
            {"name": "Rock", "url": "http://rock"}
        ]"#;
        fs::write(&path, saved).unwrap();
        assert_eq!(
            names(&StationStore::load(Some(path.clone())).unwrap()),
            ["Jazz", "Rock"]
        );

        // An empty file holds no stations yet, so the built-in ones are listed
        fs::write(&path, " \n").unwrap();
        assert_eq!(StationStore::load(Some(path)).unwrap().len(), 3);
    }

    #[test]
    fn test_store_refuses_a_list_it_cannot_read_instead_of_replacing_it() {
        let scratch = ScratchDir::create();
        let path = scratch.path().join("stations.json");
        let typo = r#"[{"name": "Jazz", "url": "http://jazz"},]"#;
        fs::write(&path, typo).unwrap();

        let message = StationStore::load(Some(path.clone()))
            .err()
            .unwrap()
            .to_string();
        assert!(
            message.starts_with(&path.display().to_string()),
            "{message}"
        );
        assert!(message.contains("line 1"), "{message}");
        // The file is left exactly as it was
        assert_eq!(fs::read_to_string(&path).unwrap(), typo);
    }

    #[test]
    fn test_store_reports_a_failed_save_and_keeps_the_change_in_memory() {
        let scratch = ScratchDir::create();
        let blocker = scratch.path().join("not-a-directory");
        fs::write(&blocker, "").unwrap();

        let mut store = StationStore::new(Vec::new(), Some(blocker.join("stations.json")));
        store.add(Station::new("Jazz", "http://jazz"));
        assert!(store.save_error().is_some());
        assert_eq!(names(&store), ["Jazz"]);
    }

    #[test]
    fn test_store_conflicts_ignore_the_station_being_replaced() {
        let store = StationStore::new(
            vec![
                Station::new("Jazz", "http://jazz"),
                Station::new("Rock", "http://rock"),
            ],
            None,
        );
        assert!(!store.conflicts(0, &Station::new("Jazz", "http://jazz/hq")));
        assert!(store.conflicts(1, &Station::new("Jazz", "http://jazz/hq")));
        // A new station (one past the end) is checked against every listed one
        assert!(store.conflicts(2, &Station::new("Blues", "http://rock")));
        assert!(!store.conflicts(2, &Station::new("Blues", "http://blues")));
    }
}
