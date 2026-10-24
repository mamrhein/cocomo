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

use cocomo_lib::{
    DirComparison, DirEntry, DirEntryStatus, Endpoint, EntryInfo,
    SyncOperation, SyncResult,
};
use cocomo_tui::{
    keys::{Cmd, KeyScope, handle_session_key, lookup},
    state::{AppState, SessionAction, SessionMode, SessionView, SyncStage},
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
        press(&mut session, KeyCode::Char('P')),
        Some(SessionAction::PlanSync { operation: op })
    );
    assert_eq!(
        press(&mut session, KeyCode::Char('e')),
        Some(SessionAction::ExecuteSync)
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
fn cancel_clears_sync_preview_before_filter() {
    let mut session = dir_session();
    session.ui.active_filter = Some("txt".into());

    // Without a preview, Esc clears the filter.
    assert_eq!(press(&mut session, KeyCode::Esc), Some(SessionAction::None));
    assert_eq!(session.ui.active_filter, None);

    // With a preview, Esc drops the preview and keeps the filter; the stage
    // variant (planned vs. executed) makes no difference to the cancel.
    for stage in [
        SyncStage::Planned(SyncResult::default()),
        SyncStage::Executed(SyncResult::default()),
    ] {
        session.ui.active_filter = Some("txt".into());
        session.dir_mut().unwrap().sync = Some(stage);
        assert_eq!(
            press(&mut session, KeyCode::Esc),
            Some(SessionAction::None)
        );
        assert!(session.dir().unwrap().sync.is_none());
        assert_eq!(session.ui.active_filter.as_deref(), Some("txt"));
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
    session.ui.mode = SessionMode::Filter;
    assert_eq!(press(&mut session, KeyCode::Char('j')), None);
    // Esc (Editing scope) cancels the mode.
    assert_eq!(press(&mut session, KeyCode::Esc), Some(SessionAction::None));
    assert_eq!(session.ui.mode, SessionMode::Normal);
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
