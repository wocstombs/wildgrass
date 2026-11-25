//! Configuration and cache file path resolution using XDG Base Directory standards.
//!
//! On Unix, respects `$XDG_CONFIG_HOME`, `$XDG_CACHE_HOME`, and `$XDG_STATE_HOME` with standard fallbacks.
//! On Windows, defaults to `%APPDATA%\wildgrass` for config and `%LOCALAPPDATA%\wildgrass` for cache/state.

use std::env;
use std::path::PathBuf;

/// Types of application data stored on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Station list configuration.
    Config,
    /// Cached metadata and downloaded lyrics.
    Cache,
    /// Log files.
    State,
}

impl Kind {
    /// Environment variable naming this directory under the XDG specification.
    fn xdg_variable(self) -> &'static str {
        match self {
            Self::Config => "XDG_CONFIG_HOME",
            Self::Cache => "XDG_CACHE_HOME",
            Self::State => "XDG_STATE_HOME",
        }
    }

    /// Default directory relative to the user's home folder when XDG variables are unset.
    fn home_default(self) -> &'static str {
        match self {
            Self::Config => ".config",
            Self::Cache => ".cache",
            Self::State => ".local/state",
        }
    }

    /// Windows folder mapping: `%APPDATA%` for config, `%LOCALAPPDATA%` for cache and state.
    fn windows_directory(self, environment: impl Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
        let wildgrass = |variable| Some(environment(variable)?.join("wildgrass"));
        match self {
            Self::Config => wildgrass("APPDATA"),
            Self::Cache => Some(wildgrass("LOCALAPPDATA")?.join("cache")),
            Self::State => Some(wildgrass("LOCALAPPDATA")?.join("state")),
        }
    }
}

/// The value of the environment variable `name` as a path, unless it is unset or blank.
fn env_path(name: &str) -> Option<PathBuf> {
    let value = env::var(name).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

/// wildgrass's directory for `kind`, reading the environment through `environment`:
/// `$<variable>/wildgrass`, or when the variable is unset `~/<default>/wildgrass` (on Windows
/// `Kind::windows_directory`).
fn wildgrass_directory_in(
    kind: Kind,
    windows: bool,
    environment: impl Fn(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(base) = environment(kind.xdg_variable()) {
        return Some(base.join("wildgrass"));
    }
    if windows {
        return kind.windows_directory(environment);
    }
    Some(
        environment("HOME")?
            .join(kind.home_default())
            .join("wildgrass"),
    )
}

fn wildgrass_directory(kind: Kind) -> Option<PathBuf> {
    wildgrass_directory_in(kind, cfg!(windows), env_path)
}

/// The station list: `$XDG_CONFIG_HOME/wildgrass/stations.json`.
pub fn stations_file() -> Option<PathBuf> {
    Some(wildgrass_directory(Kind::Config)?.join("stations.json"))
}

fn cache_directory() -> PathBuf {
    wildgrass_directory(Kind::Cache).unwrap_or_else(|| env::temp_dir().join("wildgrass-cache"))
}

/// Finished lyrics lookups: `$XDG_CACHE_HOME/wildgrass/lyrics` (in the temp directory when there
/// is no home directory).
pub fn lyrics_cache_directory() -> PathBuf {
    cache_directory().join("lyrics")
}

/// The log of the latest run: `$XDG_STATE_HOME/wildgrass/wildgrass.log`.
pub fn log_file() -> Option<PathBuf> {
    Some(wildgrass_directory(Kind::State)?.join("wildgrass.log"))
}

/// What Shazam answered and which lyrics were chosen, kept from run to run:
/// `$XDG_STATE_HOME/wildgrass/sync.log`.
pub fn sync_log_file() -> Option<PathBuf> {
    Some(wildgrass_directory(Kind::State)?.join("sync.log"))
}

/// How far apart recordings of the same song run (see `timeline::Shifts`):
/// `$XDG_CACHE_HOME/wildgrass/recordings.json`.
pub fn recordings_file() -> PathBuf {
    cache_directory().join("recordings.json")
}

/// Apple Music track lengths kept on disk (see `apple_music::TrackLengths`):
/// `$XDG_CACHE_HOME/wildgrass/track_lengths.json`.
pub fn track_lengths_file() -> PathBuf {
    cache_directory().join("track_lengths.json")
}

/// Where the `mpv` control socket goes: `$XDG_RUNTIME_DIR`, or the temp directory. Windows
/// uses a named pipe instead, which lives in no directory.
#[cfg(unix)]
pub fn runtime_directory() -> PathBuf {
    env_path("XDG_RUNTIME_DIR")
        .filter(|directory| directory.is_dir())
        .unwrap_or_else(env::temp_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An environment holding only `variables`.
    fn environment<'a>(variables: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<PathBuf> + 'a {
        |name| {
            variables
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| PathBuf::from(value))
        }
    }

    #[test]
    fn test_unix_uses_the_xdg_variable_or_the_home_directory() {
        let home = [("HOME", "/home/a")];
        let directory = |kind, variables| {
            wildgrass_directory_in(kind, /* windows= */ false, environment(variables))
        };
        assert_eq!(
            directory(Kind::Config, &home),
            Some(PathBuf::from("/home/a/.config/wildgrass"))
        );
        assert_eq!(
            directory(Kind::Cache, &home),
            Some(PathBuf::from("/home/a/.cache/wildgrass"))
        );
        assert_eq!(
            directory(Kind::State, &home),
            Some(PathBuf::from("/home/a/.local/state/wildgrass"))
        );
        let xdg = [("HOME", "/home/a"), ("XDG_CONFIG_HOME", "/cfg")];
        assert_eq!(
            directory(Kind::Config, &xdg),
            Some(PathBuf::from("/cfg/wildgrass"))
        );
        assert_eq!(directory(Kind::Config, &[]), None);
    }

    #[test]
    fn test_windows_uses_appdata_unless_an_xdg_variable_is_set() {
        let variables = [
            ("APPDATA", "C:/Users/a/AppData/Roaming"),
            ("LOCALAPPDATA", "C:/Users/a/AppData/Local"),
            ("HOME", "C:/Users/a"),
        ];
        let directory = |kind, variables| {
            wildgrass_directory_in(kind, /* windows= */ true, environment(variables))
        };
        assert_eq!(
            directory(Kind::Config, &variables),
            Some(PathBuf::from("C:/Users/a/AppData/Roaming/wildgrass"))
        );
        assert_eq!(
            directory(Kind::Cache, &variables),
            Some(PathBuf::from("C:/Users/a/AppData/Local/wildgrass/cache"))
        );
        assert_eq!(
            directory(Kind::State, &variables),
            Some(PathBuf::from("C:/Users/a/AppData/Local/wildgrass/state"))
        );
        let xdg = [("XDG_STATE_HOME", "D:/state"), ("LOCALAPPDATA", "C:/x")];
        assert_eq!(
            directory(Kind::State, &xdg),
            Some(PathBuf::from("D:/state/wildgrass"))
        );
        // HOME alone (as Git Bash sets it) is not used on Windows
        assert_eq!(directory(Kind::Config, &[("HOME", "C:/Users/a")]), None);
    }
}
