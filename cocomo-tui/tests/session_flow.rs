// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! State-machine integration tests.
//!
//! These drive the public surface of the `cocomo_tui` library the way the
//! controller does — key events in, [`SessionAction`]s and state transitions
//! out — without a terminal or the async runtime. They only use the pure
//! modules (`state` and `keys`); the async `cocomo-lib` calls are exercised
//! by their own tests in `cocomo-lib`.

use std::sync::Arc;

use cocomo_lib::{
    DirComparison, DirEntry, DirEntryStatus, Endpoint, EntryInfo,
    SyncOperation, SyncResult,
};
use cocomo_tui::{
    keys::{Cmd, KeyScope, handle_session_key, lookup},
    state::{
        AppState, NamePattern, SessionAction, SessionMode, SessionView,
        SyncStage,
    },
};
use ratatui_kit::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn press(session: &mut SessionView, code: KeyCode) -> Option<SessionAction> {
    handle_session_key(session, key(code))
}

/// Dir session with one directory and one left-only file entry.
///
/// Entry `path`s are relative to the comparison root, exactly as the scanner
/// produces them, so opening an entry must join them onto the endpoint path.
fn dir_session() -> SessionView {
    let mut session = SessionView::new_dir_compare(
        Endpoint::local("/base/left"),
        Endpoint::local("/base/right"),
        false,
    );
    session.set_comparison(Some(DirComparison {
        entries: vec![
            DirEntry {
                name: "sub".into(),
                status: DirEntryStatus::Different,
                left: Some(EntryInfo {
                    name: "sub".into(),
                    path: "sub".into(),
                    is_dir: true,
                    ..Default::default()
                }),
                right: Some(EntryInfo {
                    name: "sub".into(),
                    path: "sub".into(),
                    is_dir: true,
                    ..Default::default()
                }),
                center: None,
                sub_entries: None,
            },
            DirEntry {
                name: "file.txt".into(),
                status: DirEntryStatus::LeftOnly,
                left: Some(EntryInfo {
                    name: "file.txt".into(),
                    path: "file.txt".into(),
                    is_dir: false,
                    ..Default::default()
                }),
                right: None,
                center: None,
                sub_entries: None,
            },
        ],
        ..Default::default()
    }));
    session
}

/// Dir session whose `sub` directory carries nested children, mirroring
/// what a full-depth scan materializes in `sub_entries`.
fn tree_session() -> SessionView {
    let mut session = SessionView::new_dir_compare(
        Endpoint::local("/base/left"),
        Endpoint::local("/base/right"),
        false,
    );
    let file = |name: &str, path: &str| DirEntry {
        name: name.into(),
        status: DirEntryStatus::LeftOnly,
        left: Some(EntryInfo {
            name: name.into(),
            path: path.into(),
            is_dir: false,
            ..Default::default()
        }),
        right: None,
        center: None,
        sub_entries: None,
    };
    session.set_comparison(Some(DirComparison {
        entries: vec![
            DirEntry {
                name: "sub".into(),
                status: DirEntryStatus::Different,
                left: Some(EntryInfo {
                    name: "sub".into(),
                    path: "sub".into(),
                    is_dir: true,
                    ..Default::default()
                }),
                right: Some(EntryInfo {
                    name: "sub".into(),
                    path: "sub".into(),
                    is_dir: true,
                    ..Default::default()
                }),
                center: None,
                sub_entries: Some(Arc::new(DirComparison {
                    entries: vec![
                        file("a.txt", "sub/a.txt"),
                        file("b.txt", "sub/b.txt"),
                    ],
                    ..Default::default()
                })),
            },
            file("top.txt", "top.txt"),
        ],
        ..Default::default()
    }));
    session
}

#[test]
fn enter_on_directory_entry_requests_open_dir() {
    let mut session = dir_session();
    session.ui.table_state.select(Some(0));
    assert_eq!(
        press(&mut session, KeyCode::Enter),
        Some(SessionAction::OpenDir {
            left: Endpoint::local("/base/left/sub"),
            right: Endpoint::local("/base/right/sub"),
        })
    );
}

#[test]
fn enter_on_file_entry_requests_open_file() {
    let mut session = dir_session();
    session.ui.table_state.select(Some(1));
    assert_eq!(
        press(&mut session, KeyCode::Enter),
        Some(SessionAction::OpenFile {
            left: Endpoint::local("/base/left/file.txt"),
            // The entry is absent on the right; the handler falls back to
            // the joined right path.
            right: Endpoint::local("/base/right/file.txt"),
        })
    );
}

#[test]
fn sync_operation_cycles_forward_and_back() {
    let mut session = dir_session();
    assert_eq!(
        session.dir().unwrap().sync_operation,
        SyncOperation::default()
    );

    // A full forward cycle returns to the start.
    for _ in 0..8 {
        assert_eq!(
            press(&mut session, KeyCode::Char('m')),
            Some(SessionAction::None)
        );
    }
    assert_eq!(
        session.dir().unwrap().sync_operation,
        SyncOperation::default()
    );

    // One backward step wraps to the last operation.
    assert_eq!(
        press(&mut session, KeyCode::Char('M')),
        Some(SessionAction::None)
    );
    assert_eq!(
        session.dir().unwrap().sync_operation,
        SyncOperation::DeleteOrphans
    );
}

#[test]
fn session_keys_report_their_controller_actions() {
    let mut session = dir_session();
    press(&mut session, KeyCode::Char('m'));
    let op = session.dir().unwrap().sync_operation;
    assert_eq!(
        press(&mut session, KeyCode::Char('S')),
        Some(SessionAction::PlanSync { operation: op })
    );
    assert_eq!(
        press(&mut session, KeyCode::Char('r')),
        Some(SessionAction::Reload)
    );
    assert_eq!(
        press(&mut session, KeyCode::Backspace),
        Some(SessionAction::GoUp)
    );
}

#[test]
fn sync_confirm_scope_leaves_navigation_to_the_modal() {
    let mut session = dir_session();
    // The controller records the plan and enters the confirm mode once
    // the dry-run arrives. The approval modal owns its decision keys (`y`
    // executes, `n`/`Esc` reject) but leaves navigation to the `OptionList`
    // child it renders, which consumes it in the modal's exclusive input
    // layer, so the table behind the modal cannot move.
    session.set_sync(SyncStage::Planned(SyncResult::default()));
    session.ui.mode = SessionMode::SyncConfirm;
    assert_eq!(press(&mut session, KeyCode::Char('j')), None);
    assert_eq!(
        press(&mut session, KeyCode::Char('y')),
        Some(SessionAction::ExecuteSync)
    );
    assert_eq!(session.ui.mode, SessionMode::Normal);
    // Rejecting closes the modal and keeps the preview on screen for
    // closer inspection.
    session.ui.mode = SessionMode::SyncConfirm;
    assert_eq!(press(&mut session, KeyCode::Esc), Some(SessionAction::None));
    assert_eq!(session.ui.mode, SessionMode::Normal);
    assert!(session.dir().unwrap().sync.is_some());
}

#[test]
fn cancel_clears_sync_preview_before_filter() {
    let mut session = dir_session();
    session.ui.name_filter = Some(NamePattern::new("txt").unwrap());

    // Without a preview, Esc clears the filter.
    assert_eq!(press(&mut session, KeyCode::Esc), Some(SessionAction::None));
    assert_eq!(session.ui.name_filter, None);

    // With a preview, Esc drops the preview and keeps the filter; the stage
    // variant (planned vs. executed) makes no difference to the cancel.
    for stage in [
        SyncStage::Planned(SyncResult::default()),
        SyncStage::Executed(SyncResult::default()),
    ] {
        session.ui.name_filter = Some(NamePattern::new("txt").unwrap());
        session.dir_mut().unwrap().sync = Some(stage);
        assert_eq!(
            press(&mut session, KeyCode::Esc),
            Some(SessionAction::None)
        );
        assert!(session.dir().unwrap().sync.is_none());
        assert_eq!(session.ui.name_filter.as_ref().unwrap().pattern, "txt");
    }
}

#[test]
fn editing_scope_preempts_session_commands() {
    let mut session = dir_session();

    // `j` navigates in the normal scope...
    assert_eq!(
        lookup(KeyScope::DirNormal, key(KeyCode::Char('j'))),
        Some(Cmd::NextRow)
    );
    assert_eq!(session.ui.table_state.selected(), Some(0));
    assert_eq!(
        press(&mut session, KeyCode::Char('j')),
        Some(SessionAction::None)
    );
    assert_eq!(session.ui.table_state.selected(), Some(1));

    // ...but while a text-entry mode is active it belongs to the input
    // component, so the session handler must not claim it.
    session.ui.mode = SessionMode::Search {
        query: String::new(),
    };
    assert_eq!(press(&mut session, KeyCode::Char('j')), None);
    // Esc (Editing scope) cancels the mode.
    assert_eq!(press(&mut session, KeyCode::Esc), Some(SessionAction::None));
    assert_eq!(session.ui.mode, SessionMode::Normal);
}

#[test]
fn tree_expands_navigates_and_collapses_in_place() {
    let mut session = tree_session();
    // Collapsed by default: only the top-level rows are visible.
    assert_eq!(session.visible_rows().len(), 2);

    // `+` expands the directory under the cursor in place.
    session.ui.table_state.select(Some(0));
    assert_eq!(
        press(&mut session, KeyCode::Char('+')),
        Some(SessionAction::None)
    );
    assert_eq!(session.visible_rows().len(), 4);

    // Enter on a nested child joins its root-relative path onto the
    // endpoints, exactly like a top-level entry.
    press(&mut session, KeyCode::Char('j'));
    assert_eq!(
        press(&mut session, KeyCode::Enter),
        Some(SessionAction::OpenFile {
            left: Endpoint::local("/base/left/sub/a.txt"),
            right: Endpoint::local("/base/right/sub/a.txt"),
        })
    );

    // Backspace collapses the containing directory and selects its row
    // instead of stepping out to the parent session.
    assert_eq!(
        press(&mut session, KeyCode::Backspace),
        Some(SessionAction::None)
    );
    assert_eq!(session.visible_rows().len(), 2);
    assert_eq!(session.ui.table_state.selected(), Some(0));
}

#[test]
fn tab_lifecycle_through_app_state() {
    let mut app = AppState::new();
    for _ in 0..3 {
        app.add_session(dir_session());
    }
    assert_eq!(app.active_index(), 2);
    assert!(app.active().is_some_and(|s| s.content.is_dir()));

    app.switch_tab(1);
    assert_eq!(app.active_index(), 0); // wraps forward
    app.switch_tab(-1);
    assert_eq!(app.active_index(), 2); // wraps backward

    app.close_active();
    app.close_active();
    app.close_active();
    assert!(app.sessions.is_empty());
    assert!(app.active().is_none());
}

#[test]
fn filter_panel_flow_from_open_to_applied_filter() {
    let mut session = dir_session();

    // `f` opens the panel seeded from the session's current filters.
    assert_eq!(
        press(&mut session, KeyCode::Char('f')),
        Some(SessionAction::None)
    );
    assert!(
        matches!(&session.ui.mode, SessionMode::FilterPanel { draft } if draft.show_same)
    );

    // Type a regex, enable the name filter, and apply.
    for c in "file".chars() {
        press(&mut session, KeyCode::Char(c));
    }
    press(&mut session, KeyCode::Tab); // focus the name-filter toggle
    press(&mut session, KeyCode::Char(' ')); // enable it
    assert_eq!(
        press(&mut session, KeyCode::Enter),
        Some(SessionAction::None)
    );
    assert_eq!(session.ui.mode, SessionMode::Normal);
    assert_eq!(session.ui.name_filter.as_ref().unwrap().pattern, "file");

    // The filter is effective: only `file.txt` remains visible.
    let names: Vec<&str> = session
        .visible_rows()
        .iter()
        .map(|r| r.entry.name.as_str())
        .collect();
    assert_eq!(names, ["file.txt"]);
}

#[test]
fn report_flow_from_open_to_generate_action() {
    let mut session = dir_session();

    // `R` opens the modal; `Tab` reaches the destination field, where a
    // path can be typed; `Enter` dispatches the generation.
    assert_eq!(
        press(&mut session, KeyCode::Char('R')),
        Some(SessionAction::None)
    );
    press(&mut session, KeyCode::Tab);
    for c in "report.json".chars() {
        press(&mut session, KeyCode::Char(c));
    }
    let action = press(&mut session, KeyCode::Enter);
    let Some(SessionAction::GenerateReport {
        format: _,
        config,
        destination,
    }) = action
    else {
        panic!("expected GenerateReport, got {action:?}");
    };
    assert!(config.0.include_orphans);
    assert_eq!(destination, std::path::PathBuf::from("report.json"));
}
