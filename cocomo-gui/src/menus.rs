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

use crate::{
    session_manager::GuiSessionManager,
    state::DisplayFilters,
    ui::{LeaveDir, NextDifference, PrevDifference, SwapSides},
};

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

/// Build the full application menu bar.
///
/// `filters` is the display-filter state of the active folder tab, or
/// `None` when no folder tab is active (a text-diff tab or an empty
/// window). When it is `None` the folder-only items are disabled and the
/// filter checkmarks fall back to all-visible, so the menu never lies
/// about state the user cannot act on.
fn build_app_menus(filters: Option<DisplayFilters>) -> Vec<Menu> {
    let is_folder = filters.is_some();
    let filters = filters.unwrap_or_default();

    vec![
        Menu::new("File").items([
            MenuItem::action("New compare…", NewCompare),
            MenuItem::separator(),
            MenuItem::action("Open session…", OpenSession),
            MenuItem::separator(),
            MenuItem::action("Save session", SaveSession),
            MenuItem::action("Save session as…", SaveSessionAs),
            MenuItem::separator(),
            MenuItem::action("Close session", CloseSession),
            MenuItem::action("Close all sessions", CloseAllSessions),
            MenuItem::separator(),
            MenuItem::action("Quit", Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::action("Copy left path", CopyPathLeft),
            MenuItem::action("Copy right path", CopyPathRight),
            MenuItem::separator(),
            MenuItem::action("Select all", SelectAllEntries),
        ]),
        Menu::new("View").items([
            MenuItem::action("Show same", ToggleShowSame)
                .checked(filters.show_same)
                .disabled(!is_folder),
            MenuItem::action("Show different", ToggleShowDifferent)
                .checked(filters.show_different)
                .disabled(!is_folder),
            MenuItem::action("Show orphans", ToggleShowOrphans)
                .checked(filters.show_orphans)
                .disabled(!is_folder),
            MenuItem::separator(),
            MenuItem::action("Next difference", NextDifference)
                .disabled(!is_folder),
            MenuItem::action("Previous difference", PrevDifference)
                .disabled(!is_folder),
            MenuItem::separator(),
            MenuItem::action("Swap sides", SwapSides).disabled(!is_folder),
            MenuItem::action("Up one level", LeaveDir).disabled(!is_folder),
            MenuItem::separator(),
            MenuItem::action("Toggle word wrap", ToggleWordWrap),
            MenuItem::action("Toggle line numbers", ToggleLineNumbers),
        ]),
        Menu::new("Session").items([
            MenuItem::action("New compare…", NewCompare),
            MenuItem::action("Open session…", OpenSession),
            MenuItem::separator(),
            MenuItem::action("Save session", SaveSession),
            MenuItem::action("Save session as…", SaveSessionAs),
            MenuItem::separator(),
            MenuItem::action("Reload comparison", ReloadCompare),
            MenuItem::separator(),
            MenuItem::action("Close session", CloseSession),
            MenuItem::action("Close all sessions", CloseAllSessions),
        ]),
        Menu::new("Tools")
            .items([MenuItem::action("Preferences", Preferences)]),
        Menu::new("Help").items([MenuItem::action("About cocomo", About)]),
    ]
}

/// Build and set the application menus with no active folder tab.
pub fn set_app_menus(cx: &mut App) {
    cx.set_menus(build_app_menus(None));
}

/// Rebuild the menu bar so the View menu reflects the active tab's filter
/// state (checkmarks) and enables the folder-only commands. Called when
/// the active tab changes.
pub fn refresh_view_menus(filters: Option<DisplayFilters>, cx: &mut App) {
    cx.set_menus(build_app_menus(filters));
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

    // Edit menu — copy paths are handled by the focused folder view, which
    // owns the selection and writes the path to the clipboard.
    cx.on_action(move |_action: &SelectAllEntries, _cx: &mut App| {
        log::info!("select all — not yet implemented");
    });

    // View menu — display filters are handled by the focused folder view,
    // which owns the filter state; there is no global handler here.

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
