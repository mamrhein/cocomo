// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Application menu bar definitions and global action handlers.
//!
//! Provides the main menu structure (File, Edit, View, Session,
//! Tools, Help) and registers global action handlers on the App.

use gpui::{App, Entity, KeyBinding, Menu, MenuItem, actions};

use crate::session_manager::GuiSessionManager;

// ---------------------------------------------------------------------------
// Menu actions
// ---------------------------------------------------------------------------

actions!(
    menu,
    [
        NewCompare,
        NewTextCompare,
        OpenSession,
        SaveSession,
        SaveSessionAs,
        CloseSession,
        CloseAllSessions,
        ReloadCompare,
        CopyPathLeft,
        CopyPathRight,
        SelectAllEntries,
        ToggleShowSame,
        ToggleShowDifferent,
        ToggleShowOrphans,
        ToggleWordWrap,
        ToggleLineNumbers,
        Preferences,
        About,
        Quit,
    ]
);

// ---------------------------------------------------------------------------
// Menu key bindings
// ---------------------------------------------------------------------------

/// Register menu-related global key bindings.
pub fn menu_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-n", NewCompare, None),
        KeyBinding::new("cmd-o", OpenSession, None),
        KeyBinding::new("cmd-s", SaveSession, None),
        KeyBinding::new("cmd-shift-s", SaveSessionAs, None),
        KeyBinding::new("cmd-w", CloseSession, None),
        KeyBinding::new("cmd-r", ReloadCompare, None),
    ]
}

// ---------------------------------------------------------------------------
// Menu construction
// ---------------------------------------------------------------------------

/// Build and set the application menus.
pub fn set_app_menus(cx: &mut App) {
    cx.set_menus([
        Menu::new("File").items([
            MenuItem::action("New Compare...", NewCompare),
            MenuItem::separator(),
            MenuItem::action("Open Session...", OpenSession),
            MenuItem::separator(),
            MenuItem::action("Save Session", SaveSession),
            MenuItem::action("Save Session As...", SaveSessionAs),
            MenuItem::separator(),
            MenuItem::action("Close Session", CloseSession),
            MenuItem::action("Close All Sessions", CloseAllSessions),
            MenuItem::separator(),
            MenuItem::action("Quit", Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::action("Copy Left Path", CopyPathLeft),
            MenuItem::action("Copy Right Path", CopyPathRight),
            MenuItem::separator(),
            MenuItem::action("Select All", SelectAllEntries),
        ]),
        Menu::new("View").items([
            MenuItem::action("Show Same", ToggleShowSame),
            MenuItem::action("Show Different", ToggleShowDifferent),
            MenuItem::action("Show Orphans", ToggleShowOrphans),
            MenuItem::separator(),
            MenuItem::action("Toggle Word Wrap", ToggleWordWrap),
            MenuItem::action("Toggle Line Numbers", ToggleLineNumbers),
        ]),
        Menu::new("Session").items([
            MenuItem::action("New Compare...", NewCompare),
            MenuItem::action("Open Session...", OpenSession),
            MenuItem::separator(),
            MenuItem::action("Save Session", SaveSession),
            MenuItem::action("Save Session As...", SaveSessionAs),
            MenuItem::separator(),
            MenuItem::action("Reload Comparison", ReloadCompare),
            MenuItem::separator(),
            MenuItem::action("Close Session", CloseSession),
            MenuItem::action("Close All Sessions", CloseAllSessions),
        ]),
        Menu::new("Tools")
            .items([MenuItem::action("Preferences", Preferences)]),
        Menu::new("Help").items([MenuItem::action("About cocomo", About)]),
    ]);
}

// ---------------------------------------------------------------------------
// Global action handlers
// ---------------------------------------------------------------------------

/// Register all global menu action handlers with the app.
pub fn register_menu_handlers(
    session_manager: Entity<GuiSessionManager>,
    cx: &mut App,
) {
    // Clone session_manager for each closure that needs it.
    let mgr_new = session_manager.clone();
    let mgr_save = session_manager.clone();
    let mgr_close = session_manager.clone();
    let mgr_close_all = session_manager.clone();

    // File menu — new compare.
    cx.on_action(move |_action: &NewCompare, cx: &mut App| {
        mgr_new.update(cx, |mgr, cx| {
            mgr.add_new_folder_tab(cx);
        });
    });

    // File menu — open session.
    cx.on_action(move |_action: &OpenSession, _cx: &mut App| {
        log::info!("open session — file dialog not yet implemented");
    });

    // File menu — save session.
    cx.on_action(move |_action: &SaveSession, cx: &mut App| {
        mgr_save.update(cx, |m, cx| {
            // Detach the save task — fire and forget.
            m.save_active_session(cx).detach();
        });
    });

    // File menu — save as.
    cx.on_action(move |_action: &SaveSessionAs, _cx: &mut App| {
        log::info!("save session as — dialog not yet implemented");
    });

    // File menu — close session.
    cx.on_action(move |_action: &CloseSession, cx: &mut App| {
        mgr_close.update(cx, |mgr, cx| {
            mgr.close_active_session(cx);
        });
    });

    // File menu — close all sessions.
    cx.on_action(move |_action: &CloseAllSessions, cx: &mut App| {
        mgr_close_all.update(cx, |mgr, cx| {
            while !mgr.is_empty() {
                mgr.close_session(0, cx);
            }
        });
    });

    // File menu — quit.
    cx.on_action(move |_action: &Quit, cx: &mut App| {
        cx.quit();
    });

    // Edit menu — copy paths.
    cx.on_action(move |_action: &CopyPathLeft, _cx: &mut App| {
        log::info!("copy left path — clipboard not yet implemented");
    });

    cx.on_action(move |_action: &CopyPathRight, _cx: &mut App| {
        log::info!("copy right path — clipboard not yet implemented");
    });

    cx.on_action(move |_action: &SelectAllEntries, _cx: &mut App| {
        log::info!("select all — not yet implemented");
    });

    // View menu — display filters.
    cx.on_action(move |_action: &ToggleShowSame, _cx: &mut App| {
        log::info!("toggle show same — filter not yet implemented");
    });

    cx.on_action(move |_action: &ToggleShowDifferent, _cx: &mut App| {
        log::info!("toggle show different — filter not yet implemented");
    });

    cx.on_action(move |_action: &ToggleShowOrphans, _cx: &mut App| {
        log::info!("toggle show orphans — filter not yet implemented");
    });

    cx.on_action(move |_action: &ToggleWordWrap, _cx: &mut App| {
        log::info!("toggle word wrap — not yet implemented");
    });

    cx.on_action(move |_action: &ToggleLineNumbers, _cx: &mut App| {
        log::info!("toggle line numbers — not yet implemented");
    });

    // Session menu — reload.
    cx.on_action(move |_action: &ReloadCompare, _cx: &mut App| {
        log::info!("reload comparison dispatched");
    });

    // Tools menu — preferences.
    cx.on_action(move |_action: &Preferences, _cx: &mut App| {
        log::info!("preferences — not yet implemented");
    });

    // Help menu — about.
    cx.on_action(move |_action: &About, _cx: &mut App| {
        log::info!("about dialog — not yet implemented");
    });

    // New text compare (placeholder).
    cx.on_action(move |_action: &NewTextCompare, _cx: &mut App| {
        log::info!("new text compare — not yet implemented");
    });
}
