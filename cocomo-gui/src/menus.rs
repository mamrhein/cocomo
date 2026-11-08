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

use cocomo_lib::Endpoint;
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
        CopyToFolder, CopyToLeft, CopyToRight, DeleteLeftSide,
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
        ToggleShowOrphans,
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
    // The Open with commands need a concrete file: the focused pane of a
    // text compare, or the selected entry of a folder compare.
    let can_open_file =
        state.is_text || (state.is_folder && state.has_selection);

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
            show(hidden, "session.clear", || {
                // Merge tabs are seeded from a compare pair; clearing
                // one would leave a merge with no sources to take.
                MenuItem::action("Clear session", ClearSession)
                    .disabled(!(state.is_folder || state.is_text))
            }),
            Some(MenuItem::separator()),
            show(hidden, "session.back", || {
                MenuItem::action("Back", GoBack)
                    .disabled(off(state.can_go_back))
            }),
            show(hidden, "session.forward", || {
                MenuItem::action("Forward", GoForward)
                    .disabled(off(state.can_go_forward))
            }),
            Some(MenuItem::separator()),
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
            Some(MenuItem::separator()),
            show(hidden, "session.report", || {
                MenuItem::action("Report…", SessionReport)
                    .disabled(!state.is_text)
            }),
            show(hidden, "session.info", || {
                MenuItem::action("Info…", SessionInfo).disabled(!state.is_text)
            }),
            Some(MenuItem::separator()),
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
            Some(MenuItem::submenu(Menu::new("Compare files using").items(
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
            ))),
        ])),
        Menu::new("File").items(items([
            show(hidden, "file.new_compare", || {
                MenuItem::action("New compare…", NewCompare)
            }),
            show(hidden, "file.open_session", || {
                MenuItem::action("Open session…", OpenSession)
            }),
            Some(MenuItem::separator()),
            // Open file… retargets the focused pane of a text compare;
            // the Open with commands act on the focused file (text) or
            // the selected entry (folder).
            show(hidden, "file.open_file", || {
                MenuItem::action("Open file…", OpenFile)
                    .disabled(!state.is_text)
            }),
            show(hidden, "file.open_clipboard", || {
                MenuItem::action("Open clipboard", OpenClipboard)
                    .disabled(!state.is_text)
            }),
            Some(MenuItem::submenu(Menu::new("Open with").items(items([
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
            ])))),
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
            show(hidden, "file.close_all", || {
                MenuItem::action("Close all sessions", CloseAllSessions)
            }),
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
            show(hidden, "file.quit", || MenuItem::action("Quit", Quit)),
        ])),
    ];

    if !is_home {
        // Text-editing commands act on the focused editor and are only
        // meaningful while the text view is in edit mode.
        let edit_off = !(state.is_text && state.is_text_editing);
        menus.extend([
            Menu::new("Edit").items(items([
                show(hidden, "edit.copy_path_left", || {
                    MenuItem::action("Copy left path", CopyPathLeft)
                        .disabled(no_folder)
                }),
                show(hidden, "edit.copy_path_right", || {
                    MenuItem::action("Copy right path", CopyPathRight)
                        .disabled(no_folder)
                }),
                Some(MenuItem::separator()),
                show(hidden, "edit.select_all_entries", || {
                    MenuItem::action("Select all entries", SelectAllEntries)
                        .disabled(no_folder)
                }),
                Some(MenuItem::separator()),
                show(hidden, "edit.undo", || {
                    MenuItem::action("Undo", TextUndo).disabled(edit_off)
                }),
                show(hidden, "edit.redo", || {
                    MenuItem::action("Redo", TextRedo).disabled(edit_off)
                }),
                Some(MenuItem::separator()),
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
                Some(MenuItem::separator()),
                show(hidden, "edit.select_all_text", || {
                    MenuItem::action("Select all text", TextSelectAll)
                        .disabled(edit_off)
                }),
                show(hidden, "edit.select_section", || {
                    MenuItem::action("Select section", SelectSection)
                        .disabled(edit_off)
                }),
                Some(MenuItem::separator()),
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
                Some(MenuItem::separator()),
                Some(MenuItem::submenu(Menu::new("Convert file").items(
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
                ))),
                Some(MenuItem::separator()),
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
            ])),
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
                show(hidden, "actions.exchange", || {
                    MenuItem::action("Exchange", ExchangeEntry)
                        .disabled(off(state.can_exchange))
                }),
                Some(MenuItem::separator()),
                show(hidden, "actions.copy_to_folder", || {
                    MenuItem::action("Copy to folder…", CopyToFolder)
                        .disabled(off(state.has_selection))
                }),
                show(hidden, "actions.move_to_folder", || {
                    MenuItem::action("Move to folder…", MoveToFolder)
                        .disabled(off(state.has_selection))
                }),
                Some(MenuItem::separator()),
                // Whole-comparison sync runs; each confirms first because
                // mirror deletes the target side's extras.
                Some(MenuItem::submenu(Menu::new("Synchronize").items(
                    items([
                        show(hidden, "actions.sync_update_right", || {
                            MenuItem::action("Update right", SyncUpdateRight)
                                .disabled(off(state.can_sync))
                        }),
                        show(hidden, "actions.sync_update_left", || {
                            MenuItem::action("Update left", SyncUpdateLeft)
                                .disabled(off(state.can_sync))
                        }),
                        show(hidden, "actions.sync_update_both", || {
                            MenuItem::action("Update both", SyncUpdateBoth)
                                .disabled(off(state.can_sync))
                        }),
                        Some(MenuItem::separator()),
                        show(hidden, "actions.sync_mirror_right", || {
                            MenuItem::action(
                                "Mirror to right",
                                SyncMirrorRight,
                            )
                            .disabled(off(state.can_sync))
                        }),
                        show(hidden, "actions.sync_mirror_left", || {
                            MenuItem::action("Mirror to left", SyncMirrorLeft)
                                .disabled(off(state.can_sync))
                        }),
                    ]),
                ))),
                Some(MenuItem::separator()),
                // Text-sync commands: copy this pair's file and step the
                // parent folder session to its next difference.
                show(hidden, "actions.copy_file_right_next", || {
                    MenuItem::action(
                        "Copy file to right and open next",
                        CopyFileRightNext,
                    )
                    .disabled(!state.is_text)
                }),
                show(hidden, "actions.copy_file_left_next", || {
                    MenuItem::action(
                        "Copy file to left and open next",
                        CopyFileLeftNext,
                    )
                    .disabled(!state.is_text)
                }),
                Some(MenuItem::separator()),
                // Merge commands act on the focused merge view's selected
                // section; the ancestor command needs a three-way merge.
                show(hidden, "actions.take_left", || {
                    MenuItem::action("Take left", TakeLeft)
                        .disabled(!(state.is_merge && state.merge_can_take))
                }),
                show(hidden, "actions.take_right", || {
                    MenuItem::action("Take right", TakeRight)
                        .disabled(!(state.is_merge && state.merge_can_take))
                }),
                show(hidden, "actions.take_ancestor", || {
                    MenuItem::action("Take ancestor", TakeAncestor).disabled(
                        !(state.is_merge
                            && state.merge_can_take
                            && state.merge_has_ancestor),
                    )
                }),
                Some(MenuItem::separator()),
                show(hidden, "actions.save_merge_output", || {
                    MenuItem::action("Save merge output…", SaveMergeOutput)
                        .disabled(!state.is_merge)
                }),
                // Adding a base file is only meaningful for a two-way merge;
                // a three-way merge already has an ancestor.
                show(hidden, "actions.set_ancestor", || {
                    MenuItem::action("Set ancestor…", SetMergeAncestor)
                        .disabled(
                            !(state.is_merge && !state.merge_has_ancestor),
                        )
                }),
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
                show(hidden, "search.next_section", || {
                    MenuItem::action("Next difference section", NextSection)
                        .disabled(!state.is_text)
                }),
                show(hidden, "search.prev_section", || {
                    MenuItem::action(
                        "Previous difference section",
                        PrevSection,
                    )
                    .disabled(!state.is_text)
                }),
                Some(MenuItem::separator()),
                // Next/Previous Difference Files step the *parent* folder
                // session of a child text compare; they need a live parent.
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
                // Find filename and Replace arrive with the search UI
                // (later phase); their registry entries stay hidden until
                // then.
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
            show(hidden, "view.show_ancestor", || {
                MenuItem::action("Show ancestor pane", ToggleMergeAncestor)
                    .disabled(!state.is_merge)
                    .checked(state.show_ancestor)
            }),
            Some(MenuItem::separator()),
            // Detail pane modes are text-compare-only; the focused text
            // view handles the actions.
            show(hidden, "view.details_text", || {
                MenuItem::action(
                    "Text details",
                    SetDetailMode {
                        mode: DetailMode::Text,
                    },
                )
                .disabled(!state.is_text)
                .checked(state.detail_mode == DetailMode::Text)
            }),
            show(hidden, "view.details_hex", || {
                MenuItem::action(
                    "Hex details",
                    SetDetailMode {
                        mode: DetailMode::Hex,
                    },
                )
                .disabled(!state.is_text)
                .checked(state.detail_mode == DetailMode::Hex)
            }),
            show(hidden, "view.details_alignment", || {
                MenuItem::action(
                    "Alignment details",
                    SetDetailMode {
                        mode: DetailMode::Alignment,
                    },
                )
                .disabled(!state.is_text)
                .checked(state.detail_mode == DetailMode::Alignment)
            }),
            show(hidden, "view.thumbnails", || {
                MenuItem::action("Show thumbnails", ShowThumbnails)
                    .disabled(!state.is_text)
                    .checked(state.show_thumbnails)
            }),
            Some(MenuItem::separator()),
            show(hidden, "view.log_pane", || {
                MenuItem::action("Log pane", ToggleLogPane)
                    .checked(state.log_open)
            }),
            // The single-row layout is an application option; the global
            // handler toggles and persists it for every window.
            show(hidden, "view.single_row_toolbar", || {
                MenuItem::action("Single-row toolbar", ToggleSingleRowToolbar)
                    .checked(state.single_row_toolbar)
            }),
        ])),
        Menu::new("Tools").items(items([
            show(hidden, "tools.options", || {
                MenuItem::action("Options…", Options)
            }),
            show(hidden, "tools.file_formats", || {
                MenuItem::action("File formats…", FileFormats)
            }),
            show(hidden, "tools.sync_presets", || {
                MenuItem::action("Folder Sync presets…", SyncPresets)
            }),
        ])),
        Menu::new("Help").items(items([
            show(hidden, "help.contents", || {
                MenuItem::action("Help contents", HelpContents)
            }),
            show(hidden, "help.about", || {
                MenuItem::action("About cocomo", About)
            }),
        ])),
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

    // Session menu — clear the active folder or text session: drop the
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

    // Session menu — step from a text compare back to the folder session
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

    // Tools menu — the folder-sync presets dialog.
    cx.on_action(move |_action: &SyncPresets, cx: &mut App| {
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            crate::sync_presets::open_sync_presets_dialog(window, cx);
        });
    });

    // Session menu — "Compare files using ▸": re-open the active pair
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

**Session ▸ Merge files** opens the active pair as a three-pane merge.
**Actions ▸ Take left/right/ancestor** adopts a section from one pane;
**Save merge output** writes the result.

## Personalization

**Tools ▸ Options ▸ Commands** lists every command: re-bind its
shortcut, unbind it, or hide it from the menus entirely. **Tools ▸ File
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
        let session = visible.iter().find(|m| m.name == "Session").unwrap();
        let all = build_app_menus(
            MenuState::empty(),
            &HashSet::from(["session.save", "session.save_as"]),
        );
        let hidden_session = all.iter().find(|m| m.name == "Session").unwrap();
        assert!(
            hidden_session.items.len() < session.items.len(),
            "hiding two commands must shrink the Session menu"
        );
    }

    /// The "Compare files using" submenu carries the four file views,
    /// and hiding one command removes just that entry.
    #[test]
    fn compare_using_submenu_lists_the_four_file_views() {
        let submenu = |hidden: &HashSet<&'static str>| {
            build_app_menus(MenuState::empty(), hidden)
                .into_iter()
                .find(|m| m.name == "Session")
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
                .expect("Session menu must carry the compare submenu")
        };
        assert_eq!(submenu(&HashSet::new()).items.len(), 4);
        assert_eq!(
            submenu(&HashSet::from(["session.compare_as_hex"]))
                .items
                .len(),
            3
        );
    }

    /// The Tools menu offers the folder-sync presets dialog.
    #[test]
    fn tools_menu_offers_the_sync_presets_dialog() {
        let menus = build_app_menus(MenuState::empty(), &HashSet::new());
        let tools = menus.iter().find(|m| m.name == "Tools").unwrap();
        assert!(
            tools.items.iter().any(|item| matches!(
                item,
                MenuItem::Action { name, .. }
                    if name == "Folder Sync presets…"
            )),
            "Tools menu must offer the Folder Sync presets dialog"
        );
    }
}
