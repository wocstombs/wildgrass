//! wildgrass: minimal terminal internet radio player with synchronized lyrics.
//!
//! Initializes subsystems, launches the player process, and runs the main event loop.

mod app;
mod apple_music;
mod audio;
mod chinese;
mod cli;
mod embedding;
mod files;
mod http;
mod icy;
mod input;
mod log;
mod lyrics;
mod paths;
mod playback;
mod player;
mod shazam;
mod spectrum;
mod station;
mod task;
mod terminal;
#[cfg(test)]
mod test_util;
mod text;
mod theme;
mod timeline;
mod track;
mod ui;

use std::env;
use std::error::Error;
use std::io;
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use crossterm::event::{Event, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use reqwest::Client;

use crate::app::{App, Flow};
use crate::cli::CliStartup;
use crate::input::Input;
use crate::player::MpvPlayer;
use crate::station::StationStore;
use crate::terminal::TerminalGuard;

#[tokio::main]
async fn main() -> ExitCode {
    use std::io::Write;

    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        // TerminalGuard has been dropped, so write error to stderr on the primary screen.
        // Use writeln! rather than eprintln! to avoid panicking if stderr is broken.
        Err(error) => {
            let _ = writeln!(io::stderr(), "wildgrass: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let cli = cli::parse_cli_args(&args).map_err(|error| format!("{error}\n{}", cli::USAGE))?;
    if cli.startup == CliStartup::Help {
        println!("{}\n\n{}", cli::USAGE, cli::KEYS);
        return Ok(());
    }
    if cli.startup == CliStartup::Version {
        println!("wildgrass {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    // Relaunch in dedicated Windows Terminal profile if running natively on Windows
    if !cli.reopened && terminal::reopen_in_profile(&args) {
        return Ok(());
    }

    if let Some(path) = paths::log_file() {
        if let Err(error) = log::init(&path) {
            eprintln!(
                "wildgrass: not logging errors to {}: {error}",
                path.display()
            );
        }
    }
    if let Some(path) = paths::sync_log_file() {
        if let Err(error) = log::init_sync(&path) {
            log::error(format!("not logging sync to {}: {error}", path.display()));
        }
    }
    let stations = StationStore::load(paths::stations_file()).map_err(|error| {
        format!("could not read the station list at {error}\nfix or move that file, then start wildgrass again")
    })?;
    // Register signal handlers before spawning mpv to avoid orphan playback processes
    let (input_sender, input) = mpsc::channel();
    input::stop_on_signals(input_sender.clone())?;
    let player = MpvPlayer::spawn()
        .map_err(|error| format!("could not start mpv, which plays the audio: {error}"))?;
    let client = Client::builder()
        .timeout(Duration::from_secs(6))
        .user_agent(concat!("wildgrass/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let initial = cli::initial_station(cli.startup, &stations, &client).await;
    let mut app = App::new(stations, Some(player), client);
    if let Some(station) = &initial {
        app.play(station);
    }

    let mut terminal_guard = TerminalGuard::acquire()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    input::read_terminal_events(input_sender);

    loop {
        app.tick().await;

        match input.recv_timeout(app.poll_timeout()) {
            Ok(Input::Stop) => break,
            Ok(Input::Terminal(Ok(Event::Key(key)))) if key.kind == KeyEventKind::Press => {
                match app.handle_key(key) {
                    Flow::Continue => {}
                    Flow::Window(action) => terminal_guard.apply(action),
                    Flow::Quit => break,
                }
            }
            Ok(Input::Terminal(Err(error))) => return Err(error.into()),
            // Non-key event or poll timeout
            Ok(Input::Terminal(Ok(_))) | Err(RecvTimeoutError::Timeout) => {}
            // Input channel disconnected
            Err(RecvTimeoutError::Disconnected) => break,
        }

        // Redraw each iteration: Ratatui diffs frames and only flushes modified cells
        terminal.draw(|frame| ui::render(frame, &app))?;
    }
    Ok(())
}
