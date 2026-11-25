//! Asynchronous terminal event ingestion and OS shutdown signal handling.

use std::sync::mpsc;
use std::{io, thread};

use crossterm::event::{self, Event};

/// Input events delivered to the main event loop.
pub enum Input {
    /// Terminal key or window event.
    Terminal(io::Result<Event>),
    /// OS shutdown or window close signal requesting termination.
    Stop,
}

/// Spawns background signal listeners (`SIGHUP`, `SIGTERM` on Unix; close, logoff, shutdown on Windows).
/// Delivers `Input::Stop` to trigger graceful RAII cleanup of terminal state and child processes.
pub fn stop_on_signals(input: mpsc::Sender<Input>) -> io::Result<()> {
    #[cfg(unix)]
    let stop_requested = {
        use tokio::signal::unix::{self, SignalKind};
        let mut hangup = unix::signal(SignalKind::hangup())?;
        let mut terminate = unix::signal(SignalKind::terminate())?;
        async move {
            tokio::select! {
                _ = hangup.recv() => {}
                _ = terminate.recv() => {}
            }
        }
    };
    #[cfg(windows)]
    let stop_requested = {
        use tokio::signal::windows;
        let mut close = windows::ctrl_close()?;
        let mut logoff = windows::ctrl_logoff()?;
        let mut shutdown = windows::ctrl_shutdown()?;
        async move {
            tokio::select! {
                _ = close.recv() => {}
                _ = logoff.recv() => {}
                _ = shutdown.recv() => {}
            }
        }
    };
    tokio::spawn(async move {
        stop_requested.await;
        let _ = input.send(Input::Stop);
    });
    Ok(())
}

/// Reads terminal events on a thread of its own and sends them to the main loop.
///
/// Once the terminal has hung up (its window was closed), crossterm's `event::poll` and
/// `event::read` never return: they keep re-reading the dead terminal. The main loop must not be
/// the one stuck there, or it could never notice the signal that asks it to stop. Only this
/// thread can get stuck; it ends with the process.
pub fn read_terminal_events(input: mpsc::Sender<Input>) {
    thread::spawn(move || loop {
        let event = event::read();
        let failed = event.is_err();
        if input.send(Input::Terminal(event)).is_err() || failed {
            break;
        }
    });
}

// `kill` and these signals are Unix's; a Windows console's close event cannot be sent to a test
// process without closing its console
#[cfg(all(test, unix))]
mod tests {
    use std::process::{self, Command};
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn test_sighup_and_sigterm_send_a_stop_instead_of_killing_the_process() {
        for option in ["-HUP", "-TERM"] {
            let (sender, input) = mpsc::channel();
            stop_on_signals(sender).unwrap();
            assert!(input.try_recv().is_err());

            // Once the handler is registered, the signal goes to it and the test process lives on
            let status = Command::new("kill")
                .args([option, &process::id().to_string()])
                .status()
                .unwrap();
            assert!(status.success());

            // Polled, not waited for: the signal task runs on this very thread
            let mut stopped = false;
            for _ in 0..100 {
                stopped = matches!(input.try_recv(), Ok(Input::Stop));
                if stopped {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(stopped, "{option} did not send Stop within 2 seconds");
        }
    }
}
