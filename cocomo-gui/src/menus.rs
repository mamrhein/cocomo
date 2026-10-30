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

use std::{cell::Cell, collections::HashSet};

use gpui::{
    App, AppContext as _, AsyncApp, BorrowAppContext as _, Entity,
    Focusable as _, KeyBinding, Menu, MenuItem, PathPromptOptions, actions,
    component::{
        WindowExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{
            Copy as TextCopy, Cut as TextCut, Delete as TextDelete,
            DeleteToBeginningOfLine, DeleteToEndOfLine, Indent as TextIndent,
            Input, InputState, Outdent as TextOutdent, Paste as TextPaste,
            Redo as TextRedo, SelectAll as TextSelectAll, Undo as TextUndo,
        },
        notification::Notification,
        v_flex,
    },
    prelude::*,
};

use crate::{
    log::{self, LogLevel, LogPaneOpen},
    session_manager::GuiSessionManager,
    text_diff::{
        CompareSelectionToClipboard, ConvertEndingsMac, ConvertEndingsPc,
        ConvertEndingsUnix, ConvertSpacesToTabs, ConvertTabsToSpaces,
        ConvertTrimTrailing, CopyFileLeftNext, CopyFileRightNext, DeleteLine,
        DetailMode, InsertLineAfter, InsertLineBefore, SelectSection,
        SetDetailMode,
    },
    text_merge::{SaveMergeOutput, TakeAncestor, TakeLeft, TakeRight},
    ui::{
        CopyToLeft, CopyToRight, DeleteLeftSide, DeleteRightSide, GoBack,
        GoForward, MoveToLeft, MoveToRight, NextDifference, PrevDifference,
        SwapSides,
    },
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
        SaveWorkspace,
        RestoreWorkspace,
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
        MergeFiles,
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
        ToggleMergeAncestor,
        LayoutSideBySide,
        LayoutOverUnder,
        Options,
        StopCompare,
        FileFormats,
        About,
        Quit,
    ]
);

// ---------------------------------------------------------------------------
// Menu key bindings
// ---------------------------------------------------------------------------

/// Register menu-related global key bindings.
///
/// Reload lives on `cmd-shift-r`: plain `cmd-r` is the copy-to-right
/// command the spec assigns it, bound with the folder-view bindings.
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
    /// Whether the active tab is a text compare (enables the text-only
    /// commands: report, info, sections, wrap, thumbnails).
    pub is_text: bool,
    /// Whether the active tab is a text merge (enables the merge
    /// commands: take, save output, ancestor pane).
    pub is_merge: bool,
    /// Whether the active tab is the Home start screen (hides the
    /// view-specific menus, which have nothing to act on).
    pub is_home: bool,
    /// Whether a comparison or transfer is in flight (enables Stop).
    pub loading: bool,
    /// Whether the selected entry can be copied to the left.
    pub can_copy_left: bool,
    /// Whether the selected entry can be copied to the right.
    pub can_copy_right: bool,
    /// Whether the selected entry can be moved to the left.
    pub can_move_left: bool,
    /// Whether the selected entry can be moved to the right.
    pub can_move_right: bool,
    /// Whether the selected entry can be deleted from the left.
    pub can_delete_left: bool,
    /// Whether the selected entry can be deleted from the right.
    pub can_delete_right: bool,
    /// Whether the pair can step back in the session history.
    pub can_go_back: bool,
    /// Whether the pair can step forward in the session history.
    pub can_go_forward: bool,
    /// Whether the log pane is visible (checkmark on the View toggle).
    pub log_open: bool,
    /// Whether the active text compare wraps long lines (checkmark on
    /// the View toggle).
    pub word_wrap: bool,
    /// Whether the active text compare shows its thumbnail strips
    /// (checkmark on the View toggle).
    pub show_thumbnails: bool,
    /// Whether the active text compare is in edit mode (enables the
    /// text-editing commands).
    pub is_text_editing: bool,
    /// Whether the active text compare has content to save.
    pub can_save_file: bool,
    /// Which detail panes the active text compare shows (checkmarks on
    /// the View menu).
    pub detail_mode: DetailMode,
    /// Whether the active text merge shows its ancestor pane (checkmark
    /// on the View toggle).
    pub show_ancestor: bool,
    /// Whether the active text merge can run a take command.
    pub merge_can_take: bool,
    /// Whether the active text merge has an ancestor (three-way).
    pub merge_has_ancestor: bool,
}

impl MenuState {
    /// The snapshot for a window with no folder tab open.
    pub fn empty() -> Self {
        Self {
            is_folder: false,
            is_text: false,
            is_merge: false,
            is_home: false,
            loading: false,
            can_copy_left: false,
            can_copy_right: false,
            can_move_left: false,
            can_move_right: false,
            can_delete_left: false,
            can_delete_right: false,
            can_go_back: false,
            can_go_forward: false,
            log_open: true,
            word_wrap: false,
            show_thumbnails: false,
            is_text_editing: false,
            can_save_file: false,
            detail_mode: DetailMode::default(),
            show_ancestor: false,
            merge_can_take: false,
            merge_has_ancestor: false,
        }
    }
}

/// Include a menu item unless the user hid its command in the Options
/// dialog (spec section 7.2).
fn show(
    hidden: &HashSet<&'static str>,
    id: &'static str,
    item: impl FnOnce() -> MenuItem,
) -> Option<MenuItem> {
    (!hidden.contains(id)).then(item)
}

/// Flatten the optional items of one menu into the order they appear in.
fn items<const N: usize>(entries: [Option<MenuItem>; N]) -> Vec<MenuItem> {
    entries.into_iter().flatten().collect()
}

/// The ids of every command the user has hidden.
fn hidden_command_ids(cx: &App) -> HashSet<&'static str> {
    let opts = cx.global::<crate::options::AppOptions>();
    crate::commands::all_commands()
        .into_iter()
        .filter(|spec| !opts.command_visible(spec.id))
        .map(|spec| spec.id)
        .collect()
}

/// Build the full application menu bar from the current snapshot.
///
/// `hidden` holds the command ids the user turned off; their items are
/// left out entirely rather than disabled, which is what "hide" means.
fn build_app_menus(
    state: MenuState,
    hidden: &HashSet<&'static str>,
) -> Vec<Menu> {
    let no_folder = !state.is_folder;
    let off = |allowed: bool| no_folder || !allowed;
    // While Home is active there is no session to save or reload, and the
    // view-specific menus (Edit, Actions, Search) have nothing to act on,
    // so they are hidden entirely. The View menu stays: its log pane
    // toggle works from every view.
    let is_home = state.is_home;

    let mut menus = vec![
        Menu::new("Session").items(items([
            show(hidden, "session.save", || {
                MenuItem::action("Save session", SaveSession).disabled(is_home)
            }),
            show(hidden, "session.save_as", || {
                MenuItem::action("Save session as…", SaveSessionAs)
                    .disabled(is_home)
            }),
            show(hidden, "session.settings", || {
                MenuItem::action("Session settings…", SessionSettings)
                    .disabled(is_home)
            }),
            Some(
                MenuItem::action("Clear session", ClearSession).disabled(true),
            ),
            Some(MenuItem::separator()),
            Some(
                MenuItem::action("Back", GoBack)
                    .disabled(off(state.can_go_back)),
            ),
            Some(
                MenuItem::action("Forward", GoForward)
                    .disabled(off(state.can_go_forward)),
            ),
            Some(MenuItem::separator()),
            Some(
                MenuItem::action("Swap sides", SwapSides)
                    .disabled(!(state.is_folder || state.is_text)),
            ),
            show(hidden, "session.reload", || {
                MenuItem::action("Reload / recompare files", ReloadCompare)
                    .disabled(is_home)
            }),
            Some(
                MenuItem::action("Stop comparison", StopCompare)
                    .disabled(no_folder || !state.loading),
            ),
            Some(MenuItem::separator()),
            Some(
                MenuItem::action("Report…", SessionReport)
                    .disabled(!state.is_text),
            ),
            Some(
                MenuItem::action("Info…", SessionInfo)
                    .disabled(!state.is_text),
            ),
            Some(MenuItem::separator()),
            // Merging needs a text compare tab to seed left and right;
            // the ancestor stays empty until three-file merges arrive from
            // the Home card or a folder merge.
            show(hidden, "session.merge", || {
                MenuItem::action("Merge files…", MergeFiles)
                    .disabled(!state.is_text)
            }),
        ])),
        Menu::new("File").items(items([
            show(hidden, "file.new_compare", || {
                MenuItem::action("New compare…", NewCompare)
            }),
            show(hidden, "file.open_session", || {
                MenuItem::action("Open session…", OpenSession)
            }),
            Some(MenuItem::separator()),
            Some(MenuItem::action("Open file…", OpenFile).disabled(true)),
            Some(
                MenuItem::action("Open clipboard", OpenClipboard)
                    .disabled(true),
            ),
            Some(MenuItem::submenu(
                Menu::new("Open with").items([
                    MenuItem::action("Associated app", OpenWithAssociatedApp)
                        .disabled(true),
                    MenuItem::action("Text edit", OpenWithTextEdit)
                        .disabled(true),
                    MenuItem::action("Text patch", OpenWithTextPatch)
                        .disabled(true),
                ]),
            )),
            Some(MenuItem::separator()),
            show(hidden, "file.save_file", || {
                MenuItem::action("Save file", SaveFile)
                    .disabled(!(state.is_text && state.can_save_file))
            }),
            show(hidden, "file.save_file_as", || {
                MenuItem::action("Save file as…", SaveFileAs)
                    .disabled(!(state.is_text && state.can_save_file))
            }),
            Some(MenuItem::separator()),
            show(hidden, "file.close_session", || {
                MenuItem::action("Close session", CloseSession)
            }),
            Some(MenuItem::action("Close all sessions", CloseAllSessions)),
            Some(MenuItem::separator()),
            // Workspace commands act on the whole run, not the active tab,
            // so they stay available from every view.
            show(hidden, "file.save_workspace", || {
                MenuItem::action("Save workspace", SaveWorkspace)
            }),
            show(hidden, "file.restore_workspace", || {
                MenuItem::action("Restore workspace", RestoreWorkspace)
            }),
            Some(MenuItem::separator()),
            Some(MenuItem::action("Quit", Quit)),
        ])),
    ];

    if !is_home {
        // Text-editing commands act on the focused editor and are only
        // meaningful while the text view is in edit mode.
        let edit_off = !(state.is_text && state.is_text_editing);
        menus.extend([
            Menu::new("Edit").items([
                MenuItem::action("Copy left path", CopyPathLeft)
                    .disabled(no_folder),
                MenuItem::action("Copy right path", CopyPathRight)
                    .disabled(no_folder),
                MenuItem::separator(),
                MenuItem::action("Select all entries", SelectAllEntries)
                    .disabled(no_folder),
                MenuItem::separator(),
                MenuItem::action("Undo", TextUndo).disabled(edit_off),
                MenuItem::action("Redo", TextRedo).disabled(edit_off),
                MenuItem::separator(),
                MenuItem::action("Cut", TextCut).disabled(edit_off),
                MenuItem::action("Copy", TextCopy).disabled(edit_off),
                MenuItem::action("Paste", TextPaste).disabled(edit_off),
                MenuItem::action("Delete", TextDelete).disabled(edit_off),
                MenuItem::separator(),
                MenuItem::action("Select all text", TextSelectAll)
                    .disabled(edit_off),
                MenuItem::action("Select section", SelectSection)
                    .disabled(edit_off),
                MenuItem::separator(),
                MenuItem::action("Delete line", DeleteLine).disabled(edit_off),
                MenuItem::action("Delete to line end", DeleteToEndOfLine)
                    .disabled(edit_off),
                MenuItem::action(
                    "Delete to line start",
                    DeleteToBeginningOfLine,
                )
                .disabled(edit_off),
                MenuItem::action("Insert line before", InsertLineBefore)
                    .disabled(edit_off),
                MenuItem::action("Insert line after", InsertLineAfter)
                    .disabled(edit_off),
                MenuItem::action("Indent", TextIndent).disabled(edit_off),
                MenuItem::action("Outdent", TextOutdent).disabled(edit_off),
                MenuItem::separator(),
                MenuItem::submenu(
                    Menu::new("Convert file").items([
                        MenuItem::action(
                            "Trim trailing whitespace",
                            ConvertTrimTrailing,
                        )
                        .disabled(edit_off),
                        MenuItem::action(
                            "Spaces to tabs",
                            ConvertSpacesToTabs,
                        )
                        .disabled(edit_off),
                        MenuItem::action(
                            "Tabs to spaces",
                            ConvertTabsToSpaces,
                        )
                        .disabled(edit_off),
                        MenuItem::separator(),
                        MenuItem::action("Line endings: PC", ConvertEndingsPc)
                            .disabled(edit_off),
                        MenuItem::action(
                            "Line endings: Mac",
                            ConvertEndingsMac,
                        )
                        .disabled(edit_off),
                        MenuItem::action(
                            "Line endings: Unix",
                            ConvertEndingsUnix,
                        )
                        .disabled(edit_off),
                    ]),
                ),
                MenuItem::separator(),
                MenuItem::action(
                    "Compare selection to clipboard",
                    CompareSelectionToClipboard,
                )
                .disabled(!state.is_text),
            ]),
            Menu::new("Actions").items(items([
                // Each reconcile command is enabled only when the
                // selected entry's shape allows it; enabling otherwise
                // would promise a no-op. Overwriting copies, moves, and
                // deletes confirm in the handler before running.
                show(hidden, "actions.copy_right", || {
                    MenuItem::action("Copy to right", CopyToRight)
                        .disabled(off(state.can_copy_right))
                }),
                show(hidden, "actions.copy_left", || {
                    MenuItem::action("Copy to left", CopyToLeft)
                        .disabled(off(state.can_copy_left))
                }),
                Some(MenuItem::separator()),
                show(hidden, "actions.move_right", || {
                    MenuItem::action("Move to right", MoveToRight)
                        .disabled(off(state.can_move_right))
                }),
                show(hidden, "actions.move_left", || {
                    MenuItem::action("Move to left", MoveToLeft)
                        .disabled(off(state.can_move_left))
                }),
                Some(MenuItem::separator()),
                show(hidden, "actions.delete_right", || {
                    MenuItem::action("Delete from right", DeleteRightSide)
                        .disabled(off(state.can_delete_right))
                }),
                show(hidden, "actions.delete_left", || {
                    MenuItem::action("Delete from left", DeleteLeftSide)
                        .disabled(off(state.can_delete_left))
                }),
                Some(MenuItem::separator()),
                // Text-sync commands: copy this pair's file and step the
                // parent folder session to its next difference.
                Some(
                    MenuItem::action(
                        "Copy file to right and open next",
                        CopyFileRightNext,
                    )
                    .disabled(!state.is_text),
                ),
                Some(
                    MenuItem::action(
                        "Copy file to left and open next",
                        CopyFileLeftNext,
                    )
                    .disabled(!state.is_text),
                ),
                Some(MenuItem::separator()),
                // Merge commands act on the focused merge view's selected
                // section; the ancestor command needs a three-way merge.
                Some(
                    MenuItem::action("Take left", TakeLeft)
                        .disabled(!(state.is_merge && state.merge_can_take)),
                ),
                Some(
                    MenuItem::action("Take right", TakeRight)
                        .disabled(!(state.is_merge && state.merge_can_take)),
                ),
                Some(
                    MenuItem::action("Take ancestor", TakeAncestor).disabled(
                        !(state.is_merge
                            && state.merge_can_take
                            && state.merge_has_ancestor),
                    ),
                ),
                Some(MenuItem::separator()),
                Some(
                    MenuItem::action("Save merge output…", SaveMergeOutput)
                        .disabled(!state.is_merge),
                ),
            ])),
            Menu::new("Search").items(items([
                show(hidden, "search.next_difference", || {
                    MenuItem::action("Next difference", NextDifference)
                        .disabled(!(state.is_folder || state.is_text))
                }),
                show(hidden, "search.prev_difference", || {
                    MenuItem::action("Previous difference", PrevDifference)
                        .disabled(!(state.is_folder || state.is_text))
                }),
                Some(MenuItem::separator()),
                Some(
                    MenuItem::action("Next difference section", NextSection)
                        .disabled(!state.is_text),
                ),
                Some(
                    MenuItem::action(
                        "Previous difference section",
                        PrevSection,
                    )
                    .disabled(!state.is_text),
                ),
                Some(MenuItem::separator()),
                Some(
                    MenuItem::action("Find filename…", FindFilename)
                        .disabled(true),
                ),
                Some(MenuItem::action("Replace…", ReplaceText).disabled(true)),
            ])),
        ]);
    }

    menus.extend([
        Menu::new("View").items(items([
            // Word wrap and thumbnails are text-view-only; the focused
            // text or merge view handles the actions.
            show(hidden, "view.word_wrap", || {
                MenuItem::action("Word wrap", ToggleWordWrap)
                    .disabled(!(state.is_text || state.is_merge))
                    .checked(state.word_wrap)
            }),
            Some(
                MenuItem::action("Show ancestor pane", ToggleMergeAncestor)
                    .disabled(!state.is_merge)
                    .checked(state.show_ancestor),
            ),
            Some(MenuItem::separator()),
            // Detail pane modes are text-compare-only; the focused text
            // view handles the actions.
            Some(
                MenuItem::action(
                    "Text details",
                    SetDetailMode {
                        mode: DetailMode::Text,
                    },
                )
                .disabled(!state.is_text)
                .checked(state.detail_mode == DetailMode::Text),
            ),
            Some(
                MenuItem::action(
                    "Hex details",
                    SetDetailMode {
                        mode: DetailMode::Hex,
                    },
                )
                .disabled(!state.is_text)
                .checked(state.detail_mode == DetailMode::Hex),
            ),
            Some(
                MenuItem::action(
                    "Alignment details",
                    SetDetailMode {
                        mode: DetailMode::Alignment,
                    },
                )
                .disabled(!state.is_text)
                .checked(state.detail_mode == DetailMode::Alignment),
            ),
            show(hidden, "view.thumbnails", || {
                MenuItem::action("Show thumbnails", ShowThumbnails)
                    .disabled(!state.is_text)
                    .checked(state.show_thumbnails)
            }),
            Some(MenuItem::action("Show panels", ShowPanels).disabled(true)),
            Some(
                MenuItem::action("Side-by-side layout", LayoutSideBySide)
                    .disabled(true),
            ),
            Some(
                MenuItem::action("Over/under layout", LayoutOverUnder)
                    .disabled(true),
            ),
            Some(MenuItem::separator()),
            show(hidden, "view.log_pane", || {
                MenuItem::action("Log pane", ToggleLogPane)
                    .checked(state.log_open)
            }),
        ])),
        Menu::new("Tools").items(items([
            show(hidden, "tools.options", || {
                MenuItem::action("Options…", Options)
            }),
            show(hidden, "tools.file_formats", || {
                MenuItem::action("File formats…", FileFormats)
            }),
        ])),
        Menu::new("Help").items(items([show(hidden, "help.about", || {
            MenuItem::action("About cocomo", About)
        })])),
    ]);
    menus
}

/// The menu state the bar was last built from, so a global change (a
/// hidden command, a new binding) can rebuild it without waiting for a
/// window to re-render.
#[derive(Clone, Copy)]
struct LastMenuState(MenuState);

impl gpui::Global for LastMenuState {}

/// Build and set the application menus with no folder tab active.
pub fn set_app_menus(cx: &mut App) {
    let log_open = cx.global::<LogPaneOpen>().0;
    let state = MenuState {
        log_open,
        ..MenuState::empty()
    };
    refresh_view_menus(state, cx);
}

/// Rebuild the menu bar so enabled states and checkmarks reflect the
/// active tab. Called by the window root whenever the snapshot changes.
pub fn refresh_view_menus(state: MenuState, cx: &mut App) {
    let hidden = hidden_command_ids(cx);
    cx.set_menus(build_app_menus(state, &hidden));
    cx.set_global(LastMenuState(state));
}

/// Rebuild the menu bar from the last snapshot, e.g. after the user hid a
/// command in the Options dialog.
pub fn rebuild_menus(cx: &mut App) {
    let state = cx
        .try_global::<LastMenuState>()
        .map(|last| last.0)
        .unwrap_or_else(MenuState::empty);
    refresh_view_menus(state, cx);
}

// ---------------------------------------------------------------------------
// Global action handlers
// ---------------------------------------------------------------------------

/// The session manager of the window the command should act on.
///
/// Resolved per dispatch: with one manager per window, a handler that
/// captured a manager at startup would keep acting on the first window.
fn active_manager(cx: &App) -> Option<Entity<GuiSessionManager>> {
    crate::windows::active_manager(cx)
}

/// Register all global menu action handlers with the app.
pub fn register_menu_handlers(cx: &mut App) {
    // File menu — new compare.
    cx.on_action(move |_action: &NewCompare, cx: &mut App| {
        let Some(mgr) = active_manager(cx) else {
            return;
        };
        mgr.update(cx, |mgr, cx| {
            mgr.add_new_folder_tab(cx);
        });
    });

    // File menu — open session through the native file dialog, then load
    // the picked `.bcs` file and open it as a folder tab.
    cx.on_action(move |_action: &OpenSession, cx: &mut App| {
        let Some(mgr) = active_manager(cx) else {
            return;
        };
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
        let Some(mgr) = active_manager(cx) else {
            return;
        };
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
        let Some(mgr) = active_manager(cx) else {
            return;
        };
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
        let Some(mgr) = active_manager(cx) else {
            return;
        };
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

    // File menu — close all sessions of the active window.
    cx.on_action(move |_action: &CloseAllSessions, cx: &mut App| {
        let Some(mgr) = active_manager(cx) else {
            return;
        };
        mgr.update(cx, |mgr, cx| {
            while !mgr.is_empty() {
                mgr.close_session(0, cx);
            }
        });
    });

    // File menu — save the workspace of every open window.
    cx.on_action(move |_action: &SaveWorkspace, cx: &mut App| {
        crate::windows::save_workspace_now(cx);
        log::push(LogLevel::Info, "Workspace saved".to_string(), cx);
    });

    // File menu — replace the open windows with the saved workspace.
    cx.on_action(move |_action: &RestoreWorkspace, cx: &mut App| {
        let restored = crate::windows::restore_saved_workspace(cx);
        if restored == 0 {
            log::push(
                LogLevel::Warning,
                "No saved workspace to restore".to_string(),
                cx,
            );
        }
    });

    // File menu — quit, persisting the workspace first.
    cx.on_action(move |_action: &Quit, cx: &mut App| {
        crate::windows::quit_application(cx);
    });

    // Edit menu — copy paths are handled by the focused folder view, which
    // owns the selection and writes the path to the clipboard.
    cx.on_action(move |_action: &SelectAllEntries, cx: &mut App| {
        log::push(LogLevel::Info, "Select all — not yet implemented", cx);
    });

    // View menu — display filters are handled by the focused folder view,
    // which owns the filter state; there is no global handler here.

    // View menu — word wrap is handled by the focused text view, which
    // owns the wrap state; there is no global handler here.

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

    // File menu — save the focused side's edited buffer.
    cx.on_action(move |_action: &SaveFile, cx: &mut App| {
        let Some(mgr) = active_manager(cx) else {
            return;
        };
        let Some(session) = mgr.read(cx).active_text_session(cx) else {
            return;
        };
        let side = session.read(cx).focus_side();
        session.update(cx, |s, cx| s.save_side(side, false, cx));
    });

    // File menu — save with an explicit target path.
    cx.on_action(move |_action: &SaveFileAs, cx: &mut App| {
        let Some(mgr) = active_manager(cx) else {
            return;
        };
        let Some(session) = mgr.read(cx).active_text_session(cx) else {
            return;
        };
        let side = session.read(cx).focus_side();
        session.update(cx, |s, cx| s.save_side(side, true, cx));
    });

    // Session menu — merge the active text compare's pair in a new merge
    // tab (two-way: no ancestor is known from a plain comparison).
    cx.on_action(move |_action: &MergeFiles, cx: &mut App| {
        let Some(mgr) = active_manager(cx) else {
            return;
        };
        let Some(session) = mgr.read(cx).active_text_session(cx) else {
            return;
        };
        let (left, right) = {
            let session = session.read(cx);
            (session.left_endpoint(), session.right_endpoint())
        };
        mgr.update(cx, |m, cx| {
            m.add_text_merge_tab(left, right, None, cx);
        });
    });

    // Session menu and toolbar Rules button — the session-rules dialog,
    // branched on the active tab type.
    cx.on_action(move |_action: &SessionSettings, cx: &mut App| {
        let Some(mgr) = active_manager(cx) else {
            return;
        };
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            if let Some(session) = mgr.read(cx).active_text_session(cx) {
                crate::text_diff::open_text_rules_dialog(session, window, cx);
            } else if let Some(state) = mgr.read(cx).active_folder_session(cx)
            {
                crate::ui::open_folder_rules_dialog(state, window, cx);
            }
        });
    });

    // Tools menu — the application-options dialog.
    cx.on_action(move |_action: &Options, cx: &mut App| {
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            crate::options::open_options_dialog(window, cx);
        });
    });

    // Tools menu — the file-format registry dialog.
    cx.on_action(move |_action: &FileFormats, cx: &mut App| {
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            crate::file_formats::open_file_formats_dialog(window, cx);
        });
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
