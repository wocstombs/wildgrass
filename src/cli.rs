//! Command-line argument parsing and initial station resolution.

use reqwest::Client;

use crate::station::{self, Station, StationStore};
use crate::terminal;

pub const USAGE: &str = "\
usage: wildgrass                      open wildgrass
       wildgrass play                 play the first station on your list
       wildgrass play <station name>  play that listed station, or a station with that name
       wildgrass -V | --version       show version
       wildgrass -h | --help          show this help";

pub const KEYS: &str = "\
keys:  w / s, ↑ / ↓                   move selection
       enter / e                      play, search, edit, save
       backspace / delete             delete station (press twice)
       ctrl+s                         toggle station list and settings
       space                          pause / resume
       m                              mute / unmute
       - / +                          volume down / up
       ↑ / ↓ / ← / →                  move window while playing (Windows, WSL, X11)
       t                              cycle window on top / bottom / normal
       q / esc                        stop playback, go back, or quit
       ctrl+c                         exit immediately";

/// Parsed command-line invocation arguments.
#[derive(Debug, PartialEq, Eq)]
pub struct Cli {
    pub startup: CliStartup,
    /// True if launched via Windows Terminal profile relaunch.
    pub reopened: bool,
}

/// Initial execution mode requested via command line.
#[derive(Debug, PartialEq, Eq)]
pub enum CliStartup {
    Interactive,
    First,
    Station(String),
    Version,
    Help,
}

pub fn parse_cli_args(args: &[String]) -> Result<Cli, String> {
    let (reopened, args) = match args.split_first() {
        Some((first, rest)) if first == terminal::REOPENED => (true, rest),
        _ => (false, args),
    };
    let startup = match args.first().map(String::as_str) {
        None => CliStartup::Interactive,
        Some("-h" | "--help") => CliStartup::Help,
        Some("-V" | "--version") => CliStartup::Version,
        Some("play") => {
            let query = args[1..].join(" ").trim().to_string();
            if query.is_empty() {
                CliStartup::First
            } else {
                CliStartup::Station(query)
            }
        }
        Some(other) => return Err(format!("unknown argument {other}")),
    };
    Ok(Cli { startup, reopened })
}

/// Resolves the station to start playing from the command line, if any.
pub async fn initial_station(
    cli: CliStartup,
    stations: &StationStore,
    client: &Client,
) -> Option<Station> {
    match cli {
        CliStartup::Interactive | CliStartup::Version | CliStartup::Help => None,
        CliStartup::First => stations.first().cloned(),
        CliStartup::Station(query) => {
            let lowercase_query = query.to_lowercase();
            match stations
                .iter()
                .find(|station| station.name.to_lowercase().contains(&lowercase_query))
            {
                Some(local) => Some(local.clone()),
                None => station::search_radio_browser(client, &query, 1).await.pop(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Result<Cli, String> {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        parse_cli_args(&args)
    }

    fn parse(args: &[&str]) -> Result<CliStartup, String> {
        cli(args).map(|cli| cli.startup)
    }

    #[test]
    fn test_cli_args() {
        assert_eq!(parse(&[]), Ok(CliStartup::Interactive));
        assert_eq!(parse(&["-h"]), Ok(CliStartup::Help));
        assert_eq!(parse(&["--help"]), Ok(CliStartup::Help));
        assert_eq!(parse(&["-V"]), Ok(CliStartup::Version));
        assert_eq!(parse(&["--version"]), Ok(CliStartup::Version));
        assert_eq!(parse(&["play"]), Ok(CliStartup::First));
        assert_eq!(parse(&["play", " "]), Ok(CliStartup::First));
        assert_eq!(
            parse(&["play", "jazz", "fm"]),
            Ok(CliStartup::Station("jazz fm".into()))
        );
        // Removed flags and bare arguments are rejected
        for removed in [
            vec!["lofi"],
            vec!["station"],
            vec!["--station"],
            vec!["--station", "lofi"],
            vec!["search", "lofi"],
            vec!["--search", "lofi"],
            vec!["-s", "lofi"],
            vec!["-s"],
            vec!["-u", "https://x/stream"],
            vec!["--url", "https://x/stream"],
            vec!["https://x/stream"],
            vec!["--verbose"],
        ] {
            assert!(parse(&removed).is_err(), "{removed:?} should be rejected");
        }
    }

    #[test]
    fn test_the_reopened_copy_is_told_by_its_first_argument() {
        let reopened = terminal::REOPENED;
        assert_eq!(
            cli(&[reopened, "play", "jazz"]),
            Ok(Cli {
                startup: CliStartup::Station("jazz".into()),
                reopened: true
            })
        );
        assert_eq!(cli(&[reopened]).map(|cli| cli.reopened), Ok(true));
        assert_eq!(cli(&["play"]).map(|cli| cli.reopened), Ok(false));
        // Anywhere else it is an ordinary argument
        assert_eq!(
            cli(&["play", reopened]),
            Ok(Cli {
                startup: CliStartup::Station(reopened.into()),
                reopened: false
            })
        );
        assert!(cli(&[reopened, "--verbose"]).is_err());
    }

    #[tokio::test]
    async fn test_initial_station_picks_first_on_bare_s() {
        let stations = StationStore::new(
            vec![
                Station::new("First", "http://first"),
                Station::new("Second", "http://second"),
            ],
            None,
        );
        let picked = initial_station(CliStartup::First, &stations, &Client::new()).await;
        assert_eq!(
            picked.as_ref().map(|station| station.name.as_str()),
            Some("First")
        );
    }
}
