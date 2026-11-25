# wildgrass

A minimalist CLI for internet radio, written 100% in Rust. It blends perfectly into the background while stubbornly maintaining its presence to remind you that the world out there still exists.

## Usage

```bash
wildgrass
```
If it's your first time running, remember to close your old terminal sessions and start a new one.

## How Do I Install

### Windows

You can download and run `wildgrass-setup.exe` from the [Releases](https://github.com/wocstombs/wildgrass/releases) page.

- To install from Winget, type in Powershell

```powershell
winget install wildgrass
```

### Linux / WSL

Wildgrass is made to be fully compatible with Hyprland and Kitty.

**IMPORTANT!** See *[installation priority](#installation-priority)* for further information on dependencies. If that's not your worry, paste these sequentially into your terminal:

- **Ubuntu / Debian / Mint:**

```bash
sudo apt install build-essential curl mpv yt-dlp ffmpeg pulseaudio-utils
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env" && cargo install wildgrass
```

- **Arch:**

```bash
sudo pacman -S rust mpv yt-dlp ffmpeg libpulse
cargo install wildgrass
```

- **Fedora:**

```bash
sudo dnf install rust cargo mpv yt-dlp ffmpeg-free pulseaudio-utils
cargo install wildgrass
```

## Configuration

Your default stations exist in [station.rs](https://github.com/wocstombs/wildgrass/blob/main/src/station.rs), as a list of station names and links. It looks like this:

```rust
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
```

Don't worry though, your stations can be fully customised inside the app.


## Keybinds

The only two keybinds that are functionally important but kind of hard to figure out on your own are:

| Key | Action |
| --- | --- |
| `t` | Cycles wildgrass on top of all windows, on the bottom of all windows, or normally |
| `Ctrl+C` | Exit immediately |

*Note: Unfortunately `t` (and window moving while playing using `↑ ↓ ← →`) only works on Windows, WSL, and Linux X11*

Everything else should feel natural. Explore around! If you're ever stuck, type in a new terminal

```bash
wildgrass --help
```

## Known Issues and Fixes

### Audio Dropouts on WSL
  - If you experience periodic audio dropouts or micro-stutters every few seconds across audio players (you can tell from `mpv` logging `Audio device underrun detected`), it's because:

    - Under WSL, Windows Hyper-V host time synchronisation speeds up the Linux kernel clock (running ~2.6% fast), while `systemd-timesyncd` periodically pulls the clock backwards by ~1 second. This jittering disrupts the PulseAudio/PipeWire audio timing quite noticeably.
    - **Fix**: Disable the redundant `systemd-timesyncd` daemon and let WSL sync directly with host time:

  ```bash
  sudo systemctl disable --now systemd-timesyncd
  ```
  To revert:
  ```bash
  sudo systemctl enable --now systemd-timesyncd
  ```

## Installation Priority

### Mandatory

- **mpv** enables live audio streaming and audio control. (Stuff like pause with `space` and adjusting volume with `-`/`+`).
- **Rust** & **Cargo** if you install with `cargo install wildgrass` or build from source.

### Recommended

- **pactl** & **parec** enable visualiser, song recognition, and live lyrics syncing.
- **ffmpeg** works as a backup to `mpv` when it acts up.
- **yt-dlp** allows YouTube live stream radios to be added.

If you're hoping for a more lightweight experience, I heavily recommend [radio-cli](https://github.com/margual56/radio-cli). It's a great app on its own. I "borrowed" a lot of design elements from it, too.

## AI Disclosure

This app started from a prototype I wrote where Python handled API backend (`mpv`, `shazamio`) and the songRec pipeline while Java's `lanterna` library handled visualiser and UI/UX. As you can tell, that arrangement was not ideal. Thus I decided to do what any sane person would - have Claude Code and Agy rewrite the whole thing in Rust :)

## License

Licensed under the [MIT License](LICENSE).
