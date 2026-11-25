use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use reqwest::Client;

use super::*;
use crate::station::{Station, StationStore};
use crate::terminal::{Direction, WindowAction};

const MODES: [ListMode; 2] = [ListMode::Play, ListMode::Edit];

const CYCLE_LAYER: Flow = Flow::Window(WindowAction::CycleLayer);

/// An app whose station list lives in memory only.
fn test_app(stations: Vec<Station>) -> App {
    App::new(StationStore::new(stations, None), None, Client::new())
}

/// An app showing "Station 1" and "Station 2" in the given list mode.
fn list_app(mode: ListMode) -> App {
    let mut app = test_app(vec![
        Station::new("Station 1", "http://s1"),
        Station::new("Station 2", "http://s2"),
    ]);
    app.view = View::list(mode);
    app
}

fn press(app: &mut App, code: KeyCode) -> Flow {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn press_ctrl(app: &mut App, letter: char) -> Flow {
    app.handle_key(KeyEvent::new(KeyCode::Char(letter), KeyModifiers::CONTROL))
}

/// Moves the list cursor down to the trailing row ("Other" / "Custom"), one row at a time.
fn go_to_trailing_row(app: &mut App) {
    for _ in 0..app.stations.len() {
        press(app, KeyCode::Down);
    }
}

fn type_text(app: &mut App, text: &str) {
    for character in text.chars() {
        press(app, KeyCode::Char(character));
    }
}

fn in_list(app: &App, mode: ListMode) -> bool {
    matches!(&app.view, View::StationList(list) if list.mode == mode)
}

fn confirming_delete(app: &App) -> bool {
    matches!(&app.view, View::StationList(list) if list.delete_confirm)
}

fn editor(app: &App) -> Option<&StationEditor> {
    match &app.view {
        View::StationSettings(editor) => Some(editor),
        _ => None,
    }
}

/// The station being edited, when the editor is open with no field being typed.
fn idle_draft(app: &App) -> Option<&Station> {
    editor(app)
        .filter(|editor| {
            editor.field == SettingsField::Name
                && editor.edit.is_none()
                && editor.conflict.is_none()
        })
        .map(|editor| &editor.station)
}

fn names(app: &App) -> Vec<&str> {
    app.stations
        .iter()
        .map(|station| station.name.as_str())
        .collect()
}

#[test]
fn test_vim_keys_and_jumps_to_the_ends_are_not_bound() {
    let unbound = [
        KeyCode::Char('j'),
        KeyCode::Char('J'),
        KeyCode::Char('k'),
        KeyCode::Char('K'),
        KeyCode::Char('g'),
        KeyCode::Char('G'),
        KeyCode::Home,
        KeyCode::End,
    ];
    for mode in MODES {
        let mut app = list_app(mode);
        press(&mut app, KeyCode::Down);
        for key in unbound {
            press(&mut app, key);
            assert_eq!(app.cursor, 1, "{key:?} moved the list cursor");
        }
    }

    // The editor's two fields: only w/s, the arrows and Tab switch between them
    let mut app = list_app(ListMode::Edit);
    press(&mut app, KeyCode::Enter);
    for key in unbound {
        press(&mut app, key);
        assert_eq!(
            editor(&app).map(|editor| editor.field),
            Some(SettingsField::Name)
        );
    }

    // The search results
    let stations = vec![Station::new("A", "http://a"), Station::new("B", "http://b")];
    let mut app = search_app(vec![], stations);
    for key in [KeyCode::Home, KeyCode::End] {
        press(&mut app, key);
        assert!(matches!(&app.view, View::Search(search) if search.selected == 0));
    }
}

#[test]
fn test_list_navigation_reaches_the_trailing_row_in_both_modes() {
    for mode in MODES {
        let mut app = list_app(mode);
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(app.cursor, 1);
        // The trailing "Other" / "Custom" row sits one past the last station
        press(&mut app, KeyCode::Char('S'));
        assert_eq!(app.cursor, 2);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.cursor, 2);
        press(&mut app, KeyCode::Char('w'));
        assert_eq!(app.cursor, 1);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.cursor, 0);
    }
}

#[test]
fn test_ctrl_s_toggles_list_mode_and_keeps_the_cursor() {
    let mut app = list_app(ListMode::Play);
    go_to_trailing_row(&mut app);
    press_ctrl(&mut app, 's');
    assert!(in_list(&app, ListMode::Edit));
    assert_eq!(app.cursor, 2);
    press_ctrl(&mut app, 'S');
    assert!(in_list(&app, ListMode::Play));
    assert_eq!(app.cursor, 2);
}

#[test]
fn test_delete_needs_a_second_key_press() {
    for mode in MODES {
        let mut app = list_app(mode);
        press(&mut app, KeyCode::Backspace);
        assert!(confirming_delete(&app));
        assert_eq!(app.stations.len(), 2);

        press(&mut app, KeyCode::Delete);
        assert!(!confirming_delete(&app));
        assert!(in_list(&app, mode));
        assert_eq!(names(&app), ["Station 2"]);

        // Deleting the last station leaves the cursor on the trailing row
        press(&mut app, KeyCode::Delete);
        press(&mut app, KeyCode::Delete);
        assert!(app.stations.is_empty());
        assert_eq!(app.cursor, 0);
    }
}

#[test]
fn test_delete_confirmation_is_cancelled_by_any_other_key() {
    for mode in MODES {
        let mut app = list_app(mode);

        // Esc cancels the confirmation instead of quitting
        press(&mut app, KeyCode::Delete);
        assert!(confirming_delete(&app));
        assert_eq!(press(&mut app, KeyCode::Esc), Flow::Continue);
        assert!(!confirming_delete(&app));

        press(&mut app, KeyCode::Delete);
        press(&mut app, KeyCode::Down);
        assert!(!confirming_delete(&app));
        assert_eq!(app.cursor, 1);

        press(&mut app, KeyCode::Delete);
        press_ctrl(&mut app, 's');
        assert!(!confirming_delete(&app));
        assert!(in_list(&app, mode.toggled()));

        // After a cancel, the next delete key asks again rather than deleting
        press(&mut app, KeyCode::Delete);
        assert!(confirming_delete(&app));
        assert_eq!(app.stations.len(), 2);
        assert_eq!(press(&mut app, KeyCode::Char('q')), Flow::Quit);
    }
}

#[test]
fn test_trailing_row_cannot_be_deleted() {
    for mode in MODES {
        let mut app = list_app(mode);
        go_to_trailing_row(&mut app);
        for key in [KeyCode::Backspace, KeyCode::Delete] {
            press(&mut app, key);
            assert!(!confirming_delete(&app));
        }
        assert_eq!(app.stations.len(), 2);
    }
}

#[test]
fn test_ctrl_c_quits_from_every_view() {
    let mut app = list_app(ListMode::Edit);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    assert!(editor(&app).is_some_and(|editor| editor.edit.is_some()));
    assert_eq!(press_ctrl(&mut app, 'c'), Flow::Quit);
}

#[test]
fn test_enter_in_edit_mode_opens_the_editor_on_a_draft() {
    let mut app = list_app(ListMode::Edit);

    // "Custom" starts from a blank station
    go_to_trailing_row(&mut app);
    press(&mut app, KeyCode::Enter);
    assert_eq!(idle_draft(&app), Some(&Station::default()));

    press(&mut app, KeyCode::Esc);
    assert!(in_list(&app, ListMode::Edit));

    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        idle_draft(&app),
        Some(&Station::new("Station 2", "http://s2"))
    );
    // Nothing is saved until a field is committed
    assert_eq!(names(&app), ["Station 1", "Station 2"]);
}

#[tokio::test]
async fn test_enter_in_play_mode_plays_the_station_or_opens_search() {
    let mut app = list_app(ListMode::Play);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert!(
        matches!(&app.view, View::NowPlaying(playback) if playback.station_name == "Station 2")
    );

    // Stopping returns to the list with the cursor where it was
    press(&mut app, KeyCode::Esc);
    assert!(in_list(&app, ListMode::Play));
    assert_eq!(app.cursor, 1);

    go_to_trailing_row(&mut app);
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.view, View::Search(_)));
}

#[tokio::test]
async fn test_playing_a_listed_station_moves_the_cursor_onto_it() {
    // As when a station is picked on the command line
    let mut app = list_app(ListMode::Play);
    app.play(&Station::new("Station 2", "http://s2"));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.cursor, 1);

    // A station that is not listed leaves the cursor alone
    app.play(&Station::new("Elsewhere", "http://elsewhere"));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.cursor, 1);
}

/// An app on the search prompt with `results` already loaded.
fn search_app(stations: Vec<Station>, results: Vec<Station>) -> App {
    let mut app = test_app(stations);
    app.view = View::Search(Search::showing("radio", results));
    app
}

#[tokio::test]
async fn test_search_selection_adds_a_new_station_and_plays_it() {
    let mut app = search_app(
        vec![Station::new("Station 1", "http://s1")],
        vec![Station::new("New Found Radio", "http://new.radio/stream")],
    );
    press(&mut app, KeyCode::Enter);

    assert!(
        matches!(&app.view, View::NowPlaying(playback) if playback.station_name == "New Found Radio")
    );
    assert_eq!(names(&app), ["Station 1", "New Found Radio"]);
    assert_eq!(app.stations[1].url, "http://new.radio/stream");

    // Stopping playback returns to the list with the new station selected
    press(&mut app, KeyCode::Esc);
    assert!(in_list(&app, ListMode::Play));
    assert_eq!(app.cursor, 1);
}

#[tokio::test]
async fn test_search_selection_of_a_listed_station_plays_it_as_saved() {
    let saved = Station {
        stationuuid: "uuid-jazz".into(),
        ..Station::new("My Jazz", "http://jazz/preferred")
    };
    let found = Station {
        stationuuid: "uuid-jazz".into(),
        ..Station::new("Jazz FM", "http://jazz/directory")
    };
    let mut app = search_app(
        vec![Station::new("Station 1", "http://s1"), saved],
        vec![found],
    );
    press(&mut app, KeyCode::Enter);

    assert!(matches!(&app.view, View::NowPlaying(playback) if playback.station_name == "My Jazz"));
    assert_eq!(names(&app), ["Station 1", "My Jazz"]);
    assert_eq!(app.cursor, 1);
}

#[tokio::test]
async fn test_enter_during_a_search_plays_the_top_result_once_it_arrives() {
    let mut app = search_app(Vec::new(), Vec::new());
    let View::Search(search) = &mut app.view else {
        unreachable!()
    };
    search.awaiting(vec![Station::new("Top Hit", "http://top")]);
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.view, View::Search(_)));

    tokio::task::yield_now().await;
    app.tick().await;
    assert!(matches!(&app.view, View::NowPlaying(playback) if playback.station_name == "Top Hit"));
    assert_eq!(names(&app), ["Top Hit"]);
}

#[test]
fn test_custom_station_is_added_once_both_fields_are_filled() {
    let mut app = test_app(vec![Station::new("Station 1", "http://s1")]);
    press_ctrl(&mut app, 's');
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);

    // Type the name, Tab straight on to the link
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "Custom Jazz");
    press(&mut app, KeyCode::Tab);
    assert_eq!(names(&app), ["Station 1"]);
    type_text(&mut app, "http://jazz.stream");
    press(&mut app, KeyCode::Enter);

    assert_eq!(names(&app), ["Station 1", "Custom Jazz"]);
    assert_eq!(app.stations[1].url, "http://jazz.stream");

    // Later edits change the added station instead of adding another
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "/hq");
    press(&mut app, KeyCode::Enter);
    assert_eq!(names(&app), ["Station 1", "Custom Jazz"]);
    assert_eq!(app.stations[1].url, "http://jazz.stream/hq");

    // Esc returns to settings mode, Ctrl+S to selection mode, both on the new station
    press(&mut app, KeyCode::Esc);
    assert!(in_list(&app, ListMode::Edit));
    assert_eq!(app.cursor, 1);
    press_ctrl(&mut app, 's');
    assert!(in_list(&app, ListMode::Play));
    assert_eq!(app.cursor, 1);
}

#[test]
fn test_edit_existing_station_in_settings() {
    let mut app = test_app(vec![Station::new("Old Name", "http://old.url")]);
    press_ctrl(&mut app, 's');
    press(&mut app, KeyCode::Enter);

    // Edit the name, replacing the old text
    press(&mut app, KeyCode::Enter);
    press_ctrl(&mut app, 'u');
    type_text(&mut app, "New Name");
    press(&mut app, KeyCode::Enter);

    assert_eq!(names(&app), ["New Name"]);
    assert_eq!(app.stations[0].url, "http://old.url");
}

#[test]
fn test_editor_refuses_a_name_or_link_another_station_has() {
    let mut app = list_app(ListMode::Edit);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);

    press(&mut app, KeyCode::Enter);
    press_ctrl(&mut app, 'u');
    type_text(&mut app, "Station 1");
    press(&mut app, KeyCode::Enter);

    // The field stays open with the entry, flagged, and nothing is saved
    let refused = editor(&app).unwrap();
    assert_eq!(refused.conflict, Some(SettingsField::Name));
    assert_eq!(
        refused.edit.as_ref().map(TextInput::as_str),
        Some("Station 1")
    );
    assert_eq!(names(&app), ["Station 1", "Station 2"]);

    // The next key clears the flag; a free name is accepted
    type_text(&mut app, "b");
    assert_eq!(editor(&app).unwrap().conflict, None);
    press(&mut app, KeyCode::Enter);
    assert_eq!(names(&app), ["Station 1", "Station 1b"]);

    // A station keeps its own name and link without conflicting with itself
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        idle_draft(&app).map(|station| station.name.as_str()),
        Some("Station 1b")
    );

    // A new station is checked the same way, as soon as the clashing field is entered
    press(&mut app, KeyCode::Esc);
    go_to_trailing_row(&mut app);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "http://s1");
    press(&mut app, KeyCode::Enter);
    assert_eq!(editor(&app).unwrap().conflict, Some(SettingsField::Link));
    assert_eq!(app.stations.len(), 2);
}

#[tokio::test]
async fn test_arrow_keys_move_window_only_while_streaming_or_paused() {
    let mut app = list_app(ListMode::Play);
    for key in [KeyCode::Left, KeyCode::Right, KeyCode::Up, KeyCode::Down] {
        assert_eq!(press(&mut app, key), Flow::Continue);
    }

    // Playing ("Streaming:") enables window movement on arrow keys
    app.play(&Station::new("Station 1", "http://s1"));
    assert_eq!(
        press(&mut app, KeyCode::Left),
        Flow::Window(WindowAction::Move(Direction::Left))
    );
    assert_eq!(
        press(&mut app, KeyCode::Right),
        Flow::Window(WindowAction::Move(Direction::Right))
    );
    assert_eq!(
        press(&mut app, KeyCode::Up),
        Flow::Window(WindowAction::Move(Direction::Up))
    );
    assert_eq!(
        press(&mut app, KeyCode::Down),
        Flow::Window(WindowAction::Move(Direction::Down))
    );

    // Pausing ("Paused:") keeps arrow keys moving the window
    if let View::NowPlaying(playback) = &mut app.view {
        playback.set_paused(true);
    }
    assert_eq!(
        press(&mut app, KeyCode::Left),
        Flow::Window(WindowAction::Move(Direction::Left))
    );
    assert_eq!(
        press(&mut app, KeyCode::Down),
        Flow::Window(WindowAction::Move(Direction::Down))
    );

    // Stopping playback returns to the list, where arrows no longer move the window
    press(&mut app, KeyCode::Esc);
    assert_eq!(press(&mut app, KeyCode::Left), Flow::Continue);
    assert_eq!(press(&mut app, KeyCode::Down), Flow::Continue);
}

fn playing(app: &App) -> bool {
    matches!(app.view, View::NowPlaying(_))
}

fn typed(app: &App) -> Option<&str> {
    editor(app).and_then(|editor| editor.edit.as_ref().map(TextInput::as_str))
}

#[tokio::test]
async fn test_e_does_what_enter_does_where_letters_are_not_text() {
    // The list: play mode plays the station, and "Other" opens the search prompt
    for key in ['e', 'E'] {
        let mut app = list_app(ListMode::Play);
        assert_eq!(press(&mut app, KeyCode::Char(key)), Flow::Continue);
        assert!(playing(&app));
    }
    let mut app = list_app(ListMode::Play);
    go_to_trailing_row(&mut app);
    press(&mut app, KeyCode::Char('e'));
    assert!(matches!(app.view, View::Search(_)));

    // Edit mode opens the editor, and in the editor "e" opens the field, as Enter does
    let mut app = list_app(ListMode::Edit);
    press(&mut app, KeyCode::Char('e'));
    assert_eq!(
        idle_draft(&app).map(|station| station.name.as_str()),
        Some("Station 1")
    );
    press(&mut app, KeyCode::Char('e'));
    assert_eq!(typed(&app), Some("Station 1"));

    // While a field is typed, "e" is a letter like any other
    press(&mut app, KeyCode::Char('e'));
    press(&mut app, KeyCode::Char('E'));
    assert_eq!(typed(&app), Some("Station 1eE"));

    // And so it is in the search prompt
    let mut app = search_app(vec![], vec![]);
    type_text(&mut app, "ee");
    assert!(matches!(&app.view, View::Search(search) if search.query.as_str() == "radioee"));
}

#[tokio::test]
async fn test_t_cycles_the_window_layer_where_letters_are_not_text() {
    for mode in MODES {
        let mut app = list_app(mode);
        assert_eq!(press(&mut app, KeyCode::Char('t')), CYCLE_LAYER);
        assert_eq!(press(&mut app, KeyCode::Char('T')), CYCLE_LAYER);
        assert!(in_list(&app, mode));

        // Like any other key, it cancels a delete that waits for its second key press
        press(&mut app, KeyCode::Delete);
        assert!(confirming_delete(&app));
        assert_eq!(press(&mut app, KeyCode::Char('t')), CYCLE_LAYER);
        assert!(!confirming_delete(&app));
        assert_eq!(app.stations.len(), 2);
    }

    // The now-playing screen
    let mut app = list_app(ListMode::Play);
    press(&mut app, KeyCode::Enter);
    assert!(playing(&app));
    assert_eq!(press(&mut app, KeyCode::Char('t')), CYCLE_LAYER);
    assert!(playing(&app));

    // The station editor, until a field is typed in it
    let mut app = list_app(ListMode::Edit);
    press(&mut app, KeyCode::Enter);
    assert_eq!(press(&mut app, KeyCode::Char('t')), CYCLE_LAYER);
    press(&mut app, KeyCode::Enter);
    assert_eq!(press(&mut app, KeyCode::Char('t')), Flow::Continue);
    assert_eq!(typed(&app), Some("Station 1t"));

    // The search prompt
    let mut app = search_app(vec![], vec![]);
    assert_eq!(press(&mut app, KeyCode::Char('t')), Flow::Continue);
    assert!(matches!(&app.view, View::Search(search) if search.query.as_str() == "radiot"));
}

#[tokio::test]
async fn test_left_and_right_move_the_cursor_where_text_is_typed() {
    // The station editor: typing and Backspace act at the cursor, which stops at the ends
    let mut app = list_app(ListMode::Edit);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Left);
    type_text(&mut app, "s");
    assert_eq!(typed(&app), Some("Stations 1"));
    for _ in 0..5 {
        press(&mut app, KeyCode::Right);
    }
    press(&mut app, KeyCode::Backspace);
    assert_eq!(typed(&app), Some("Stations "));

    // The search prompt
    let mut app = search_app(vec![], vec![]);
    for _ in 0..6 {
        assert_eq!(press(&mut app, KeyCode::Left), Flow::Continue);
    }
    type_text(&mut app, "x");
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Backspace);
    assert!(matches!(&app.view, View::Search(search) if search.query.as_str() == "xadio"));

    // Home, End, Delete, and Ctrl+U work in Search; other Ctrl combos do not type letters
    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Delete);
    assert!(matches!(&app.view, View::Search(search) if search.query.as_str() == "adio"));
    press(&mut app, KeyCode::End);
    press_ctrl(&mut app, 's');
    assert!(matches!(&app.view, View::Search(search) if search.query.as_str() == "adio"));
    press_ctrl(&mut app, 'u');
    assert!(matches!(&app.view, View::Search(search) if search.query.as_str().is_empty()));
}

#[tokio::test]
async fn test_ctrl_letter_combos_do_not_trigger_plain_shortcuts() {
    let mut app = list_app(ListMode::Play);
    assert_eq!(press_ctrl(&mut app, 'q'), Flow::Continue);
    assert_eq!(press_ctrl(&mut app, 't'), Flow::Continue);
    assert_eq!(press_ctrl(&mut app, 'e'), Flow::Continue);
    assert!(in_list(&app, ListMode::Play));

    app.play(&Station::new("Station 1", "http://s1"));
    assert_eq!(press_ctrl(&mut app, 'q'), Flow::Continue);
    assert_eq!(press_ctrl(&mut app, 't'), Flow::Continue);
    assert!(playing(&app));
}
