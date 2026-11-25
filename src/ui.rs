//! TUI rendering using Ratatui with minimalist styling.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{
    App, ListMode, Search, SettingsField, StationEditor, StationList, TextInput, View,
};
use crate::lyrics::Lyrics;
use crate::playback::{Playback, ShownLyrics};
use crate::spectrum::{Spectrum, SPECTRUM_BANDS};
use crate::theme;

const OTHER_LABEL: &str = "Other";
const CUSTOM_LABEL: &str = "Custom";
const DUPLICATE_NAME_HINT: &str = "another station already has that name";
const DUPLICATE_LINK_HINT: &str = "another station already has that link";

pub fn render(frame: &mut Frame, app: &App) {
    frame.render_widget(Clear, frame.area());
    match &app.view {
        View::StationList(list) => render_station_list(frame, app, list),
        View::StationSettings(editor) => {
            let problem = match editor.conflict {
                Some(SettingsField::Name) => Some(DUPLICATE_NAME_HINT.to_string()),
                Some(SettingsField::Link) => Some(DUPLICATE_LINK_HINT.to_string()),
                None => save_failure(app),
            };
            render_lines(
                frame,
                build_station_settings_lines(editor, problem.as_deref()),
            );
        }
        View::Search(search) => render_search(frame, search),
        View::NowPlaying(playback) => render_now_playing(frame, playback),
    }
}

/// Formats the error banner if the station directory failed to persist to disk.
fn save_failure(app: &App) -> Option<String> {
    let error = app.stations.save_error()?;
    Some(format!("could not save stations: {error}"))
}

fn render_lines(frame: &mut Frame, lines: Vec<Line<'_>>) {
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        frame.area(),
    );
}

/// Formats a bold title line and an italic navigation hint.
fn header(title: &str, hint: &str) -> Vec<Line<'static>> {
    vec![
        Line::styled(title.to_string(), theme::TITLE),
        Line::styled(hint.to_string(), theme::HINT),
    ]
}

/// Renders header `lines` above a list that scrolls to keep the `selected` row centered.
fn render_list<'text>(
    frame: &mut Frame,
    mut lines: Vec<Line<'text>>,
    items: impl ExactSizeIterator<Item = &'text str>,
    selected: usize,
    inactive_color: Color,
) {
    let total = items.len();
    let max_rows = (frame.area().height as usize)
        .saturating_sub(lines.len())
        .max(1);
    let selected = selected.min(total.saturating_sub(1));
    let start = selected
        .saturating_sub(max_rows / 2)
        .min(total.saturating_sub(max_rows));

    let shown = items.enumerate().skip(start).take(max_rows);
    for (index, label) in shown {
        let (prefix, style) = if index == selected {
            ("> ", theme::SELECTED)
        } else {
            ("  ", Style::new().fg(inactive_color))
        };
        lines.push(Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(label, style),
        ]));
    }
    render_lines(frame, lines);
}

fn render_station_list(frame: &mut Frame, app: &App, list: &StationList) {
    let (mode_hint, trailing_row) = match list.mode {
        ListMode::Play => ("press ctrl + s to switch to settings mode", OTHER_LABEL),
        ListMode::Edit => ("press ctrl + s to switch to selection mode", CUSTOM_LABEL),
    };
    let hint = if list.delete_confirm {
        "delete radio?".to_string()
    } else {
        save_failure(app).unwrap_or_else(|| mode_hint.to_string())
    };
    let labels: Vec<&str> = app
        .stations
        .iter()
        .map(|station| station.name.as_str())
        .chain([trailing_row])
        .collect();
    render_list(
        frame,
        header("Select a station to play:", &hint),
        labels.into_iter(),
        app.cursor,
        theme::TEXT_PRIMARY,
    );
}

/// The station editor: a title, a hint line, and the two fields. `problem` replaces the key
/// hint when the last entry was refused or could not be saved.
fn build_station_settings_lines(
    editor: &StationEditor,
    problem: Option<&str>,
) -> Vec<Line<'static>> {
    let StationEditor {
        station,
        field,
        edit,
        ..
    } = editor;
    let hint = problem.unwrap_or(if edit.is_some() {
        "press enter to save, esc to cancel"
    } else {
        "press enter to edit, esc to go back"
    });
    let field_line = |this: SettingsField, label: &str, value: &str| {
        let selected = this == *field;
        let (prefix, style) = if selected {
            ("> ", theme::SELECTED)
        } else {
            ("  ", Style::new().fg(theme::TEXT_PRIMARY))
        };
        let mut spans = vec![
            Span::styled(prefix, style),
            Span::styled(format!("{label}: "), style),
        ];
        match edit {
            Some(buffer) if selected => spans.extend(typed_spans(buffer, style)),
            _ => spans.push(Span::styled(value.to_string(), style)),
        }
        Line::from(spans)
    };

    // A new station has no name to show until one is typed
    let title = if station.name.is_empty() {
        CUSTOM_LABEL
    } else {
        station.name.as_str()
    };
    let mut lines = header(title, hint);
    lines.push(field_line(SettingsField::Name, "Name", &station.name));
    lines.push(field_line(SettingsField::Link, "Link", &station.url));
    lines
}

/// Draws `input` with a block cursor: the character under the cursor in reverse video, or `█`
/// when the cursor is after the last character.
fn typed_spans(input: &TextInput, style: Style) -> Vec<Span<'static>> {
    let mut after = input.after_cursor().chars();
    let cursor = match after.next() {
        Some(character) => Span::styled(
            character.to_string(),
            style.add_modifier(Modifier::REVERSED),
        ),
        None => Span::styled("█", style),
    };
    vec![
        Span::styled(input.before_cursor().to_string(), style),
        cursor,
        Span::styled(after.as_str().to_string(), style),
    ]
}

fn render_search(frame: &mut Frame, search: &Search) {
    let mut prompt = vec![Span::styled("Search for a station: ", theme::TITLE)];
    prompt.extend(typed_spans(&search.query, Style::new().fg(theme::ACCENT)));
    render_list(
        frame,
        vec![Line::from(prompt)],
        search.results.iter().map(|station| station.name.as_str()),
        search.selected,
        theme::TEXT_SECONDARY,
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlaybackStatus {
    Streaming,
    Buffering,
    Paused,
}

fn render_now_playing(frame: &mut Frame, playback: &Playback) {
    let status = if playback.is_paused() {
        PlaybackStatus::Paused
    } else if playback.is_idle() {
        PlaybackStatus::Buffering
    } else {
        PlaybackStatus::Streaming
    };
    let lines = build_now_playing_lines(
        &playback.station_name,
        status,
        playback.display_name(),
        playback.lyrics_at(),
        playback.spectrum(),
        frame.area().width,
    );
    render_lines(frame, lines);
}

fn build_now_playing_lines(
    station_name: &str,
    status: PlaybackStatus,
    song_display_name: Option<&str>,
    shown_lyrics: Option<ShownLyrics>,
    spectrum: &Spectrum,
    width: u16,
) -> Vec<Line<'static>> {
    let status_label = match status {
        PlaybackStatus::Paused => "Paused: ",
        PlaybackStatus::Streaming | PlaybackStatus::Buffering => "Streaming: ",
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(status_label, Style::new().fg(theme::TEXT_SECONDARY)),
        Span::styled(station_name.to_string(), theme::TITLE),
    ])];

    if let Some(song) = song_display_name {
        lines.push(Line::styled(song.to_string(), theme::HINT));
        if let Some(shown) = shown_lyrics {
            let lyric_line = build_lyrics_line(shown.lyrics, shown.position);
            if !lyric_line.spans.is_empty() {
                lines.push(lyric_line);
            }
        }
    }

    lines.extend(build_braille_wave_lines(
        width,
        spectrum,
        status == PlaybackStatus::Buffering,
    ));
    lines
}

/// Builds the current line of lyrics with word-by-word karaoke highlighting,
/// returning an empty line before the first lyric.
fn build_lyrics_line(lyrics: &Lyrics, song_position: f64) -> Line<'static> {
    let Some(index) = lyrics.index_at(song_position) else {
        return Line::default();
    };

    let text = &lyrics.lines[index].text;
    let sung_characters = lyrics.sung_characters(index, song_position);
    let sung_part: String = text.chars().take(sung_characters).collect();
    let unsung_part: String = text.chars().skip(sung_characters).collect();

    let mut spans = Vec::with_capacity(2);
    if !sung_part.is_empty() {
        spans.push(Span::styled(sung_part, theme::SELECTED));
    }
    if !unsung_part.is_empty() {
        spans.push(Span::styled(
            unsung_part,
            Style::new().fg(theme::TEXT_PRIMARY),
        ));
    }
    Line::from(spans)
}

/// Quantizes a normalized `[0.0, 1.0]` spectrum magnitude into a 6-dot Braille column height (`1..=6`).
fn quantize_braille_dots(level: f32) -> u8 {
    if level < 0.06 {
        1
    } else {
        ((level * 6.4).round() as u8).clamp(1, 6)
    }
}

/// Builds a real-time `btop`-style 6-dot-tall Braille soundwave (two stacked lines:
/// 4 bottom dots + 2 upper peak dots per column) driven by the live 72-band FFT audio spectrum.
/// While the stream is `buffering` it lies flat.
fn build_braille_wave_lines(
    width: u16,
    spectrum: &Spectrum,
    buffering: bool,
) -> [Line<'static>; 2] {
    const BRAILLE_LEFT: [u32; 5] = [0x00, 0x40, 0x44, 0x46, 0x47];
    const BRAILLE_RIGHT: [u32; 5] = [0x00, 0x80, 0xa0, 0xb0, 0xb8];

    let cells = (width as usize).clamp(1, SPECTRUM_BANDS / 2);
    if buffering {
        return [
            Line::styled("⠀".repeat(cells), Style::new().fg(theme::TEXT_SECONDARY)),
            Line::styled("⣀".repeat(cells), Style::new().fg(theme::TEXT_SECONDARY)),
        ];
    }

    let mut top_spans = Vec::with_capacity(cells);
    let mut bottom_spans = Vec::with_capacity(cells);

    for cell in 0..cells {
        let left = quantize_braille_dots(spectrum[cell * 2]) as usize;
        let right = quantize_braille_dots(spectrum[cell * 2 + 1]) as usize;

        let bottom_left = left.min(4);
        let bottom_right = right.min(4);
        let bottom_character =
            char::from_u32(0x2800 + BRAILLE_LEFT[bottom_left] + BRAILLE_RIGHT[bottom_right])
                .unwrap_or('⣀');
        let bottom_color = match bottom_left.max(bottom_right) {
            0 | 1 => theme::TEXT_SECONDARY,
            _ => theme::ACCENT,
        };
        bottom_spans.push(Span::styled(
            bottom_character.to_string(),
            Style::new().fg(bottom_color),
        ));

        let top_left = left.saturating_sub(4);
        let top_right = right.saturating_sub(4);
        let top_character =
            char::from_u32(0x2800 + BRAILLE_LEFT[top_left] + BRAILLE_RIGHT[top_right])
                .unwrap_or(' ');
        let top_color = if top_left.max(top_right) >= 2 {
            theme::WAVE_PEAK
        } else {
            theme::ACCENT
        };
        top_spans.push(Span::styled(
            top_character.to_string(),
            Style::new().fg(top_color),
        ));
    }

    [Line::from(top_spans), Line::from(bottom_spans)]
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::lyrics::LyricLine;
    use crate::station::{Station, StationStore};
    use crate::test_util::ScratchDir;

    /// Draws the app on an off-screen terminal and returns its rows as text.
    fn rendered_rows(app: &App) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(60, 5)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                let row: String = (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect();
                row.trim_end().to_string()
            })
            .collect()
    }

    fn show_list(app: &mut App, mode: ListMode, delete_confirm: bool) -> Vec<String> {
        app.view = View::StationList(StationList {
            mode,
            delete_confirm,
        });
        rendered_rows(app)
    }

    #[test]
    fn test_station_list_hint_and_trailing_row_follow_the_mode() {
        let stations = vec![Station::new("Jazz", "http://jazz")];
        let mut app = App::new(StationStore::new(stations, None), None, Default::default());

        assert_eq!(
            show_list(&mut app, ListMode::Play, false),
            [
                "Select a station to play:",
                "press ctrl + s to switch to settings mode",
                "> Jazz",
                "  Other",
                ""
            ]
        );
        let settings = show_list(&mut app, ListMode::Edit, false);
        assert_eq!(settings[1], "press ctrl + s to switch to selection mode");
        assert_eq!(settings[3], "  Custom");
        for mode in [ListMode::Play, ListMode::Edit] {
            assert_eq!(show_list(&mut app, mode, true)[1], "delete radio?");
        }
    }

    #[test]
    fn test_failed_save_is_shown_in_place_of_the_key_hint() {
        // A stations file whose parent "directory" is a regular file cannot be written
        let scratch = ScratchDir::create();
        let blocker = scratch.path().join("not-a-directory");
        fs::write(&blocker, "").unwrap();
        let mut stations = StationStore::new(Vec::new(), Some(blocker.join("stations.json")));
        stations.add(Station::new("Jazz", "http://jazz"));
        let mut app = App::new(stations, None, Default::default());

        let failed = show_list(&mut app, ListMode::Play, false);
        assert!(failed[1].starts_with("could not save stations: "));
        // Right after the message, which Windows' longer wording wraps onto a second row
        let first_station = failed.iter().find(|row| row.starts_with("> "));
        assert_eq!(first_station.map(String::as_str), Some("> Jazz"));
        // A pending delete still asks its question first
        assert_eq!(
            show_list(&mut app, ListMode::Play, true)[1],
            "delete radio?"
        );

        app.view = View::StationSettings(StationEditor::new(app.stations[0].clone()));
        assert!(rendered_rows(&app)[1].starts_with("could not save stations: "));
    }

    #[test]
    fn test_build_lyrics_line_returns_only_current_line() {
        let lines: Vec<LyricLine> = (1..=5)
            .map(|i| LyricLine {
                time: (i * 5) as f64,
                text: format!("Line {i}"),
                words: None,
            })
            .collect();
        let lyrics = Lyrics::from_lines(lines).expect("expected synced lyrics");

        let before = build_lyrics_line(&lyrics, 2.0);
        assert!(before.spans.is_empty());

        let active = build_lyrics_line(&lyrics, 16.5);
        let rendered: String = active
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(rendered, "Line 3");
    }

    #[test]
    fn test_now_playing_layout_and_pause_behavior() {
        let spectrum = [0.0; SPECTRUM_BANDS];
        // Without a song: header + 2 wave lines
        let lines_no_song = build_now_playing_lines(
            "Love Radio 103.7",
            PlaybackStatus::Streaming,
            None,
            None,
            &spectrum,
            80,
        );
        assert_eq!(lines_no_song.len(), 3);
        assert_eq!(lines_no_song[0].spans[0].content, "Streaming: ");
        assert_eq!(lines_no_song[0].spans[1].content, "Love Radio 103.7");

        // With song and no lyrics: header, song, wave (4 lines)
        let lines_with_song = build_now_playing_lines(
            "Love Radio 103.7",
            PlaybackStatus::Streaming,
            Some("Beyond - 灰色轨迹"),
            None,
            &spectrum,
            80,
        );
        assert_eq!(lines_with_song.len(), 4);
        assert_eq!(lines_with_song[0].spans[0].content, "Streaming: ");
        assert_eq!(lines_with_song[1].spans[0].content, "Beyond - 灰色轨迹");

        // Paused mirrors the active layout with only the header label changed
        let live_spectrum = [0.5; SPECTRUM_BANDS];
        let stream_live = build_now_playing_lines(
            "Love Radio 103.7",
            PlaybackStatus::Streaming,
            Some("Beyond - 灰色轨迹"),
            None,
            &live_spectrum,
            80,
        );
        let paused_live = build_now_playing_lines(
            "Love Radio 103.7",
            PlaybackStatus::Paused,
            Some("Beyond - 灰色轨迹"),
            None,
            &live_spectrum,
            80,
        );
        assert_eq!(paused_live.len(), stream_live.len());
        assert_eq!(paused_live[0].spans[0].content, "Paused: ");
        assert_eq!(paused_live[1], stream_live[1]);
        assert_eq!(paused_live[2], stream_live[2]);
        assert_eq!(paused_live[3], stream_live[3]);

        let paused_no_song = build_now_playing_lines(
            "Love Radio 103.7",
            PlaybackStatus::Paused,
            None,
            None,
            &spectrum,
            80,
        );
        assert_eq!(paused_no_song.len(), 3);
        assert_eq!(paused_no_song[0].spans[0].content, "Paused: ");
    }

    #[test]
    fn test_station_settings_custom_display() {
        let editing = |station: &Station, edit: Option<&str>| StationEditor {
            edit: edit.map(|text| TextInput::new(text.to_string())),
            ..StationEditor::new(station.clone())
        };
        let empty_station = Station::default();

        // Unfocused custom station shows "Custom" as title with blank fields
        let lines_custom = build_station_settings_lines(&editing(&empty_station, None), None);
        assert_eq!(lines_custom[0].spans[0].content, "Custom");
        assert_eq!(
            lines_custom[1].spans[0].content,
            "press enter to edit, esc to go back"
        );
        assert_eq!(lines_custom[2].spans[0].content, "> ");
        assert_eq!(lines_custom[2].spans[1].content, "Name: ");
        assert_eq!(lines_custom[2].spans[2].content, "");
        assert_eq!(lines_custom[3].spans[0].content, "  ");
        assert_eq!(lines_custom[3].spans[1].content, "Link: ");
        assert_eq!(lines_custom[3].spans[2].content, "");

        // Active edit field shows the block cursor and save/cancel hint
        let lines_edit = build_station_settings_lines(&editing(&empty_station, Some("Jazz")), None);
        assert_eq!(
            lines_edit[1].spans[0].content,
            "press enter to save, esc to cancel"
        );
        assert_eq!(lines_edit[2].spans[2].content, "Jazz");
        assert_eq!(lines_edit[2].spans[3].content, "█");

        // Named station shows its own name as title
        let named_station = Station::new("Classic FM", "http://classic.fm");
        let lines_named = build_station_settings_lines(&editing(&named_station, None), None);
        assert_eq!(lines_named[0].spans[0].content, "Classic FM");
        assert_eq!(lines_named[2].spans[2].content, "Classic FM");
        assert_eq!(lines_named[3].spans[2].content, "http://classic.fm");

        // Conflict hint replaces the default key hint
        let mut app = App::new(
            StationStore::new(vec![named_station.clone()], None),
            None,
            Default::default(),
        );
        let mut conflict_editor = editing(&named_station, Some("Jazz"));
        conflict_editor.conflict = Some(SettingsField::Name);
        app.view = View::StationSettings(conflict_editor);
        assert_eq!(rendered_rows(&app)[1], DUPLICATE_NAME_HINT);

        if let View::StationSettings(editor) = &mut app.view {
            editor.conflict = Some(SettingsField::Link);
        }
        assert_eq!(rendered_rows(&app)[1], DUPLICATE_LINK_HINT);
    }

    #[test]
    fn test_typed_text_shows_the_cursor_where_it_is() {
        let mut input = TextInput::new("Jazz".to_string());
        input.move_left();
        input.move_left();
        let spans = typed_spans(&input, theme::SELECTED);
        let texts: Vec<&str> = spans.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(texts, ["Ja", "z", "z"]);
        assert_eq!(
            spans[1].style,
            theme::SELECTED.add_modifier(Modifier::REVERSED)
        );
        assert_eq!(spans[2].style, theme::SELECTED);

        // The search prompt draws its query the same way
        let mut search = Search::default();
        search.query = TextInput::new("lofi".to_string());
        search.query.move_left();
        let mut app = App::new(StationStore::new(vec![], None), None, Default::default());
        app.view = View::Search(search);
        assert_eq!(rendered_rows(&app)[0], "Search for a station: lofi");
    }
}
