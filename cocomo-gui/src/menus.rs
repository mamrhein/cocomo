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
//! Provides the main menu structure (Session, File, Edit, Actions,
//! Search, View, Tools, Help) per the UI spec, and registers the global
//! action handlers on the App. Commands whose operations arrive in later
//! phases are present but disabled, so the menu never promises an action
//! that does nothing.

use std::cell::Cell;

use gpui::{
    App, AppContext as _, AsyncApp, BorrowAppContext as _, Entity,
    Focusable as _, KeyBinding, Menu, MenuItem, PathPromptOptions, actions,
    component::{
        WindowExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputState},
        notification::Notification,
        v_flex,
    },
    prelude::*,
};

use crate::{
    log::{self, LogLevel, LogPaneOpen},
    session_manager::GuiSessionManager,
    ui::{CopyToLeft, CopyToRight, NextDifference, PrevDifference, SwapSides},
};

// ---------------------------------------------------------------------------
// Menu actions
// ---------------------------------------------------------------------------

actions!(
    menu,
    [
        NewCompare,
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
        ToggleLogPane,
        SessionSettings,
        ClearSession,
        SessionReport,
        SessionInfo,
        OpenFile,
        OpenClipboard,
        OpenWithAssociatedApp,
        OpenWithTextEdit,
        OpenWithTextPatch,
        SaveFile,
        SaveFileAs,
        NextSection,
        PrevSection,
        FindFilename,
        ReplaceText,
        ShowDetails,
        ShowThumbnails,
        ShowPanels,
        LayoutSideBySide,
        LayoutOverUnder,
        Options,
        MoveTabToNewWindow,
        StopCompare,
        About,
        Quit,
    ]
);

// ---------------------------------------------------------------------------
// Menu key bindings
// ---------------------------------------------------------------------------

/// Register menu-related global key bindings.
///
/// Reload lives on `cmd-shift-r`: plain `cmd-r` stays free for the
/// copy-to-right command the spec assigns it (wired in a later phase).
pub fn menu_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-n", NewCompare, None),
        KeyBinding::new("cmd-o", OpenSession, None),
        KeyBinding::new("cmd-s", SaveSession, None),
        KeyBinding::new("cmd-shift-s", SaveSessionAs, None),
        KeyBinding::new("cmd-w", CloseSession, None),
        KeyBinding::new("cmd-shift-r", ReloadCompare, None),
    ]
}

// ---------------------------------------------------------------------------
// Menu construction
// ---------------------------------------------------------------------------

/// Snapshot of the state the menu bar reflects.
///
/// The native menu bar is a plain data structure, so the window root
/// rebuilds it whenever this snapshot changes rather than on every frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuState {
    /// Whether the active tab is a folder compare (enables the
    /// folder-only commands).
    pub is_folder: bool,
    /// Whether the active tab is the Home start screen (hides the
    /// view-specific menus, which have nothing to act on).
    pub is_home: bool,
    /// Whether the selected entry exists on only one side (enables the
    /// copy-to-side commands).
    pub selected_orphan: bool,
    /// Whether the log pane is visible (checkmark on the View toggle).
    pub log_open: bool,
}

impl MenuState {
    /// The snapshot for a window with no folder tab open.
    pub fn empty() -> Self {
        Self {
            is_folder: false,
            is_home: false,
            selected_orphan: false,
            log_open: true,
        }
    }
}

/// Build the full application menu bar from the current snapshot.
fn build_app_menus(state: MenuState) -> Vec<Menu> {
    let no_folder = !state.is_folder;
    let no_orphan = !(state.is_folder && state.selected_orphan);
    // While Home is active there is no session to save or reload, and the
    // view-specific menus (Edit, Actions, Search) have nothing to act on,
    // so they are hidden entirely. The View menu stays: its log pane
    // toggle works from every view.
    let is_home = state.is_home;

    let mut menus = vec![
        Menu::new("Session").items([
            MenuItem::action("Save session", SaveSession).disabled(is_home),
            MenuItem::action("Save session as…", SaveSessionAs)
                .disabled(is_home),
            MenuItem::action("Session settings…", SessionSettings)
                .disabled(true),
            MenuItem::action("Clear session", ClearSession).disabled(true),
            MenuItem::separator(),
            MenuItem::action("Swap sides", SwapSides).disabled(no_folder),
            MenuItem::action("Reload / recompare files", ReloadCompare)
                .disabled(is_home),
            MenuItem::separator(),
            MenuItem::action("Report…", SessionReport).disabled(true),
            MenuItem::action("Info…", SessionInfo).disabled(true),
        ]),
        Menu::new("File").items([
            MenuItem::action("New compare…", NewCompare),
            MenuItem::action("Open session…", OpenSession),
            MenuItem::separator(),
            MenuItem::action("Open file…", OpenFile).disabled(true),
            MenuItem::action("Open clipboard", OpenClipboard).disabled(true),
            MenuItem::submenu(
                Menu::new("Open with").items([
                    MenuItem::action("Associated app", OpenWithAssociatedApp)
                        .disabled(true),
                    MenuItem::action("Text edit", OpenWithTextEdit)
                        .disabled(true),
                    MenuItem::action("Text patch", OpenWithTextPatch)
                        .disabled(true),
                ]),
            ),
            MenuItem::separator(),
            MenuItem::action("Save file", SaveFile).disabled(true),
            MenuItem::action("Save file as…", SaveFileAs).disabled(true),
            MenuItem::separator(),
            MenuItem::action("Close session", CloseSession),
            MenuItem::action("Close all sessions", CloseAllSessions),
            MenuItem::separator(),
            MenuItem::action("Quit", Quit),
        ]),
    ];

    if !is_home {
        menus.extend([
            Menu::new("Edit").items([
                MenuItem::action("Copy left path", CopyPathLeft)
                    .disabled(no_folder),
                MenuItem::action("Copy right path", CopyPathRight)
                    .disabled(no_folder),
                MenuItem::separator(),
                MenuItem::action("Select all", SelectAllEntries)
                    .disabled(no_folder),
            ]),
            Menu::new("Actions").items([
                // Copying only makes sense for an entry that exists on one
                // side; enabling otherwise would promise a no-op.
                MenuItem::action("Copy to right", CopyToRight)
                    .disabled(no_orphan),
                MenuItem::action("Copy to left", CopyToLeft)
                    .disabled(no_orphan),
            ]),
            Menu::new("Search").items([
                MenuItem::action("Next difference", NextDifference)
                    .disabled(no_folder),
                MenuItem::action("Previous difference", PrevDifference)
                    .disabled(no_folder),
                MenuItem::separator(),
                MenuItem::action("Next difference section", NextSection)
                    .disabled(true),
                MenuItem::action("Previous difference section", PrevSection)
                    .disabled(true),
                MenuItem::separator(),
                MenuItem::action("Find filename…", FindFilename)
                    .disabled(true),
                MenuItem::action("Replace…", ReplaceText).disabled(true),
            ]),
        ]);
    }

    menus.extend([
        Menu::new("View").items([
            // Wired to the text view in a later phase; disabled until it
            // can actually change something.
            MenuItem::action("Word wrap", ToggleWordWrap).disabled(true),
            MenuItem::separator(),
            MenuItem::action("Show details", ShowDetails).disabled(true),
            MenuItem::action("Show thumbnails", ShowThumbnails).disabled(true),
            MenuItem::action("Show panels", ShowPanels).disabled(true),
            MenuItem::action("Side-by-side layout", LayoutSideBySide)
                .disabled(true),
            MenuItem::action("Over/under layout", LayoutOverUnder)
                .disabled(true),
            MenuItem::separator(),
            MenuItem::action("Log pane", ToggleLogPane)
                .checked(state.log_open),
        ]),
        Menu::new("Tools")
            .items([MenuItem::action("Options…", Options).disabled(true)]),
        Menu::new("Help").items([MenuItem::action("About cocomo", About)]),
    ]);
    menus
}

/// Build and set the application menus with no folder tab active.
pub fn set_app_menus(cx: &mut App) {
    let log_open = cx.global::<LogPaneOpen>().0;
    let state = MenuState {
        log_open,
        ..MenuState::empty()
    };
    cx.set_menus(build_app_menus(state));
}

/// Rebuild the menu bar so enabled states and checkmarks reflect the
/// active tab. Called by the window root whenever the snapshot changes.
pub fn refresh_view_menus(state: MenuState, cx: &mut App) {
    cx.set_menus(build_app_menus(state));
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
    let mgr_open = session_manager.clone();
    let mgr_save = session_manager.clone();
    let mgr_save_as = session_manager.clone();
    let mgr_close = session_manager.clone();
    let mgr_close_all = session_manager.clone();

    // File menu — new compare.
    cx.on_action(move |_action: &NewCompare, cx: &mut App| {
        mgr_new.update(cx, |mgr, cx| {
            mgr.add_new_folder_tab(cx);
        });
    });

    // File menu — open session through the native file dialog, then load
    // the picked `.bcs` file and open it as a folder tab.
    cx.on_action(move |_action: &OpenSession, cx: &mut App| {
        let mgr = mgr_open.clone();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open session file".into()),
        });
        cx.spawn(move |cx: &mut AsyncApp| {
            // Clone before the async block: an `async move` over the
            // borrowed `&mut AsyncApp` would capture its lifetime.
            let mut cx = cx.clone();
            async move {
                let paths = match receiver.await {
                    Ok(Ok(Some(paths))) => paths,
                    // Dialog cancelled or failed to open: nothing to do.
                    _ => return,
                };
                let Some(path) = paths.into_iter().next() else {
                    return;
                };
                let task =
                    mgr.update(&mut cx, |m, cx| m.load_session_file(path, cx));
                match task.await {
                    Ok(config) => {
                        cx.update(|cx| {
                            mgr.update(cx, |m, cx| {
                                if let Err(e) =
                                    m.open_session_config(config, cx)
                                {
                                    log::push(
                                        LogLevel::Error,
                                        format!("Open session failed: {e}"),
                                        cx,
                                    );
                                }
                            });
                        });
                    }
                    Err(e) => {
                        let message =
                            format!("Couldn't open the session. {e}");
                        cx.update(|cx| {
                            log::push(LogLevel::Error, message.clone(), cx);
                            notify_error(message, cx);
                        });
                    }
                }
            }
        })
        .detach();
    });

    // Session menu — save. Mark the session clean once the file write
    // succeeded, so the close prompt only appears for genuinely unsaved
    // work.
    cx.on_action(move |_action: &SaveSession, cx: &mut App| {
        let mgr = mgr_save.clone();
        let weak = mgr.downgrade();
        let name = mgr
            .read(cx)
            .active_config()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let task = mgr.update(cx, |m, cx| m.save_active_session(cx));
        cx.spawn(move |cx: &mut AsyncApp| {
            let cx = cx.clone();
            async move {
                match task.await {
                    Ok(()) => {
                        cx.update(|cx| {
                            log::push(
                                LogLevel::Info,
                                format!("Session saved: {name}"),
                                cx,
                            );
                            if let Some(mgr) = weak.upgrade() {
                                mgr.update(cx, |m, cx| m.mark_clean(cx));
                            }
                        });
                    }
                    Err(e) => {
                        let message =
                            format!("Couldn't save the session. {e}");
                        cx.update(|cx| {
                            log::push(LogLevel::Error, message.clone(), cx);
                            notify_error(message, cx);
                        });
                    }
                }
            }
        })
        .detach();
    });

    // Session menu — save as: ask for a name, then persist under it.
    cx.on_action(move |_action: &SaveSessionAs, cx: &mut App| {
        let mgr = mgr_save_as.clone();
        let Some(window) = cx.active_window() else {
            return;
        };
        let default_name = mgr
            .read(cx)
            .active_config()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let _ = window.update(cx, |_, window, cx| {
            // Created before opening: the dialog builder re-runs on every
            // layer rebuild, so persistent entities must live outside it.
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Session name")
                    .default_value(default_name)
            });
            let initial_focus = Cell::new(true);
            window.open_dialog(cx, move |dialog, window, cx| {
                if initial_focus.replace(false) {
                    input.focus_handle(cx).focus(window, cx);
                }
                let save_mgr = mgr.clone();
                let save_input = input.clone();
                dialog
                    .title("Save session as")
                    .child(Input::new(&input))
                    .footer(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("save_as_cancel")
                                    .label("Cancel")
                                    .on_click(|_, window, cx| {
                                        window.close_dialog(cx);
                                    }),
                            )
                            .child(
                                Button::new("save_as_ok")
                                    .label("Save")
                                    .primary()
                                    .on_click(move |_, window, cx| {
                                        let name = save_input
                                            .read(cx)
                                            .value()
                                            .trim()
                                            .to_string();
                                        if !name.is_empty() {
                                            save_session_as(
                                                save_mgr.clone(),
                                                name,
                                                cx,
                                            );
                                        }
                                        window.close_dialog(cx);
                                    }),
                            ),
                    )
            });
        });
    });

    // Session/File menu — close the active session, prompting first when
    // it has unsaved changes.
    cx.on_action(move |_action: &CloseSession, cx: &mut App| {
        let mgr = mgr_close.clone();
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            mgr.update(cx, |m, cx| {
                let index = m.active_index();
                m.request_close_session(index, window, cx);
            });
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
    cx.on_action(move |_action: &SelectAllEntries, cx: &mut App| {
        log::push(LogLevel::Info, "Select all — not yet implemented", cx);
    });

    // View menu — display filters are handled by the focused folder view,
    // which owns the filter state; there is no global handler here.

    cx.on_action(move |_action: &ToggleWordWrap, cx: &mut App| {
        log::push(LogLevel::Info, "Word wrap — not yet implemented", cx);
    });

    // View menu — log pane visibility is a global the window root
    // observes, so the toggle works regardless of which view has focus.
    cx.on_action(move |_action: &ToggleLogPane, cx: &mut App| {
        cx.update_global::<LogPaneOpen, _>(|open, _| open.0 = !open.0);
    });

    // Session menu — reload is handled by the focused folder view; the
    // global handler only records dispatches that reached no view.
    cx.on_action(move |_action: &ReloadCompare, cx: &mut App| {
        log::push(LogLevel::Info, "Reload comparison dispatched", cx);
    });

    // Help menu — about: a real modal naming the product and its version.
    cx.on_action(move |_action: &About, cx: &mut App| {
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            window.open_dialog(cx, |dialog, _, _| {
                dialog
                    .title("About cocomo")
                    .child(
                        v_flex()
                            .gap_2()
                            .child(format!(
                                "cocomo {}",
                                env!("CARGO_PKG_VERSION")
                            ))
                            .child(
                                "COmpare, COpy & MOve directories and files.",
                            )
                            .child("Copyright © 2026 Michael Amrhein")
                            .child(
                                "Distributed under the cocomo license; see \
                                 LICENSE.TXT.",
                            ),
                    )
                    .footer(h_flex().justify_end().child(
                        Button::new("about_ok").label("OK").on_click(
                            |_, window, cx| {
                                window.close_dialog(cx);
                            },
                        ),
                    ))
            });
        });
    });
}

/// Save the active session under `name` and record the outcome in the log.
fn save_session_as(
    mgr: Entity<GuiSessionManager>,
    name: String,
    cx: &mut App,
) {
    let weak = mgr.downgrade();
    let task = mgr.update(cx, |m, cx| m.save_as(name.clone(), cx));
    cx.spawn(move |cx: &mut AsyncApp| {
        let cx = cx.clone();
        async move {
            match task.await {
                Ok(()) => {
                    cx.update(|cx| {
                        log::push(
                            LogLevel::Info,
                            format!("Session saved as: {name}"),
                            cx,
                        );
                        if let Some(mgr) = weak.upgrade() {
                            mgr.update(cx, |m, cx| m.mark_clean(cx));
                        }
                    });
                }
                Err(e) => {
                    let message = format!("Couldn't save the session. {e}");
                    cx.update(|cx| {
                        log::push(LogLevel::Error, message.clone(), cx);
                        notify_error(message, cx);
                    });
                }
            }
        }
    })
    .detach();
}

/// Surface a failed operation as an error notification in the active
/// window, so a background-task failure is never silent.
pub(crate) fn notify_error(message: String, cx: &mut App) {
    // The notification layer belongs to a window; without one there is
    // nowhere to show the message, and the log already recorded it.
    let Some(window) = cx.active_window() else {
        return;
    };
    let _ = window.update(cx, |_, window, cx| {
        window.push_notification(Notification::error(message), cx);
    });
}
