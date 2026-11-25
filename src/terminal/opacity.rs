//! Terminal window opacity controls for Hyprland and kitty.

use std::env;
use std::process::{Command, Stdio};

use serde_json::Value;

/// Target window opacity while running.
const OPACITY: &str = "0.85";

/// Tracks windows whose opacity was adjusted.
pub(super) enum Window {
    /// Active Hyprland window address.
    Hyprland { address: String },
    /// Kitty terminal window identifier.
    Kitty { id: String },
}

impl Window {
    /// Sets the active window's opacity to `OPACITY` and records modified windows.
    pub(super) fn make_translucent() -> Vec<Self> {
        let mut changed = Vec::new();
        if env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            if let Some(address) = hyprland_active_window() {
                hyprctl_batch(&format!(
                    "dispatch focuswindow address:{address} ; \
                     dispatch setprop address:{address} alpha {OPACITY} ; \
                     dispatch setprop address:{address} alphainactive {OPACITY}"
                ));
                changed.push(Self::Hyprland { address });
            }
        }
        // Requires `allow_remote_control` and `dynamic_background_opacity` in kitty.conf
        if let Ok(id) = env::var("KITTY_WINDOW_ID") {
            kitty_background_opacity(&id, &[OPACITY]);
            changed.push(Self::Kitty { id });
        }
        changed
    }

    /// Restores the window's original opacity.
    pub(super) fn restore(&self) {
        match self {
            // Hyprland cannot unset per-window opacity directly, so restore the configured default
            Self::Hyprland { address } => hyprctl_batch(&format!(
                "dispatch setprop address:{address} alpha {} ; \
                 dispatch setprop address:{address} alphainactive {}",
                hyprland_opacity("active_opacity"),
                hyprland_opacity("inactive_opacity"),
            )),
            // --toggle restores opacity configured in kitty.conf if wildgrass altered it
            Self::Kitty { id } => kitty_background_opacity(id, &["--toggle", OPACITY]),
        }
    }
}

/// Runs `program` with `args`, ignoring errors for non-essential visual settings.
fn run_quietly(program: &str, args: &[&str]) {
    let _ = Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn hyprctl_batch(commands: &str) {
    run_quietly("hyprctl", &["--batch", commands]);
}

fn kitty_background_opacity(window_id: &str, args: &[&str]) {
    let window = format!("id:{window_id}");
    let mut all_args = vec!["@", "set-background-opacity", "--match", &window];
    all_args.extend(args);
    run_quietly("kitty", &all_args);
}

/// Runs `hyprctl -j` with `args` and parses the JSON response.
fn hyprctl_json(args: &[&str]) -> Option<Value> {
    let output = Command::new("hyprctl")
        .arg("-j")
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    serde_json::from_slice(&output.stdout).ok()
}

fn hyprland_active_window() -> Option<String> {
    let window = hyprctl_json(&["activewindow"])?;
    Some(window.get("address")?.as_str()?.to_string())
}

/// Reads configured opacity from Hyprland configuration (defaults to 1.0 if not found).
fn hyprland_opacity(option: &str) -> f64 {
    hyprctl_json(&["getoption", &format!("decoration:{option}")])
        .and_then(|answer| answer.get("float")?.as_f64())
        .unwrap_or(1.0)
}
