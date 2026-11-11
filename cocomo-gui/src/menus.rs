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
//! Provides the unified menu bar (Sessions, File / Folder, Edit, Merge,
//! Search, View, Options, Help) per the menu-structure proposal. The
//! File / Folder entry is titled after the active view, the Merge menu
//! only appears on a merge tab, and a base menu whose items do not apply
//! to the active view is hidden entirely. The module also registers the
//! global action handlers on the App. Commands whose operations arrive
//! in later phases are present but disabled, so the menu never promises
//! an action that does nothing.

use std::{cell::Cell, collections::HashSet};

use cocomo_lib::{DirEntryStatus, Endpoint};
#[cfg(not(target_os = "macos"))]
use gpui::component::menu::AppMenuBar;
use gpui::{
    App, AppContext as _, AsyncApp, BorrowAppContext as _, ElementId, Entity,
    Focusable as _, KeyBinding, Menu, MenuItem, PathPromptOptions,
    SharedString, actions,
    component::{
        WindowExt as _,
        button::{Button, ButtonVariants as _},
        diff::{Diff, DiffFile, DiffState},
        h_flex,
        input::{
            Copy as TextCopy, Cut as TextCut, Delete as TextDelete,
            DeleteToBeginningOfLine, DeleteToEndOfLine, Indent as TextIndent,
            Input, InputState, Outdent as TextOutdent, Paste as TextPaste,
            Redo as TextRedo, SelectAll as TextSelectAll, Undo as TextUndo,
        },
        notification::Notification,
        text::TextView,
        v_flex,
    },
    div,
    prelude::*,
    px,
};

use crate::{
    color::Side,
    log::{self, LogLevel, LogPaneOpen},
    session_manager::GuiSessionManager,
    state::StatusFilter,
    text_diff::{
        AlignWithAnchor, ClearAlignments, CompareSelectionToClipboard,
        ConvertEndingsMac, ConvertEndingsPc, ConvertEndingsUnix,
        ConvertSpacesToTabs, ConvertTabsToSpaces, ConvertTrimTrailing,
        CopyFileLeftNext, CopyFileRightNext, DeleteLine, DetailMode,
        InsertLineAfter, InsertLineBefore, IsolateSelection,
        NextDifferenceFiles, PrevDifferenceFiles, SelectSection,
        SetDetailMode,
    },
    text_merge::{
        SaveMergeOutput, SetMergeAncestor, TakeAncestor, TakeLeft, TakeRight,
    },
    ui::{
        CopyToFolder, CopyToLeft, CopyToRight, DeleteBothSide, DeleteLeftSide,
        DeleteRightSide, ExchangeEntry, GoBack, GoForward, MoveToFolder,
        MoveToLeft, MoveToRight, NextDifference, PrevDifference, SwapSides,
        SyncMirrorLeft, SyncMirrorRight, SyncUpdateBoth, SyncUpdateLeft,
        SyncUpdateRight,
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
        ToggleShowLeftNewer,
        ToggleShowRightNewer,
        ToggleShowOrphans,
        ToggleShowLeftOnly,
        ToggleShowRightOnly,
        ToggleShowHidden,
        ShowAllFilters,
        ExpandDir,
        CollapseDir,
        ExpandAll,
        CollapseAll,
        ToggleWordWrap,
        ToggleLogPane,
        ToggleSingleRowToolbar,
        SessionSettings,
        ClearSession,
        CompareParentFolders,
        CompareUsingText,
        CompareUsingTable,
        CompareUsingHex,
        CompareUsingPicture,
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
        ShowThumbnails,
        ToggleMergeAncestor,
        Options,
        StopCompare,
        FileFormats,
        SyncPresets,
        Profiles,
        HelpContents,
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
/// `alt-/` is the unified search accelerator: the menu bar has no menu
/// accelerators, so it dispatches the Search menu's primary command
/// (next difference) directly.
pub fn menu_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-n", NewCompare, None),
        KeyBinding::new("cmd-o", OpenSession, None),
        KeyBinding::new("cmd-s", SaveSession, None),
        KeyBinding::new("cmd-shift-s", SaveSessionAs, None),
        KeyBinding::new("cmd-w", CloseSession, None),
        KeyBinding::new("cmd-shift-r", ReloadCompare, None),
        KeyBinding::new("alt-/", NextDifference, None),
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
    /// Whether the selected entry can be exchanged between the sides.
    pub can_exchange: bool,
    /// Whether any entry is selected (enables the folder commands that
    /// act on the selection: Copy/Move to folder, Open with ▸).
    pub has_selection: bool,
    /// Whether a comparison is loaded (enables the Synchronize submenu).
    pub can_sync: bool,
    /// Whether the active text compare was opened from a folder session
    /// that is still open (enables Compare Parent Folders).
    pub can_compare_parent: bool,
    /// Whether the pair can step back in the session history.
    pub can_go_back: bool,
    /// Whether the pair can step forward in the session history.
    pub can_go_forward: bool,
    /// Whether the active view offers a file pair that "Compare files
    /// using ▸" can re-open in another view kind (a file-pair tab, or a
    /// folder tab with a file selected).
    pub can_compare_using: bool,
    /// Whether the log pane is visible (checkmark on the View toggle).
    pub log_open: bool,
    /// Whether the selected folder entry is a directory (enables the
    /// View menu's Expand/Collapse directory items).
    pub selected_is_dir: bool,
    /// Whether hidden (dot-prefixed) entries are visible (drives the
    /// View menu's hidden-items Show/Hide label).
    pub show_hidden: bool,
    /// The status show-mask the View menu's filter toggles read and
    /// flip (drives their Show/Hide labels).
    pub show_statuses: DirEntryStatus,
    /// Whether the toolbars render in a single merged row (checkmark on
    /// the View toggle).
    pub single_row_toolbar: bool,
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
            can_exchange: false,
            has_selection: false,
            can_sync: false,
            can_compare_parent: false,
            can_go_back: false,
            can_go_forward: false,
            can_compare_using: false,
            log_open: true,
            selected_is_dir: false,
            show_hidden: true,
            show_statuses: DirEntryStatus::all(),
            single_row_toolbar: false,
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
///
/// The id must exist in the command registry: the registry is the single
/// source for visibility, so an unregistered id would silently always
/// show. The debug assertion catches new menu items whose spec is
/// missing when the menu-building tests run.
fn show(
    hidden: &HashSet<&'static str>,
    id: &'static str,
    item: impl FnOnce() -> MenuItem,
) -> Option<MenuItem> {
    debug_assert!(
        crate::commands::find_command(id).is_some(),
        "menu item {id} is not registered in the command registry"
    );
    (!hidden.contains(id)).then(item)
}

/// Flatten the optional items of one menu into the order they appear in.
fn items<const N: usize>(entries: [Option<MenuItem>; N]) -> Vec<MenuItem> {
    entries.into_iter().flatten().collect()
}

/// Concatenate menu groups, separated by separator lines.
///
/// Empty groups drop out without leaving a dangling line, so a group
/// whose items are all hidden or do not apply to the active view does
/// not fragment the menu.
fn groups(parts: Vec<Vec<MenuItem>>) -> Vec<MenuItem> {
    let mut out: Vec<MenuItem> = Vec::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(MenuItem::separator());
        }
        out.extend(part);
    }
    out
}

/// A submenu item, or `None` when every entry was hidden.
fn submenu(name: &'static str, entries: Vec<MenuItem>) -> Option<MenuItem> {
    (!entries.is_empty())
        .then(|| MenuItem::submenu(Menu::new(name).items(entries)))
}

/// A menu group, or empty when `cond` does not hold.
///
/// Feeds [`groups`], which drops empty groups instead of leaving a
/// dangling separator line.
fn when(cond: bool, part: impl FnOnce() -> Vec<MenuItem>) -> Vec<MenuItem> {
    cond.then(part).unwrap_or_default()
}

/// Label of a View-menu filter toggle.
///
/// The label states the result of invoking the item, so the pair reads
/// `Show …` while the group is filtered out and `Hide …` while it is
/// shown (matching the log-pane toggle; no checkmark is needed).
fn filter_label(shown: bool, group: &str) -> SharedString {
    format!("{} {group} items", if shown { "Hide" } else { "Show" }).into()
}

/// The "Open with" submenu shared by the File / Folder menu's open
/// groups; its commands need a concrete file to act on.
fn open_with_submenu(
    hidden: &HashSet<&'static str>,
    can_open_file: bool,
) -> Option<MenuItem> {
    submenu(
        "Open with",
        items([
            show(hidden, "file.open_with_app", || {
                MenuItem::action("Associated app", OpenWithAssociatedApp)
                    .disabled(!can_open_file)
            }),
            show(hidden, "file.open_with_text_edit", || {
                MenuItem::action("Text edit", OpenWithTextEdit)
                    .disabled(!can_open_file)
            }),
            show(hidden, "file.open_with_text_patch", || {
                MenuItem::action("Text patch", OpenWithTextPatch)
                    .disabled(!can_open_file)
            }),
        ]),
    )
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
/// A base menu whose items do not apply to the active view is omitted
/// from the bar entirely.
fn build_app_menus(
    state: MenuState,
    hidden: &HashSet<&'static str>,
) -> Vec<Menu> {
    let is_home = state.is_home;
    // The Open with commands need a concrete file: the focused pane of a
    // text compare, or the selected entry of a folder compare.
    let can_open_file =
        state.is_text || (state.is_folder && state.has_selection);
    // A tab the File / Folder menu can act on: Home has neither a file
    // pair nor a folder pair, so the menu is hidden there.
    let has_file_view = state.is_folder || state.is_text || state.is_merge;

    // Sessions: every session-related command; Quit closes the run.
    let sessions = Menu::new("Sessions").items(groups(vec![
        items([
            show(hidden, "session.new_compare", || {
                MenuItem::action("New compare…", NewCompare)
            }),
            show(hidden, "session.open", || {
                MenuItem::action("Open session…", OpenSession)
            }),
        ]),
        // The save group needs a live session; Home has none.
        items([
            show(hidden, "session.save", || {
                MenuItem::action("Save session…", SaveSession)
                    .disabled(is_home)
            }),
            show(hidden, "session.save_as", || {
                MenuItem::action("Save session as…", SaveSessionAs)
                    .disabled(is_home)
            }),
            show(hidden, "session.settings", || {
                MenuItem::action("Session settings…", SessionSettings)
                    .disabled(is_home)
            }),
            show(hidden, "session.clear", || {
                // Merge tabs are seeded from a compare pair; clearing
                // one would leave a merge with no sources to take.
                MenuItem::action("Clear session", ClearSession)
                    .disabled(!(state.is_folder || state.is_text))
            }),
        ]),
        items([
            show(hidden, "session.back", || {
                MenuItem::action("Back", GoBack).disabled(!state.can_go_back)
            }),
            show(hidden, "session.forward", || {
                MenuItem::action("Forward", GoForward)
                    .disabled(!state.can_go_forward)
            }),
        ]),
        items([
            show(hidden, "session.swap_sides", || {
                MenuItem::action("Swap sides", SwapSides)
                    .disabled(!(state.is_folder || state.is_text))
            }),
            show(hidden, "session.reload", || {
                MenuItem::action("Reload / recompare files", ReloadCompare)
                    .disabled(is_home)
            }),
            show(hidden, "session.stop", || {
                MenuItem::action("Stop comparison", StopCompare)
                    .disabled(!state.loading)
            }),
        ]),
        items([
            show(hidden, "session.report", || {
                MenuItem::action("Report…", SessionReport)
                    .disabled(!state.is_text)
            }),
            show(hidden, "session.info", || {
                MenuItem::action("Info…", SessionInfo).disabled(!state.is_text)
            }),
        ]),
        items([
            // Merging needs a text compare tab to seed left and right;
            // the ancestor stays empty until three-file merges arrive from
            // the Home card or a folder merge.
            show(hidden, "session.merge", || {
                MenuItem::action("Merge files…", MergeFiles)
                    .disabled(!state.is_text)
            }),
            // The parent link exists only for text tabs opened out of a
            // folder session that is still open.
            show(hidden, "session.compare_parent", || {
                MenuItem::action(
                    "Compare parent folders",
                    CompareParentFolders,
                )
                .disabled(!state.can_compare_parent)
            }),
            // Re-open the active file pair in another compare view; the
            // items are disabled unless a pair is in reach.
            submenu(
                "Compare files using",
                items([
                    show(hidden, "session.compare_as_text", || {
                        MenuItem::action("Text", CompareUsingText)
                            .disabled(!state.can_compare_using)
                    }),
                    show(hidden, "session.compare_as_table", || {
                        MenuItem::action("Table", CompareUsingTable)
                            .disabled(!state.can_compare_using)
                    }),
                    show(hidden, "session.compare_as_hex", || {
                        MenuItem::action("Hex", CompareUsingHex)
                            .disabled(!state.can_compare_using)
                    }),
                    show(hidden, "session.compare_as_picture", || {
                        MenuItem::action("Picture", CompareUsingPicture)
                            .disabled(!state.can_compare_using)
                    }),
                ]),
            ),
        ]),
        items([
            show(hidden, "session.close", || {
                MenuItem::action("Close session", CloseSession)
            }),
            show(hidden, "session.close_all", || {
                MenuItem::action("Close all sessions", CloseAllSessions)
            }),
        ]),
        // Workspace commands act on the whole run, not the active tab,
        // so they stay available from every view.
        items([
            show(hidden, "session.save_workspace", || {
                MenuItem::action("Save workspace", SaveWorkspace)
            }),
            show(hidden, "session.restore_workspace", || {
                MenuItem::action("Restore workspace", RestoreWorkspace)
            }),
        ]),
        items([show(hidden, "session.quit", || {
            MenuItem::action("Quit", Quit)
        })]),
    ]));

    // File / Folder: one bar entry titled after the active view. The
    // open and save groups serve text and merge tabs, the reconcile,
    // to-folder, and sync groups serve folder tabs.
    let file_items = has_file_view.then(|| {
        groups(vec![
            // Open file… retargets the focused pane of a text compare;
            // the Open with commands act on the focused file (text) or
            // the selected entry (folder).
            if state.is_text {
                items([
                    show(hidden, "file.open_file", || {
                        MenuItem::action("Open file…", OpenFile)
                    }),
                    show(hidden, "file.open_clipboard", || {
                        MenuItem::action("Open clipboard", OpenClipboard)
                    }),
                    open_with_submenu(hidden, can_open_file),
                ])
            } else {
                items([open_with_submenu(hidden, can_open_file)])
            },
            // Saving writes the focused side's edited buffer; a merge
            // keeps the group but only its output can be saved so far.
            when(state.is_text || state.is_merge, || {
                items([
                    show(hidden, "file.save_file", || {
                        MenuItem::action("Save file", SaveFile)
                            .disabled(!(state.is_text && state.can_save_file))
                    }),
                    show(hidden, "file.save_file_as", || {
                        MenuItem::action("Save file as…", SaveFileAs)
                            .disabled(!(state.is_text && state.can_save_file))
                    }),
                ])
            }),
            // Each reconcile command is enabled only when the selected
            // entry's shape allows it; enabling otherwise would promise
            // a no-op. Overwriting copies, moves, and deletes confirm in
            // the handler before running.
            when(state.is_folder, || {
                items([
                    submenu(
                        "Copy",
                        items([
                            show(hidden, "file.copy_right", || {
                                MenuItem::action("Left to right", CopyToRight)
                                    .disabled(!state.can_copy_right)
                            }),
                            show(hidden, "file.copy_left", || {
                                MenuItem::action("Right to left", CopyToLeft)
                                    .disabled(!state.can_copy_left)
                            }),
                        ]),
                    ),
                    submenu(
                        "Move",
                        items([
                            show(hidden, "file.move_right", || {
                                MenuItem::action("Left to right", MoveToRight)
                                    .disabled(!state.can_move_right)
                            }),
                            show(hidden, "file.move_left", || {
                                MenuItem::action("Right to left", MoveToLeft)
                                    .disabled(!state.can_move_left)
                            }),
                        ]),
                    ),
                    submenu(
                        "Delete",
                        items([
                            show(hidden, "file.delete_left", || {
                                MenuItem::action("Left", DeleteLeftSide)
                                    .disabled(!state.can_delete_left)
                            }),
                            show(hidden, "file.delete_right", || {
                                MenuItem::action("Right", DeleteRightSide)
                                    .disabled(!state.can_delete_right)
                            }),
                            show(hidden, "file.delete_both", || {
                                MenuItem::action("Both", DeleteBothSide)
                                    .disabled(
                                        !(state.can_delete_left
                                            && state.can_delete_right),
                                    )
                            }),
                        ]),
                    ),
                    show(hidden, "file.exchange", || {
                        MenuItem::action("Exchange", ExchangeEntry)
                            .disabled(!state.can_exchange)
                    }),
                ])
            }),
            when(state.is_folder, || {
                items([
                    show(hidden, "file.copy_to_folder", || {
                        MenuItem::action("Copy to folder…", CopyToFolder)
                            .disabled(!state.has_selection)
                    }),
                    show(hidden, "file.move_to_folder", || {
                        MenuItem::action("Move to folder…", MoveToFolder)
                            .disabled(!state.has_selection)
                    }),
                ])
            }),
            // Whole-comparison sync runs; each confirms first because
            // mirror deletes the target side's extras.
            when(state.is_folder, || {
                items([
                    submenu(
                        "Update",
                        items([
                            show(hidden, "file.sync_update_left", || {
                                MenuItem::action("Left", SyncUpdateLeft)
                                    .disabled(!state.can_sync)
                            }),
                            show(hidden, "file.sync_update_right", || {
                                MenuItem::action("Right", SyncUpdateRight)
                                    .disabled(!state.can_sync)
                            }),
                            show(hidden, "file.sync_update_both", || {
                                MenuItem::action("Both", SyncUpdateBoth)
                                    .disabled(!state.can_sync)
                            }),
                        ]),
                    ),
                    submenu(
                        "Mirror",
                        items([
                            show(hidden, "file.sync_mirror_left", || {
                                MenuItem::action("Left", SyncMirrorLeft)
                                    .disabled(!state.can_sync)
                            }),
                            show(hidden, "file.sync_mirror_right", || {
                                MenuItem::action("Right", SyncMirrorRight)
                                    .disabled(!state.can_sync)
                            }),
                        ]),
                    ),
                ])
            }),
            // Text-sync commands: copy this pair's file and step the
            // parent folder session to its next difference.
            when(state.is_text, || {
                items([
                    show(hidden, "file.copy_file_right_next", || {
                        MenuItem::action(
                            "Copy file to right and open next",
                            CopyFileRightNext,
                        )
                    }),
                    show(hidden, "file.copy_file_left_next", || {
                        MenuItem::action(
                            "Copy file to left and open next",
                            CopyFileLeftNext,
                        )
                    }),
                ])
            }),
        ])
    });
    let mut menus = vec![sessions];
    if let Some(items) = file_items {
        let title = if state.is_folder { "Folder" } else { "File" };
        menus.push(Menu::new(title).items(items));
    }

    // The Edit menu is view-specific: folder views offer path copying and
    // entry selection, text views the full editing set; merge and home
    // views have nothing to edit, so the menu stays hidden there.
    if state.is_folder {
        menus.push(Menu::new("Edit").items(groups(vec![
            items([
                show(hidden, "edit.copy_path_left", || {
                    MenuItem::action("Copy left path", CopyPathLeft)
                }),
                show(hidden, "edit.copy_path_right", || {
                    MenuItem::action("Copy right path", CopyPathRight)
                }),
            ]),
            items([show(hidden, "edit.select_all_entries", || {
                MenuItem::action("Select all entries", SelectAllEntries)
            })]),
        ])));
    } else if state.is_text {
        // Text-editing commands act on the focused editor and are only
        // meaningful while the text view is in edit mode.
        let edit_off = !(state.is_text && state.is_text_editing);
        menus.push(Menu::new("Edit").items(groups(vec![
            items([
                show(hidden, "edit.undo", || {
                    MenuItem::action("Undo", TextUndo).disabled(edit_off)
                }),
                show(hidden, "edit.redo", || {
                    MenuItem::action("Redo", TextRedo).disabled(edit_off)
                }),
            ]),
            items([
                show(hidden, "edit.cut", || {
                    MenuItem::action("Cut", TextCut).disabled(edit_off)
                }),
                show(hidden, "edit.copy", || {
                    MenuItem::action("Copy", TextCopy).disabled(edit_off)
                }),
                show(hidden, "edit.paste", || {
                    MenuItem::action("Paste", TextPaste).disabled(edit_off)
                }),
                show(hidden, "edit.delete", || {
                    MenuItem::action("Delete", TextDelete).disabled(edit_off)
                }),
            ]),
            items([
                show(hidden, "edit.select_all_text", || {
                    MenuItem::action("Select all text", TextSelectAll)
                        .disabled(edit_off)
                }),
                show(hidden, "edit.select_section", || {
                    MenuItem::action("Select section", SelectSection)
                        .disabled(edit_off)
                }),
            ]),
            items([
                show(hidden, "edit.delete_line", || {
                    MenuItem::action("Delete line", DeleteLine)
                        .disabled(edit_off)
                }),
                show(hidden, "edit.delete_to_end", || {
                    MenuItem::action("Delete to line end", DeleteToEndOfLine)
                        .disabled(edit_off)
                }),
                show(hidden, "edit.delete_to_start", || {
                    MenuItem::action(
                        "Delete to line start",
                        DeleteToBeginningOfLine,
                    )
                    .disabled(edit_off)
                }),
                show(hidden, "edit.insert_before", || {
                    MenuItem::action("Insert line before", InsertLineBefore)
                        .disabled(edit_off)
                }),
                show(hidden, "edit.insert_after", || {
                    MenuItem::action("Insert line after", InsertLineAfter)
                        .disabled(edit_off)
                }),
                show(hidden, "edit.indent", || {
                    MenuItem::action("Indent", TextIndent).disabled(edit_off)
                }),
                show(hidden, "edit.outdent", || {
                    MenuItem::action("Outdent", TextOutdent).disabled(edit_off)
                }),
            ]),
            submenu(
                "Convert file",
                items([
                    show(hidden, "edit.convert_trim", || {
                        MenuItem::action(
                            "Trim trailing whitespace",
                            ConvertTrimTrailing,
                        )
                        .disabled(edit_off)
                    }),
                    show(hidden, "edit.convert_spaces_to_tabs", || {
                        MenuItem::action(
                            "Spaces to tabs",
                            ConvertSpacesToTabs,
                        )
                        .disabled(edit_off)
                    }),
                    show(hidden, "edit.convert_tabs_to_spaces", || {
                        MenuItem::action(
                            "Tabs to spaces",
                            ConvertTabsToSpaces,
                        )
                        .disabled(edit_off)
                    }),
                    Some(MenuItem::separator()),
                    show(hidden, "edit.convert_endings_pc", || {
                        MenuItem::action(
                            "Line endings: PC",
                            ConvertEndingsPc,
                        )
                        .disabled(edit_off)
                    }),
                    show(hidden, "edit.convert_endings_mac", || {
                        MenuItem::action(
                            "Line endings: Mac",
                            ConvertEndingsMac,
                        )
                        .disabled(edit_off)
                    }),
                    show(hidden, "edit.convert_endings_unix", || {
                        MenuItem::action(
                            "Line endings: Unix",
                            ConvertEndingsUnix,
                        )
                        .disabled(edit_off)
                    }),
                ]),
            )
            .into_iter()
            .collect(),
            items([
                show(hidden, "edit.compare_selection", || {
                    MenuItem::action(
                        "Compare selection to clipboard",
                        CompareSelectionToClipboard,
                    )
                    .disabled(!state.is_text)
                }),
                // Alignment and section commands act on the focused text
                // view's diff list, not on the editor.
                show(hidden, "edit.align_with", || {
                    MenuItem::action("Align with marked line", AlignWithAnchor)
                        .disabled(!state.is_text)
                }),
                show(hidden, "edit.clear_alignments", || {
                    MenuItem::action(
                        "Clear alignment overrides",
                        ClearAlignments,
                    )
                    .disabled(!state.is_text)
                }),
                show(hidden, "edit.isolate_section", || {
                    MenuItem::action("Isolate section", IsolateSelection)
                        .disabled(!state.is_text)
                }),
            ]),
        ])));
    }

    // The Merge menu is a merge-tab exclusive addition; its take commands
    // act on the selected conflict section.
    if state.is_merge {
        menus.push(Menu::new("Merge").items(groups(vec![
            items([
                show(hidden, "merge.take_left", || {
                    MenuItem::action("Take left", TakeLeft)
                        .disabled(!state.merge_can_take)
                }),
                show(hidden, "merge.take_right", || {
                    MenuItem::action("Take right", TakeRight)
                        .disabled(!state.merge_can_take)
                }),
                show(hidden, "merge.take_ancestor", || {
                    MenuItem::action("Take ancestor", TakeAncestor).disabled(
                        !(state.merge_can_take && state.merge_has_ancestor),
                    )
                }),
            ]),
            items([
                show(hidden, "merge.save_output", || {
                    MenuItem::action("Save merge output…", SaveMergeOutput)
                }),
                // Adding a base file is only meaningful for a two-way
                // merge; a three-way merge already has an ancestor.
                show(hidden, "merge.set_ancestor", || {
                    MenuItem::action("Set ancestor…", SetMergeAncestor)
                        .disabled(state.merge_has_ancestor)
                }),
            ]),
        ])));
    }

    // The Search menu hides itself on merge and home views; the
    // next/previous commands have nothing to step there.
    if state.is_folder || state.is_text {
        menus.push(Menu::new("Search").items(groups(vec![
            items([
                show(hidden, "search.next_difference", || {
                    MenuItem::action("Next difference", NextDifference)
                }),
                show(hidden, "search.prev_difference", || {
                    MenuItem::action("Previous difference", PrevDifference)
                }),
            ]),
            // Sections only exist in a text compare; difference files are
            // the parent folder session's entries, so they need a live
            // parent. Find filename and Replace arrive with the search UI
            // (phase 2); their registry entries stay hidden until then.
            when(state.is_text, || {
                items([
                    show(hidden, "search.next_section", || {
                        MenuItem::action(
                            "Next difference section",
                            NextSection,
                        )
                    }),
                    show(hidden, "search.prev_section", || {
                        MenuItem::action(
                            "Previous difference section",
                            PrevSection,
                        )
                    }),
                ])
            }),
            when(state.is_text, || {
                items([
                    show(hidden, "search.next_difference_files", || {
                        MenuItem::action(
                            "Next difference files",
                            NextDifferenceFiles,
                        )
                        .disabled(!state.can_compare_parent)
                    }),
                    show(hidden, "search.prev_difference_files", || {
                        MenuItem::action(
                            "Previous difference files",
                            PrevDifferenceFiles,
                        )
                        .disabled(!state.can_compare_parent)
                    }),
                ])
            }),
        ])));
    }

    menus.push(Menu::new("View").items(groups(vec![
        // Folder tree commands: the directory pair acts on the
        // selection, the all pair on the whole loaded tree. Both share
        // the expansion state with the toolbar's structure controls.
        when(state.is_folder, || {
            items([
                show(hidden, "view.expand_dir", || {
                    MenuItem::action("Expand directory", ExpandDir)
                        .disabled(!state.selected_is_dir)
                }),
                show(hidden, "view.collapse_dir", || {
                    MenuItem::action("Collapse directory", CollapseDir)
                        .disabled(!state.selected_is_dir)
                }),
                show(hidden, "view.expand_all", || {
                    MenuItem::action("Expand all", ExpandAll)
                }),
                show(hidden, "view.collapse_all", || {
                    MenuItem::action("Collapse all", CollapseAll)
                }),
            ])
        }),
        // Display-filter toggles share the show-mask with the toolbar's
        // preset buttons: one toggle per status group (side-specific
        // sub-groups plus their umbrellas), mirroring the TUI's filter
        // rows, so every status the toolbar can isolate is also
        // reachable from the menu.
        when(state.is_folder, || {
            let shown = state.show_statuses;
            items([
                show(hidden, "view.show_hidden", || {
                    MenuItem::action(
                        filter_label(state.show_hidden, "hidden"),
                        ToggleShowHidden,
                    )
                }),
                show(hidden, "view.show_same", || {
                    MenuItem::action(
                        filter_label(
                            StatusFilter::Equal.is_shown(shown),
                            "identical",
                        ),
                        ToggleShowSame,
                    )
                }),
                show(hidden, "view.show_different", || {
                    MenuItem::action(
                        filter_label(
                            StatusFilter::Different.is_shown(shown),
                            "different",
                        ),
                        ToggleShowDifferent,
                    )
                }),
                show(hidden, "view.show_left_newer", || {
                    MenuItem::action(
                        filter_label(
                            StatusFilter::LeftNewer.is_shown(shown),
                            "left newer",
                        ),
                        ToggleShowLeftNewer,
                    )
                }),
                show(hidden, "view.show_right_newer", || {
                    MenuItem::action(
                        filter_label(
                            StatusFilter::RightNewer.is_shown(shown),
                            "right newer",
                        ),
                        ToggleShowRightNewer,
                    )
                }),
                show(hidden, "view.show_orphans", || {
                    MenuItem::action(
                        filter_label(
                            StatusFilter::Orphans.is_shown(shown),
                            "orphan",
                        ),
                        ToggleShowOrphans,
                    )
                }),
                show(hidden, "view.show_left_only", || {
                    MenuItem::action(
                        filter_label(
                            StatusFilter::LeftOnly.is_shown(shown),
                            "left-only",
                        ),
                        ToggleShowLeftOnly,
                    )
                }),
                show(hidden, "view.show_right_only", || {
                    MenuItem::action(
                        filter_label(
                            StatusFilter::RightOnly.is_shown(shown),
                            "right-only",
                        ),
                        ToggleShowRightOnly,
                    )
                }),
                show(hidden, "view.show_all", || {
                    MenuItem::action("Show all", ShowAllFilters)
                }),
            ])
        }),
        // Word wrap spans text and merge views; the focused view handles
        // the action.
        when(state.is_text || state.is_merge, || {
            items([show(hidden, "view.word_wrap", || {
                MenuItem::action("Word wrap", ToggleWordWrap)
                    .checked(state.word_wrap)
            })])
        }),
        when(state.is_merge, || {
            items([show(hidden, "view.show_ancestor", || {
                MenuItem::action("Show ancestor pane", ToggleMergeAncestor)
                    .checked(state.show_ancestor)
            })])
        }),
        // Detail pane modes are text-compare-only.
        when(state.is_text, || {
            items([
                show(hidden, "view.details_text", || {
                    MenuItem::action(
                        "Text details",
                        SetDetailMode {
                            mode: DetailMode::Text,
                        },
                    )
                    .checked(state.detail_mode == DetailMode::Text)
                }),
                show(hidden, "view.details_hex", || {
                    MenuItem::action(
                        "Hex details",
                        SetDetailMode {
                            mode: DetailMode::Hex,
                        },
                    )
                    .checked(state.detail_mode == DetailMode::Hex)
                }),
                show(hidden, "view.details_alignment", || {
                    MenuItem::action(
                        "Alignment details",
                        SetDetailMode {
                            mode: DetailMode::Alignment,
                        },
                    )
                    .checked(state.detail_mode == DetailMode::Alignment)
                }),
                show(hidden, "view.thumbnails", || {
                    MenuItem::action("Show thumbnails", ShowThumbnails)
                        .checked(state.show_thumbnails)
                }),
            ])
        }),
        // Window furniture is available from every view, home included.
        items([
            show(hidden, "view.log_pane", || {
                // The label states the result of invoking the item, so
                // it reads as a Show/Hide pair instead of a checkmark.
                MenuItem::action(
                    if state.log_open {
                        "Hide log pane"
                    } else {
                        "Show log pane"
                    },
                    ToggleLogPane,
                )
            }),
            // The single-row layout is an application option; the global
            // handler toggles and persists it for every window.
            show(hidden, "view.single_row_toolbar", || {
                MenuItem::action("Single-row toolbar", ToggleSingleRowToolbar)
                    .checked(state.single_row_toolbar)
            }),
        ]),
    ])));

    menus.push(Menu::new("Options").items(items([
        show(hidden, "options.open", || {
            MenuItem::action("Options…", Options)
        }),
        show(hidden, "options.file_formats", || {
            MenuItem::action("File formats…", FileFormats)
        }),
        show(hidden, "options.sync_presets", || {
            MenuItem::action("Folder Sync presets…", SyncPresets)
        }),
        show(hidden, "options.profiles", || {
            MenuItem::action("Profiles…", Profiles)
        }),
    ])));

    menus.push(Menu::new("Help").items(items([
        show(hidden, "help.contents", || {
            MenuItem::action("Help contents", HelpContents)
        }),
        show(hidden, "help.about", || {
            MenuItem::action("About cocomo", About)
        }),
    ])));

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

/// Live in-window menu bars, one per open window.
///
/// `cx.set_menus` only reaches the native macOS bar. On Windows and Linux
/// every window renders the kit's `AppMenuBar`, which reads the component
/// layer's `GlobalState` copy of the menus; this registry lets
/// `refresh_view_menus` reload each window's bar after the copy changes.
#[cfg(not(target_os = "macos"))]
#[derive(Default)]
struct MenuBars(Vec<gpui::WeakEntity<AppMenuBar>>);

#[cfg(not(target_os = "macos"))]
impl gpui::Global for MenuBars {}

/// Register a window's in-window menu bar so menu rebuilds reload it.
#[cfg(not(target_os = "macos"))]
pub fn register_menu_bar(bar: Entity<AppMenuBar>, cx: &mut App) {
    if cx.try_global::<MenuBars>().is_none() {
        cx.set_global(MenuBars::default());
    }
    let bars = cx.global_mut::<MenuBars>();
    bars.0.retain(|bar| bar.upgrade().is_some());
    bars.0.push(bar.downgrade());
}

/// Reload every live in-window menu bar from the component layer's copy.
#[cfg(not(target_os = "macos"))]
fn reload_menu_bars(cx: &mut App) {
    if cx.try_global::<MenuBars>().is_none() {
        // No window has registered a bar yet; the first bar loads the
        // menus when it is created.
        return;
    }
    let bars = {
        let bars = cx.global_mut::<MenuBars>();
        bars.0.retain(|bar| bar.upgrade().is_some());
        bars.0.clone()
    };
    for bar in bars {
        _ = bar.update(cx, |bar, cx| bar.reload(cx));
    }
}

/// Rebuild the menu bar so enabled states and checkmarks reflect the
/// active tab. Called by the window root whenever the snapshot changes.
pub fn refresh_view_menus(state: MenuState, cx: &mut App) {
    let hidden = hidden_command_ids(cx);
    cx.set_menus(build_app_menus(state, &hidden));
    // Keep the component layer's copy of the menus current for the
    // in-window bars on Windows and Linux. The copy is rebuilt from the
    // same inputs as the platform menus, so the two never diverge.
    #[cfg(not(target_os = "macos"))]
    {
        let owned = build_app_menus(state, &hidden)
            .into_iter()
            .map(Menu::owned)
            .collect::<Vec<_>>();
        gpui::component::GlobalState::global_mut(cx).set_app_menus(owned);
        reload_menu_bars(cx);
    }
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

/// Re-open the active view's file pair in another file compare view.
///
/// The pair comes from the active file-pair tab, or from the selected
/// entry of a folder tab. With neither in reach the command logs a
/// warning and does nothing; the menu items are disabled in that case,
/// so this only guards palette invocations.
fn compare_active_pair(kind: crate::file_formats::FileViewKind, cx: &mut App) {
    let Some(mgr) = active_manager(cx) else {
        return;
    };
    let pair = mgr
        .read(cx)
        .active_session()
        .and_then(|s| s.tab_entity.pair_endpoints(cx))
        .or_else(|| {
            mgr.read(cx)
                .active_session()
                .and_then(|s| s.tab_entity.folder_state(cx))
                .and_then(|state| state.read(cx).selected_endpoints())
        });
    let Some((left, right)) = pair else {
        log::push(LogLevel::Warning, "No file pair in reach to compare.", cx);
        return;
    };
    mgr.update(cx, |m, cx| {
        m.add_file_pair_tab(kind, left, right, None, cx);
    });
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

    // Sessions menu — save. Mark the session clean once the file write
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

    // Sessions menu — save as: ask for a name, then persist under it.
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

    // File menu — close all sessions of the active window, prompting
    // once when any session has unsaved changes.
    cx.on_action(move |_action: &CloseAllSessions, cx: &mut App| {
        let Some(mgr) = active_manager(cx) else {
            return;
        };
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            mgr.update(cx, |m, cx| m.request_close_all_sessions(window, cx));
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

    // Edit menu — copy paths and Select all entries are handled by the
    // focused folder view, which owns the selection.

    // View menu — display filters are handled by the focused folder view,
    // which owns the filter state; there is no global handler here.

    // View menu — word wrap is handled by the focused text view, which
    // owns the wrap state; there is no global handler here.

    // View menu — log pane visibility is a global the window root
    // observes, so the toggle works regardless of which view has focus.
    cx.on_action(move |_action: &ToggleLogPane, cx: &mut App| {
        cx.update_global::<LogPaneOpen, _>(|open, _| open.0 = !open.0);
    });

    // View menu — the single-row toolbar is a persisted application
    // option. Toggling it updates the global (which every window root
    // observes), saves it, and refreshes the windows so the toolbars
    // re-layout; the menu checkmark follows through the window root's
    // recomputed snapshot.
    cx.on_action(move |_action: &ToggleSingleRowToolbar, cx: &mut App| {
        let enabled = {
            let options = cx.global::<crate::options::AppOptions>();
            !options.single_row_toolbar
        };
        cx.update_global::<crate::options::AppOptions, _>(|options, _| {
            options.single_row_toolbar = enabled;
        });
        let options = cx.global::<crate::options::AppOptions>().clone();
        if let Err(e) = crate::options::save_options(&options) {
            log::push(
                LogLevel::Error,
                format!("Failed to save the toolbar layout option: {e}"),
                cx,
            );
        }
        crate::windows::refresh_all_windows(cx);
    });

    // Sessions menu — reload is handled by the focused folder view; the
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

    // Sessions menu — merge the active text compare's pair in a new merge
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

    // Sessions menu — clear the active folder or text session: drop the
    // comparison and selection, keep the endpoints so the session can be
    // reloaded.
    cx.on_action(move |_action: &ClearSession, cx: &mut App| {
        let Some(mgr) = active_manager(cx) else {
            return;
        };
        if let Some(state) = mgr.read(cx).active_folder_session(cx) {
            state.update(cx, |s, cx| s.clear_session(cx));
        } else if let Some(session) = mgr.read(cx).active_text_session(cx) {
            session.update(cx, |s, cx| s.clear_session(cx));
        }
    });

    // Sessions menu — step from a text compare back to the folder session
    // it was opened from.
    cx.on_action(move |_action: &CompareParentFolders, cx: &mut App| {
        let Some(mgr) = active_manager(cx) else {
            return;
        };
        let Some(session) = mgr.read(cx).active_text_session(cx) else {
            return;
        };
        let Some(parent) = session.read(cx).parent() else {
            return;
        };
        let found = mgr.update(cx, |m, cx| m.activate_folder_tab(&parent, cx));
        if !found {
            log::push(
                LogLevel::Warning,
                "The parent folder session is no longer open".to_string(),
                cx,
            );
        }
    });

    // File menu — open a file into the focused pane of the active text
    // compare through a path prompt (spec: "Open File (pane-specific)").
    cx.on_action(move |_action: &OpenFile, cx: &mut App| {
        let Some(mgr) = active_manager(cx) else {
            return;
        };
        let Some(session) = mgr.read(cx).active_text_session(cx) else {
            return;
        };
        let Some(window) = cx.active_window() else {
            return;
        };
        let side = session.read(cx).focus_side();
        let default_path = {
            let session = session.read(cx);
            let endpoint = match side {
                Side::Left => session.left_endpoint(),
                Side::Right => session.right_endpoint(),
            };
            endpoint.display().to_string()
        };
        let _ = window.update(cx, |_, window, cx| {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("File path")
                    .default_value(default_path)
            });
            let initial_focus = Cell::new(true);
            window.open_dialog(cx, move |dialog, window, cx| {
                if initial_focus.replace(false) {
                    input.focus_handle(cx).focus(window, cx);
                }
                let open_session = session.clone();
                let open_input = input.clone();
                dialog
                    .title(format!(
                        "Open file in {} pane",
                        match side {
                            Side::Left => "left",
                            Side::Right => "right",
                        }
                    ))
                    .child(Input::new(&input))
                    .footer(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("open_file_cancel")
                                    .label("Cancel")
                                    .on_click(|_, window, cx| {
                                        window.close_dialog(cx);
                                    }),
                            )
                            .child(
                                Button::new("open_file_ok")
                                    .label("Open")
                                    .primary()
                                    .on_click(move |_, window, cx| {
                                        let path = open_input
                                            .read(cx)
                                            .value()
                                            .trim()
                                            .to_string();
                                        if !path.is_empty() {
                                            open_session.update(
                                                cx,
                                                |s, cx| {
                                                    s.set_side_path(
                                                        side, &path, cx,
                                                    );
                                                },
                                            );
                                        }
                                        window.close_dialog(cx);
                                    }),
                            ),
                    )
            });
        });
    });

    // File menu — compare the focused pane's full text against the
    // clipboard in a new tab.
    cx.on_action(move |_action: &OpenClipboard, cx: &mut App| {
        let Some(mgr) = active_manager(cx) else {
            return;
        };
        let Some(session) = mgr.read(cx).active_text_session(cx) else {
            return;
        };
        let (side, text, endpoint) = {
            let session = session.read(cx);
            let side = session.focus_side();
            if !session.can_save() && !session.is_editing() {
                log::push(
                    LogLevel::Warning,
                    "The focused pane has no content to compare".to_string(),
                    cx,
                );
                return;
            }
            let text = session.side_text(side, cx);
            let endpoint = match side {
                Side::Left => session.left_endpoint(),
                Side::Right => session.right_endpoint(),
            };
            (side, text, endpoint)
        };
        let clipboard = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        let (left, right) = match side {
            Side::Left => (text, clipboard),
            Side::Right => (clipboard, text),
        };
        let label = endpoint
            .path()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| endpoint.display().to_string());
        let title = format!("{label} vs clipboard").into();
        mgr.update(cx, |m, cx| {
            m.add_in_memory_diff_tab(
                &left,
                &right,
                title,
                "file vs clipboard",
                &label,
                "clipboard",
                cx,
            );
        });
    });

    // File menu ▸ Open with — the target file is the focused pane of a
    // text compare or the selected entry of a folder compare.
    let open_with_target = |cx: &App| -> Option<(
        Endpoint,
        Option<gpui::WeakEntity<crate::state::AppState>>,
    )> {
        let mgr = active_manager(cx)?;
        if let Some(session) = mgr.read(cx).active_text_session(cx) {
            let session = session.read(cx);
            let endpoint = match session.focus_side() {
                Side::Left => session.left_endpoint(),
                Side::Right => session.right_endpoint(),
            };
            Some((endpoint, session.parent()))
        } else {
            let state = mgr.read(cx).active_folder_session(cx)?;
            let (left, _) = state.read(cx).selected_endpoints()?;
            Some((left, Some(state.downgrade())))
        }
    };

    // Open with ▸ Associated app: hand the file to the OS opener.
    let target = open_with_target;
    cx.on_action(move |_action: &OpenWithAssociatedApp, cx: &mut App| {
        let Some((endpoint, _)) = target(cx) else {
            return;
        };
        if let Err(e) = crate::fsops::open_with_associated_app(&endpoint) {
            log::push(LogLevel::Error, e.clone(), cx);
            notify_error(e, cx);
        }
    });

    // Open with ▸ Text edit: open the file in a text compare tab. A
    // single file has no counterpart, so both sides show it (the same
    // convention as opening a lone file from the folder table).
    let target = open_with_target;
    cx.on_action(move |_action: &OpenWithTextEdit, cx: &mut App| {
        let Some((endpoint, parent)) = target(cx) else {
            return;
        };
        let Some(mgr) = active_manager(cx) else {
            return;
        };
        mgr.update(cx, |m, cx| {
            m.add_text_diff_tab(endpoint.clone(), endpoint, parent, cx);
        });
    });

    // Open with ▸ Text patch: render the file as a unified diff in a
    // modal patch view.
    let target = open_with_target;
    cx.on_action(move |_action: &OpenWithTextPatch, cx: &mut App| {
        let Some((endpoint, _)) = target(cx) else {
            return;
        };
        let Some(window) = cx.active_window() else {
            return;
        };
        let path = endpoint.path().to_path_buf();
        let title: SharedString = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
            .into();
        cx.spawn(move |cx: &mut AsyncApp| {
            let cx = cx.clone();
            async move {
                let contents = match fs_err::tokio::read(&path).await {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                    Err(e) => {
                        let message =
                            format!("Couldn't read {}: {e}", path.display());
                        cx.update(|cx| {
                            log::push(LogLevel::Error, message.clone(), cx);
                            notify_error(message, cx);
                        });
                        return;
                    }
                };
                let files = match DiffFile::parse(&contents) {
                    Ok(files) => files,
                    Err(e) => {
                        let message = format!(
                            "{} is not a valid patch: {e}",
                            path.display()
                        );
                        cx.update(|cx| {
                            log::push(LogLevel::Warning, message.clone(), cx);
                            notify_error(message, cx);
                        });
                        return;
                    }
                };
                cx.update(|cx| {
                    let _ = window.update(cx, |_, window, cx| {
                        let state = cx.new(|cx| DiffState::new(files, cx));
                        let patch_title = title.clone();
                        window.open_dialog(cx, move |dialog, _, _| {
                            dialog
                                .title(format!("Patch — {patch_title}"))
                                .child(
                                    div()
                                        .w(px(760.))
                                        .h(px(480.))
                                        .overflow_hidden()
                                        .child(
                                            Diff::new(&state)
                                                .line_number(true),
                                        ),
                                )
                                .footer(
                                    h_flex().justify_end().child(
                                        Button::new("patch_close")
                                            .label("Close")
                                            .on_click(|_, window, cx| {
                                                window.close_dialog(cx);
                                            }),
                                    ),
                                )
                        });
                    });
                });
            }
        })
        .detach();
    });

    // Options menu — the application-options dialog.
    cx.on_action(move |_action: &Options, cx: &mut App| {
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            crate::options::open_options_dialog(window, cx);
        });
    });

    // Options menu — the file-format registry dialog.
    cx.on_action(move |_action: &FileFormats, cx: &mut App| {
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            crate::file_formats::open_file_formats_dialog(window, cx);
        });
    });

    // Options menu — the folder-sync presets dialog.
    cx.on_action(move |_action: &SyncPresets, cx: &mut App| {
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            crate::sync_presets::open_sync_presets_dialog(window, cx);
        });
    });

    // Options menu — the profile management dialog.
    cx.on_action(move |_action: &Profiles, cx: &mut App| {
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            crate::profiles::open_profiles_dialog(window, cx);
        });
    });

    // Sessions menu — "Compare files using ▸": re-open the active pair
    // in the chosen file view kind.
    cx.on_action(move |_action: &CompareUsingText, cx: &mut App| {
        compare_active_pair(crate::file_formats::FileViewKind::Text, cx);
    });
    cx.on_action(move |_action: &CompareUsingTable, cx: &mut App| {
        compare_active_pair(crate::file_formats::FileViewKind::Table, cx);
    });
    cx.on_action(move |_action: &CompareUsingHex, cx: &mut App| {
        compare_active_pair(crate::file_formats::FileViewKind::Hex, cx);
    });
    cx.on_action(move |_action: &CompareUsingPicture, cx: &mut App| {
        compare_active_pair(crate::file_formats::FileViewKind::Picture, cx);
    });

    // Help menu — help contents: a static markdown guide. Version and
    // license live in the About dialog.
    cx.on_action(move |_action: &HelpContents, cx: &mut App| {
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            window.open_dialog(cx, |dialog, _, _| {
                dialog
                    .title("cocomo help")
                    .child(
                        div()
                            .id("help_contents_scroll")
                            .w(px(600.))
                            .max_h(px(520.))
                            .overflow_y_scroll()
                            .child(TextView::markdown(
                                ElementId::Name("help_contents".into()),
                                HELP_MD,
                            )),
                    )
                    .footer(h_flex().justify_end().child(
                        Button::new("help_ok").label("Close").on_click(
                            |_, window, cx| {
                                window.close_dialog(cx);
                            },
                        ),
                    ))
            });
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

/// The static Help contents page (spec section 1.3, Help menu).
const HELP_MD: &str = "\
# cocomo

COmpare, COpy & MOve directories and files. Every open session lives in
its own tab; the toolbar and menu bar act on the active tab.

## Folder compare

Browse two directory trees side by side. The gutter marks show how each
entry differs; select a row and use the **Actions** menu (or the row
context menu) to copy, move, exchange, delete, or synchronize.

- Double-click a directory to descend into it; **Session ▸ Back/Forward**
  steps through the compared pairs.
- The **Filters** combo narrows the visible rows by name pattern.
- **Synchronize** reconciles the whole tree: *Update* copies what is
  newer or missing, *Mirror* also deletes extras on the target side.

## Text compare

Select a file pair in a folder compare (or use **File ▸ New compare**)
to open a two-pane diff.

- **Search ▸ Next/Previous Difference** jumps between differences;
  **Next/Previous Difference Section** jumps between sections.
- **Mark line for alignment** then **Align with marked line** forces a
  pairing the automatic diff cannot infer.
- Toggle edit mode to edit either side; **File ▸ Save File** writes the
  focused pane. **Convert file** fixes whitespace and line endings.
- **File ▸ Open clipboard** compares the focused pane against the
  clipboard; **Compare selection to clipboard** uses just the selection.

## Text merge

**Sessions ▸ Merge files** opens the active pair as a three-pane merge.
**Merge ▸ Take left/right/ancestor** adopts a section from one pane;
**Save merge output** writes the result.

## Personalization

**Options ▸ Options ▸ Commands** lists every command: re-bind its
shortcut, unbind it, or hide it from the menus entirely. **Options ▸ File
formats** controls which view opens each file type.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `cmd-n` | New compare |
| `cmd-o` | Open session |
| `cmd-s` / `cmd-shift-s` | Save session / Save as |
| `cmd-w` | Close session |
| `cmd-shift-r` | Reload / recompare |
| `cmd-]` / `cmd-[` | Next / previous difference |
| `cmd-shift-]` / `cmd-shift-[` | Next / previous section |
| `cmd-l` / `cmd-r` | Copy to left / right |
| `j` / `k` | Move through rows |
";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Building every menu for every tab kind must not panic. The
    /// `show` debug assertion fails the test if a menu item references a
    /// command id that is missing from the registry, keeping the
    /// registry the single source for visibility.
    #[test]
    fn menus_only_reference_registered_commands() {
        let hidden = HashSet::new();
        let states = [
            MenuState::empty(),
            MenuState {
                is_folder: true,
                has_selection: true,
                can_exchange: true,
                can_sync: true,
                ..MenuState::empty()
            },
            MenuState {
                is_text: true,
                is_text_editing: true,
                can_compare_parent: true,
                ..MenuState::empty()
            },
            MenuState {
                is_merge: true,
                merge_can_take: true,
                merge_has_ancestor: true,
                ..MenuState::empty()
            },
            MenuState {
                is_home: true,
                ..MenuState::empty()
            },
        ];
        for state in states {
            let menus = build_app_menus(state, &hidden);
            assert!(!menus.is_empty());
        }
    }

    /// Hiding a command removes its item entirely (spec section 7.2:
    /// "hide", not "disable").
    #[test]
    fn hidden_commands_leave_their_items_out() {
        let visible = build_app_menus(MenuState::empty(), &HashSet::new());
        let session = visible.iter().find(|m| m.name == "Sessions").unwrap();
        let all = build_app_menus(
            MenuState::empty(),
            &HashSet::from(["session.save", "session.save_as"]),
        );
        let hidden_session =
            all.iter().find(|m| m.name == "Sessions").unwrap();
        assert!(
            hidden_session.items.len() < session.items.len(),
            "hiding two commands must shrink the Sessions menu"
        );
    }

    /// Home has no file pair, no folder pair, and nothing to search,
    /// so only the always-present menus appear in the bar.
    #[test]
    fn home_hides_the_view_specific_menus() {
        let menus = build_app_menus(MenuState::empty(), &HashSet::new());
        let names: Vec<String> =
            menus.iter().map(|m| m.name.to_string()).collect();
        assert_eq!(names, ["Sessions", "View", "Options", "Help"]);
    }

    /// The "Compare files using" submenu carries the four file views,
    /// and hiding one command removes just that entry.
    #[test]
    fn compare_using_submenu_lists_the_four_file_views() {
        let submenu = |hidden: &HashSet<&'static str>| {
            build_app_menus(MenuState::empty(), hidden)
                .into_iter()
                .find(|m| m.name == "Sessions")
                .and_then(|m| {
                    m.items.into_iter().find_map(|item| match item {
                        MenuItem::Submenu(menu)
                            if menu.name == "Compare files using" =>
                        {
                            Some(menu)
                        }
                        _ => None,
                    })
                })
                .expect("Sessions menu must carry the compare submenu")
        };
        assert_eq!(submenu(&HashSet::new()).items.len(), 4);
        assert_eq!(
            submenu(&HashSet::from(["session.compare_as_hex"]))
                .items
                .len(),
            3
        );
    }

    /// The Options menu offers the folder-sync presets dialog.
    #[test]
    fn options_menu_offers_the_sync_presets_dialog() {
        let menus = build_app_menus(MenuState::empty(), &HashSet::new());
        let options = menus.iter().find(|m| m.name == "Options").unwrap();
        assert!(
            options.items.iter().any(|item| matches!(
                item,
                MenuItem::Action { name, .. }
                    if name == "Folder Sync presets…"
            )),
            "Options menu must offer the Folder Sync presets dialog"
        );
    }

    /// On a folder tab the View menu carries the tree and filter
    /// groups; every filter toggle reads Hide while its group is shown,
    /// and the log pane item reads as a Hide/Show pair.
    #[test]
    fn folder_view_menu_carries_the_tree_and_filter_groups() {
        let menus = build_app_menus(
            MenuState {
                is_folder: true,
                ..MenuState::empty()
            },
            &HashSet::new(),
        );
        let view = menus.iter().find(|m| m.name == "View").unwrap();
        let has = |target: &str| {
            view.items.iter().any(|item| {
                matches!(item, MenuItem::Action { name, .. } if name == target)
            })
        };
        for expected in [
            "Expand directory",
            "Collapse directory",
            "Expand all",
            "Collapse all",
            // The default snapshot shows everything, so every toggle
            // offers hiding its group.
            "Hide hidden items",
            "Hide identical items",
            "Hide different items",
            "Hide left newer items",
            "Hide right newer items",
            "Hide orphan items",
            "Hide left-only items",
            "Hide right-only items",
            "Show all",
            // The snapshot starts with the pane open, so the item
            // offers hiding it.
            "Hide log pane",
        ] {
            assert!(has(expected), "View menu must carry {expected}");
        }
    }

    /// A filtered snapshot flips the affected toggles to `Show …`,
    /// including the umbrellas of partially hidden side-specific groups.
    #[test]
    fn filter_toggles_read_show_for_filtered_groups() {
        let menus = build_app_menus(
            MenuState {
                is_folder: true,
                show_hidden: false,
                show_statuses: DirEntryStatus::all()
                    .difference(DirEntryStatus::LeftNewer)
                    .difference(DirEntryStatus::RightOrphan),
                ..MenuState::empty()
            },
            &HashSet::new(),
        );
        let view = menus.iter().find(|m| m.name == "View").unwrap();
        let has = |target: &str| {
            view.items.iter().any(|item| {
                matches!(item, MenuItem::Action { name, .. } if name == target)
            })
        };
        for expected in [
            "Show hidden items",
            "Show left newer items",
            "Show right-only items",
            // Partial umbrellas count as filtered out.
            "Show different items",
            "Show orphan items",
        ] {
            assert!(has(expected), "View menu must carry {expected}");
        }
        assert!(
            has("Hide identical items"),
            "an untouched group must still offer hiding"
        );
    }

    /// Without a folder tab the tree and filter groups have nothing to
    /// act on and stay out of the View menu.
    #[test]
    fn view_menu_drops_folder_groups_without_a_folder_tab() {
        let menus = build_app_menus(MenuState::empty(), &HashSet::new());
        let view = menus.iter().find(|m| m.name == "View").unwrap();
        assert!(
            !view.items.iter().any(|item| matches!(
                item,
                MenuItem::Action { name, .. } if name == "Expand directory"
            )),
            "tree commands must not appear without a folder tab"
        );
    }

    /// On platforms without a native menu bar, `refresh_view_menus` must
    /// mirror the rebuilt menus into the component layer so the
    /// window-rendered `AppMenuBar` has them to display.
    #[cfg(not(target_os = "macos"))]
    #[gpui::test]
    fn refresh_view_menus_feeds_the_in_window_bars(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::init(cx);
            cx.set_global(crate::options::AppOptions::default());
            cx.set_global(crate::log::LogPaneOpen(true));
            set_app_menus(cx);
            let menus = gpui::component::GlobalState::global(cx).app_menus();
            assert!(
                menus.iter().any(|menu| menu.name == "Sessions"),
                "the component layer must hold the rebuilt menu bar"
            );
            let view = menus.iter().find(|menu| menu.name == "View").unwrap();
            assert!(
                !view.items.is_empty(),
                "the View menu must carry items for the in-window bar"
            );
        });
    }
}
