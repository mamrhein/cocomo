// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Root `App` component: menu bar with its dropdown, tab bar, active view,
//! and error modal.

use ratatui_kit::{
    crossterm::event::{
        Event, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
    },
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction, Position, Rect},
        text::Line,
    },
};

use super::{
    dir_compare::DirCompareView,
    filter_panel::FilterPanel,
    frame::tabs_hit_item,
    menu_bar::{
        DIVIDER_WIDTH, MenuBar, MenuDropdown, dropdown_hit_item,
        dropdown_rect, menu_title,
    },
    report_modal::ReportModal,
    session_browser::SessionBrowser,
    settings_modal::SettingsModal,
    sync_confirm::SyncConfirmModal,
    tab_bar::TabBar,
    text_diff::TextCompareView,
    transfer_dialog::TransferDialog,
    welcome::WelcomeScreen,
};
use crate::{
    controller,
    keys::{self, Cmd, KeyScope},
    menu::MenuState,
    state::{APP_STATE, SessionContent},
};

/// Screen position of the menu bar's first cell.
///
/// The bar is the first child of the borderless root view, so it starts at
/// the terminal origin; both the dropdown geometry and the click hit-test
/// are anchored here.
const BAR_ORIGIN: Position = Position { x: 0, y: 0 };

/// What the caller must do after a menu command was run.
///
/// Quit and close-tab are decided by the controller ([`MenuOutcome`]); the
/// shell owns the actual exit call, since only it holds the exit handle.
fn apply_menu_outcome(
    outcome: controller::MenuOutcome,
    exit: &mut impl FnMut(),
) {
    match outcome {
        controller::MenuOutcome::Handled => {}
        controller::MenuOutcome::CloseTab => {
            if controller::close_active_tab() {
                exit();
            }
        }
        controller::MenuOutcome::Quit => exit(),
    }
}

/// Run a menu-navigation command, returning whether `cmd` was one.
///
/// Both the shell handler and the menu layer dispatch through here, so the
/// bar cannot end up in two different states depending on which handler saw
/// the key first.
fn run_menu_command(cmd: Cmd, exit: &mut impl FnMut()) -> bool {
    match cmd {
        Cmd::MenuPrev => controller::switch_menu(-1),
        Cmd::MenuNext => controller::switch_menu(1),
        // `Alt+<accel>` and a click on the bar toggle, so the same title
        // closes the dropdown it just opened.
        Cmd::MenuOpen(index) => controller::toggle_menu(index),
        Cmd::MenuUp => controller::move_menu_item(-1),
        Cmd::MenuDown => controller::move_menu_item(1),
        Cmd::MenuConfirm => {
            apply_menu_outcome(controller::confirm_menu_item(), exit)
        }
        Cmd::MenuClose => controller::close_menu(),
        _ => return false,
    }
    true
}

/// Handle a left click while a dropdown owns the menu input layer.
///
/// A click on a bar title toggles its dropdown (a click on the open title
/// closes it), a click on a list row confirms that row, and anything else
/// dismisses the dropdown without running an item.
fn menu_mouse(
    pos: Position,
    titles: &[Line<'static>],
    dropdown: &Option<(MenuState, Rect)>,
    exit: &mut impl FnMut(),
) {
    if pos.y == BAR_ORIGIN.y {
        match tabs_hit_item(
            pos.x.saturating_sub(BAR_ORIGIN.x),
            titles,
            DIVIDER_WIDTH,
        ) {
            Some(index) => controller::toggle_menu(index),
            None => controller::close_menu(),
        }
        return;
    }
    let Some((menu, rect)) = dropdown else {
        controller::close_menu();
        return;
    };
    // Rows the open menu does not have (below its last item, on the border)
    // only close the dropdown.
    let item = dropdown_hit_item(*rect, pos)
        .filter(|item| menu.menu().is_some_and(|def| *item < def.items.len()));
    match item {
        Some(item) => {
            apply_menu_outcome(controller::pick_menu_item(item), exit)
        }
        None => controller::close_menu(),
    }
}

/// Which view the main content area renders, derived from the active
/// session's [`SessionContent`] (the enum is the single source of truth for
/// the session type).
#[derive(Clone, Copy)]
enum ActiveView {
    Dir,
    Text,
    Welcome,
}

/// Root component: subscribes to global state, registers global key handlers,
/// and renders the menu bar, tab bar, and main content.
#[component]
pub(crate) fn App(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let app_state = hooks.use_atom(&APP_STATE);
    let mut exit = hooks.use_exit();

    // Titles with their accelerator letter underlined. The dropdown
    // geometry and the click hit-test are derived from these very lines, so
    // bar, dropdown and mouse mapping cannot drift apart.
    let menu_titles: Vec<Line<'static>> =
        crate::menu::MENUS.iter().map(menu_title).collect();

    // While a dropdown is open its input layer blocks every handler below
    // (keys and mouse), exactly like a modal blocks the shell. Read the
    // state the layer and its handler need before registering them.
    let (open_menu, dir_active) = {
        let app = app_state.read();
        (app.menu, app.active().is_some_and(|s| s.content.is_dir()))
    };
    let dropdown = open_menu
        .map(|menu| (menu, dropdown_rect(&menu_titles, menu, BAR_ORIGIN)));
    let menu_layer = hooks.use_input_layer(open_menu.is_some(), true);
    {
        let titles = menu_titles.clone();
        let mut menu_exit = hooks.use_exit();
        hooks.use_event_handler(
            EventScope::Layer(menu_layer),
            EventPriority::High,
            move |event| match event {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        return EventResult::Ignored;
                    }
                    // The Menu scope owns navigation, the Global scope the
                    // shell keys that stay useful with an open dropdown
                    // (`Alt+<accel>` switches menus, `?` still toggles the
                    // help bar). Anything else is swallowed, so table and
                    // session commands cannot fire behind the dropdown.
                    let cmd = keys::lookup(KeyScope::Menu, key)
                        .or_else(|| keys::lookup(KeyScope::Global, key));
                    if let Some(cmd) = cmd
                        && !run_menu_command(cmd, &mut menu_exit)
                    {
                        match cmd {
                            Cmd::CloseTab => {
                                if controller::close_active_tab() {
                                    menu_exit();
                                }
                            }
                            Cmd::Quit => menu_exit(),
                            Cmd::ToggleHelp => controller::toggle_help(),
                            _ => {}
                        }
                    }
                    EventResult::Consumed
                }
                Event::Mouse(mouse) => {
                    if matches!(
                        mouse.kind,
                        MouseEventKind::Down(MouseButton::Left)
                    ) {
                        menu_mouse(
                            Position {
                                x: mouse.column,
                                y: mouse.row,
                            },
                            &titles,
                            &dropdown,
                            &mut menu_exit,
                        );
                    }
                    EventResult::Consumed
                }
                _ => EventResult::Ignored,
            },
        );
    }

    // Register the shell key handler. Which keys the shell owns is defined
    // by the `Global` scope of the keymap (`keys::KEYMAP`); the help bar is
    // generated from the same table. All state mutations go through the
    // controller; the atom handle is only used for reads. While a dropdown
    // is open its blocking layer already claimed the key, so only the keys
    // the shell itself owns reach this handler.
    hooks.use_event_handler(
        EventScope::Current,
        EventPriority::Normal,
        move |event| {
            let Event::Key(key) = event else {
                return EventResult::Ignored;
            };
            if key.kind != KeyEventKind::Press {
                return EventResult::Ignored;
            }
            let Some(cmd) = keys::lookup(KeyScope::Global, key) else {
                return EventResult::Ignored;
            };
            if run_menu_command(cmd, &mut exit) {
                return EventResult::Consumed;
            }

            match cmd {
                // Close active tab; quit when it was the last one.
                Cmd::CloseTab => {
                    if controller::close_active_tab() {
                        exit();
                    }
                }
                // Switch tabs (Shift reverses direction).
                Cmd::SwitchTab => {
                    let is_shift = key.modifiers.contains(KeyModifiers::SHIFT);
                    controller::switch_tab(if is_shift { -1 } else { 1 });
                }
                Cmd::Quit => {
                    // Text-entry modes own `q` for their input fields;
                    // their input layers normally block this handler
                    // already, so the guard is belt and braces.
                    if app_state
                        .read()
                        .active()
                        .is_some_and(|s| s.ui.mode.is_editing())
                    {
                        return EventResult::Ignored;
                    }
                    exit();
                }
                Cmd::ToggleHelp => {
                    controller::toggle_help();
                }
                // Session keys are owned by the session view components.
                _ => return EventResult::Ignored,
            }

            EventResult::Consumed
        },
    );

    // Register the global paste handler. `Global` scope runs before the
    // input-layer cutoff, so pastes reach it even while a `SearchInput`
    // owns an exclusive layer (the framework's text fields do not consume
    // `Event::Paste` themselves). With bracketed paste mode enabled at the
    // entry point, a clipboard paste arrives as one event instead of a
    // burst of key presses.
    hooks.use_event_handler(
        EventScope::Global,
        EventPriority::Normal,
        move |event| {
            let Event::Paste(text) = event else {
                return EventResult::Ignored;
            };
            if controller::paste_into_mode(text.as_str()) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    // Read current state for rendering the menu and tab bars.
    let app = app_state.read();
    // Only the title of the open dropdown is highlighted.
    let menu_selected = app.menu.map(|m| m.bar_index);
    let active_index = app.active_index();
    let active_view = match app.active().map(|s| &s.content) {
        Some(SessionContent::Dir(_)) => ActiveView::Dir,
        Some(SessionContent::Text(_)) => ActiveView::Text,
        None => ActiveView::Welcome,
    };

    // Extract error message for the AlertModal (cloned to avoid holding the
    // read guard across element! rendering).
    let error_message = app.error_message.clone();

    // Build session tab titles.
    let tab_titles: Vec<Line<'static>> = app
        .sessions
        .iter()
        .map(|s| Line::from(s.tab_title().to_string()))
        .collect();

    drop(app);

    element!(
        View(flex_direction: Direction::Vertical) {
            MenuBar(
                menu_titles: menu_titles,
                selected: menu_selected,
                on_select: Handler::from(move |index| {
                    controller::toggle_menu(index);
                }),
            )
            TabBar(
                tab_titles: tab_titles,
                active_index: active_index,
                on_select: Handler::from(move |index| {
                    controller::set_active_tab(index);
                }),
            )
            View(height: Constraint::Min(1)) {
                match active_view {
                    ActiveView::Dir => { DirCompareView() }
                    ActiveView::Text => { TextCompareView() }
                    ActiveView::Welcome => { WelcomeScreen() }
                }
            }
            AlertModal(
                open: error_message.is_some(),
                title: Line::from(" Error "),
                message: error_message.unwrap_or_default(),
                close_hint: Line::from("Enter / Esc").centered(),
                on_close: move |_: ()| {
                    controller::dismiss_error();
                },
            )
            // Copy/delete confirmation, driven by the active session's
            // transfer mode.
            TransferDialog()
            // Sync execution approval, driven by the active session's
            // sync-confirm mode.
            SyncConfirmModal()
            // Filter panel, session browser, settings, and report modal,
            // each driven by the active session's mode (at most one is
            // open at a time).
            FilterPanel()
            SessionBrowser()
            SettingsModal()
            ReportModal()
            // Dropdown of the open menu. Last in the tree so the overlay
            // paints over everything else, and positioned in absolute
            // coordinates so it costs the main view no rows.
            if let Some((menu, rect)) = dropdown {
                MenuDropdown(
                    menu: Some(menu),
                    rect: rect,
                    dir_active: dir_active,
                )
            }
        }
    )
}

#[cfg(test)]
mod render_tests {
    use cocomo_lib::{
        DirComparison, DirEntry, DirEntryStatus, Endpoint, EntryInfo,
        SyncResult, TransferAction, TransferItem,
    };
    use ratatui_kit::{ratatui::buffer::Buffer, test_util::render_frames};

    use super::*;
    use crate::{
        state::{
            APP_STATE, AppState, SessionMode, SessionView, SyncStage,
            TransferOp,
        },
        ui::theme::cocomo_palette,
    };

    /// Render the full `App` offscreen and flatten the buffer to text.
    ///
    /// Three frames: the session table windows its rows against the draw
    /// area measured in the previous frame, so a single frame would render
    /// the first-frame placeholder instead of the settled layout.
    fn render(app: AppState) -> String {
        APP_STATE.set(app);
        let buf: Buffer = render_frames(
            element!(PaletteProvider(palette: cocomo_palette()) {
                App()
            }),
            100,
            30,
            3,
        );
        let mut out = String::new();
        for y in 0..buf.area().height {
            for x in 0..buf.area().width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn dir_session_state() -> AppState {
        let mut app = AppState::new();
        app.add_session(SessionView::new_dir_compare(
            Endpoint::local("/left"),
            Endpoint::local("/right"),
            false,
        ));
        app
    }

    #[test]
    fn dir_session_renders_the_table_panel() {
        // Regression: the closed filter panel and report modal used to
        // return a childless `Fragment`, whose default `Fill(1)` height
        // stole the main view's rows and left the content area blank.
        let text = render(dir_session_state());
        assert!(
            text.contains("Loading comparison"),
            "table panel missing from the main view:\n{text}"
        );
        assert!(
            text.contains('┌'),
            "table border missing from the main view:\n{text}"
        );
    }

    #[test]
    fn welcome_screen_renders_the_full_panel() {
        let text = render(AppState::new());
        assert!(
            text.contains("Welcome") && text.contains('└'),
            "welcome panel not rendered at full height:\n{text}"
        );
    }

    #[test]
    fn open_menu_renders_the_dropdown_under_its_title() {
        let mut app = dir_session_state();
        app.menu = MenuState::open(0);
        let screen = render(app);
        let rows: Vec<&str> = screen.lines().collect();
        // Row 0 keeps the bar, row 1 is the top border of the box anchored
        // under the "Sessions" title, row 2 its first item.
        assert!(rows[0].contains("Sessions"), "{rows:?}");
        assert!(rows[1].contains('┌'), "{}", rows[1]);
        assert!(rows[2].contains("open session browser"), "{}", rows[2]);
        assert!(rows[2].contains('▸'), "{}", rows[2]);
        // With no open menu the overlay is gone and the view below it shows
        // through again.
        let closed = render(dir_session_state());
        assert!(!closed.contains("open session browser"), "{closed}");
        assert!(closed.contains("Loading comparison"), "{closed}");
    }

    /// Dir session whose single entry exists on both sides, so the transfer
    /// dialog has a target and every option key applies to it.
    fn sides_state(op: TransferOp) -> AppState {
        let info = |name: &str| EntryInfo {
            name: name.into(),
            path: name.into(),
            is_dir: false,
            ..Default::default()
        };
        let mut session = SessionView::new_dir_compare(
            Endpoint::local("/left"),
            Endpoint::local("/right"),
            false,
        );
        session.set_comparison(Some(DirComparison {
            entries: vec![DirEntry {
                name: "both.txt".into(),
                status: DirEntryStatus::Different,
                left: Some(info("both.txt")),
                right: Some(info("both.txt")),
                center: None,
                sub_entries: None,
            }],
            ..Default::default()
        }));
        session.ui.table_state.select(Some(0));
        session.ui.mode = SessionMode::Transfer { op };
        let mut app = AppState::new();
        app.add_session(session);
        app
    }

    /// The row of the rendered screen containing `needle`.
    fn row_with<'a>(screen: &'a str, needle: &str) -> &'a str {
        screen
            .lines()
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not on screen:\n{screen}"))
    }

    #[test]
    fn transfer_dialog_lists_every_option_with_its_key_code() {
        let screen = render(sides_state(TransferOp::Copy));
        // All four options are listed, each labelled with its bracketed
        // accelerator, and the shared hint line explains the navigation.
        assert!(screen.contains("Copy"), "{screen}");
        for key in ["[l]", "[r]", "[b]", "[Esc]"] {
            assert!(screen.contains(key), "missing {key:?}:\n{screen}");
        }
        assert!(screen.contains("left \u{2192} right"), "{screen}");
        assert!(screen.contains("j/k select"), "{screen}");
        assert!(screen.contains("Enter confirm"), "{screen}");
        // The first option the selection activates carries the cursor and
        // previews its effect; the change summary of that option is listed
        // below the options.
        let cursor_row = row_with(&screen, "left \u{2192} right");
        assert!(cursor_row.contains('\u{25b8}'), "{cursor_row}");
        assert!(cursor_row.contains("(1 file)"), "{cursor_row}");
        assert!(
            screen.contains("copy left \u{2192} right: 1 file"),
            "{screen}"
        );
        assert!(screen.contains("both.txt"), "{screen}");
        // A delete offers `both` under the same key code.
        let deleted = render(sides_state(TransferOp::Delete));
        assert!(deleted.contains("Delete") && deleted.contains("[b]"));
        // A move offers the same arrows and previews the move as its
        // effect.
        let moved = render(sides_state(TransferOp::Move));
        assert!(moved.contains("Move"), "{moved}");
        assert!(moved.contains("left \u{2192} right"), "{moved}");
        assert!(
            moved.contains("move left \u{2192} right: 1 file"),
            "{moved}"
        );
    }

    #[test]
    fn sync_confirm_modal_offers_the_two_decisions() {
        let mut app = dir_session_state();
        {
            let session = app.active_mut().unwrap();
            session.set_sync(SyncStage::Planned(SyncResult::default()));
            session.ui.mode = SessionMode::SyncConfirm;
        }
        let screen = render(app);
        assert!(screen.contains("planned"), "{screen}");
        assert!(screen.contains("[y] Execute"), "{screen}");
        assert!(screen.contains("[n] Cancel"), "{screen}");
        // The cursor starts on Cancel, so a bare `Enter` cannot execute.
        assert!(row_with(&screen, "Cancel").contains('\u{25b8}'));
        assert!(!row_with(&screen, "Execute").contains('\u{25b8}'));
        // Rejecting leaves the modal rendered nowhere.
        let closed = render(dir_session_state());
        assert!(!closed.contains("[y] Execute"), "{closed}");
    }

    #[test]
    fn sync_confirm_modal_lists_the_planned_changes() {
        let mut app = dir_session_state();
        {
            let session = app.active_mut().unwrap();
            session.set_sync(SyncStage::Planned(SyncResult {
                planned: vec![
                    TransferItem::new(
                        TransferAction::CopyRight,
                        "a.txt".into(),
                        false,
                        "a.txt".into(),
                    ),
                    TransferItem::new(
                        TransferAction::DeleteRight,
                        "b.txt".into(),
                        false,
                        "b.txt".into(),
                    ),
                ],
                ..Default::default()
            }));
            session.ui.mode = SessionMode::SyncConfirm;
        }
        let screen = render(app);
        // The totals name the concrete change, not just a count.
        assert!(
            screen.contains("copy left \u{2192} right: 1 file"),
            "{screen}"
        );
        assert!(screen.contains("delete right: 1 file"), "{screen}");
        // The per-item rows list the affected paths.
        assert!(screen.contains("a.txt"), "{screen}");
        assert!(screen.contains("b.txt"), "{screen}");
        assert!(screen.contains("[y] Execute"), "{screen}");
    }

    #[test]
    fn session_browser_lists_the_saved_sessions() {
        let mut app = dir_session_state();
        app.active_mut().unwrap().ui.mode = SessionMode::SessionBrowser {
            saved_sessions: vec!["alpha".into(), "beta".into()],
        };
        let screen = render(app);
        assert!(screen.contains("alpha"), "{screen}");
        assert!(screen.contains("beta"), "{screen}");
        assert!(screen.contains("Enter load"), "{screen}");
        assert!(screen.contains("n new"), "{screen}");
        assert!(row_with(&screen, "alpha").contains('\u{25b8}'));
    }

    #[test]
    fn session_browser_placeholders_while_the_list_is_empty() {
        let mut app = dir_session_state();
        app.active_mut().unwrap().ui.mode = SessionMode::SessionBrowser {
            saved_sessions: Vec::new(),
        };
        let screen = render(app);
        assert!(screen.contains("No saved sessions yet."), "{screen}");
        assert!(!screen.contains('\u{25b8}'), "{screen}");
    }

    #[test]
    fn settings_modal_shows_the_values_and_toggle_keys() {
        let mut app = dir_session_state();
        app.active_mut().unwrap().ui.mode = SessionMode::Settings;
        let screen = render(app);
        assert!(screen.contains("Compare content"), "{screen}");
        assert!(screen.contains("Hide identical"), "{screen}");
        assert!(screen.contains("Sync operation"), "{screen}");
        assert!(screen.contains("Enter toggle"), "{screen}");
        assert!(row_with(&screen, "Compare content").contains('\u{25b8}'));
    }
}

#[cfg(test)]
mod menu_mouse_tests {
    use super::*;
    use crate::{menu::MENUS, state::AppState};

    fn titles() -> Vec<Line<'static>> {
        MENUS.iter().map(menu_title).collect()
    }

    #[test]
    fn a_click_on_a_title_toggles_its_dropdown() {
        let titles = titles();
        let mut exits = 0;
        let mut exit = || exits += 1;
        APP_STATE.set(AppState::new());
        // Tabs pad each title by one cell, so "Compare" (index 1) spans
        // x=13..=21 and "Help" starts at x=54.
        menu_mouse(Position { x: 14, y: 0 }, &titles, &None, &mut exit);
        assert_eq!(
            APP_STATE.state().read().menu.map(|m| m.bar_index),
            Some(1)
        );
        // A second click on the same title closes the dropdown...
        menu_mouse(Position { x: 14, y: 0 }, &titles, &None, &mut exit);
        assert_eq!(APP_STATE.state().read().menu, None);
        // ...and so does a click on the rest of the bar.
        menu_mouse(Position { x: 14, y: 0 }, &titles, &None, &mut exit);
        menu_mouse(Position { x: 80, y: 0 }, &titles, &None, &mut exit);
        assert_eq!(APP_STATE.state().read().menu, None);
        assert_eq!(exits, 0);
    }

    #[test]
    fn a_click_on_a_row_confirms_it() {
        let titles = titles();
        let mut exits = 0;
        let mut exit = || exits += 1;
        APP_STATE.set(AppState::new());
        // "Help" is the last menu and its single item toggles the help bar.
        let help = MENUS.len() - 1;
        let menu = MenuState::open(help).unwrap();
        let rect = dropdown_rect(&titles, menu, BAR_ORIGIN);
        let dropdown = Some((menu, rect));
        APP_STATE.state().write().menu = Some(menu);
        menu_mouse(
            Position {
                x: rect.x + 2,
                y: rect.y + 1,
            },
            &titles,
            &dropdown,
            &mut exit,
        );
        {
            let state = APP_STATE.state();
            let app = state.read();
            assert!(app.show_help, "the help item did not run");
            assert_eq!(app.menu, None);
        }
        // The quit row of the Sessions menu hands the exit request to the
        // shell instead of running a command.
        let menu = MenuState::open(0).unwrap();
        let rect = dropdown_rect(&titles, menu, BAR_ORIGIN);
        let dropdown = Some((menu, rect));
        APP_STATE.set(AppState::new());
        APP_STATE.state().write().menu = Some(menu);
        // The quit row is the fourth item, i.e. the fifth content row of
        // the box (below its top border).
        menu_mouse(
            Position {
                x: rect.x + 2,
                y: rect.y + 4,
            },
            &titles,
            &dropdown,
            &mut exit,
        );
        assert_eq!(exits, 1);
        assert_eq!(APP_STATE.state().read().menu, None);
    }

    #[test]
    fn a_click_outside_the_rows_only_closes_the_dropdown() {
        let titles = titles();
        let mut exits = 0;
        let mut exit = || exits += 1;
        APP_STATE.set(AppState::new());
        let menu = MenuState::open(0).unwrap();
        let rect = dropdown_rect(&titles, menu, BAR_ORIGIN);
        let dropdown = Some((menu, rect));
        APP_STATE.state().write().menu = Some(menu);
        // The top border row is not an item, and neither is the area right
        // of the box.
        for pos in [
            Position {
                x: rect.x + 2,
                y: rect.y,
            },
            Position {
                x: rect.x + rect.width + 1,
                y: rect.y + 1,
            },
        ] {
            menu_mouse(pos, &titles, &dropdown, &mut exit);
            assert_eq!(APP_STATE.state().read().menu, None);
            APP_STATE.state().write().menu = Some(menu);
        }
        // The help bar stayed untouched, i.e. no item ran.
        assert!(!APP_STATE.state().read().show_help);
        assert_eq!(exits, 0);
    }
}
