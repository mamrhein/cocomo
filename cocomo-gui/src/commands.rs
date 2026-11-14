// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! The command registry: every user-invocable command, its default key
//! binding, and whether the user has chosen to show it.
//!
//! The registry is the single source of truth for personalization (spec
//! section 7.2). It drives three things:
//!
//! - the keymap, rebuilt from the base bindings with the user's overrides
//!   applied ([`rebuild_keymap`]),
//! - which menu items the menu bar contains, and
//! - which toolbar buttons toolbar row 1 shows.
//!
//! Bindings are rebuilt rather than patched: GPUI's keymap only appends,
//! so replacing a binding requires clearing and re-adding every binding,
//! including the view-scoped ones the views no longer register themselves.

use std::any::TypeId;

use gpui::{
    Action, App, InvalidKeystrokeError, KeyBinding,
    component::input::{
        Copy as TextCopy, Cut as TextCut, Delete as TextDelete,
        DeleteToBeginningOfLine, DeleteToEndOfLine, Indent as TextIndent,
        Outdent as TextOutdent, Paste as TextPaste, Redo as TextRedo,
        SelectAll as TextSelectAll, Undo as TextUndo,
    },
};

use crate::{
    home,
    menus::{
        About, CloseAllSessions, CloseSession, CollapseAll, CollapseDir,
        CompareParentFolders, CompareUsingHex, CompareUsingPicture,
        CompareUsingTable, CompareUsingText, ExpandAll, ExpandDir,
        FileFormats, HelpContents, MergeFiles, NewCompare, NextSection,
        OpenClipboard, OpenFile, OpenSession, OpenWithAssociatedApp,
        OpenWithTextEdit, OpenWithTextPatch, Options, PrevSection, Profiles,
        Quit, ReloadCompare, RestoreWorkspace, SaveFile, SaveFileAs,
        SaveSession, SaveSessionAs, SaveWorkspace, SessionInfo, SessionReport,
        SessionSettings, ShowAllFilters, StopCompare, SyncPresets,
        ToggleLogPane, ToggleMergeAncestor, ToggleShowDifferent,
        ToggleShowHidden, ToggleShowLeftNewer, ToggleShowLeftOnly,
        ToggleShowOrphans, ToggleShowRightNewer, ToggleShowRightOnly,
        ToggleShowSame, ToggleSingleRowToolbar, ToggleWordWrap, menu_bindings,
    },
    options::{self, AppOptions},
    text_diff::{
        AlignWithAnchor, ClearAlignments, CompareSelectionToClipboard,
        ConvertEndingsMac, ConvertEndingsPc, ConvertEndingsUnix,
        ConvertSpacesToTabs, ConvertTabsToSpaces, ConvertTrimTrailing,
        CopyFileLeftNext, CopyFileRightNext, CopySectionLeft,
        CopySectionRight, DeleteLine, DetailMode, InsertLineAfter,
        InsertLineBefore, IsolateSelection, NextDifferenceFiles, NextLine,
        PrevDifferenceFiles, PrevLine, SelectSection, SetDetailMode,
    },
    text_merge::{
        NextConflict, NextMergeSection, PrevConflict, PrevMergeSection,
        SaveMergeOutput, SetMergeAncestor, TakeAncestor, TakeLeft, TakeRight,
    },
    ui::{
        CompareContentsHash, CompareContentsMeta, CopyFilename, CopyToFolder,
        CopyToLeft, CopyToOtherSide, CopyToRight, DeleteBothSide,
        DeleteLeftSide, DeleteRightSide, ExchangeEntry, ExcludeEntry, GoBack,
        GoForward, MoveToFolder, MoveToLeft, MoveToRight, NewFolder,
        NextDifference, PrevDifference, QuickCompare, RefreshSelection,
        RenameEntry, RevealInExplorer, SetBaseBoth, SetBaseLeft, SetBaseRight,
        ShowAttributes, SwapSides, SyncMirrorLeft, SyncMirrorRight,
        SyncUpdateBoth, SyncUpdateLeft, SyncUpdateRight, ToggleIgnored,
        TouchEntry, folder_compare_bindings,
    },
};

/// One command the user can show or hide, and (where it has one) rebind.
pub struct CommandSpec {
    /// Stable identifier persisted in the options file. Never reuse an id
    /// for a different command; renames must keep the old id working.
    pub id: &'static str,
    /// Human-readable name shown in the Options dialog.
    pub label: &'static str,
    /// Grouping label in the Options dialog.
    pub category: &'static str,
    /// The binding shipped with the application, if the command has one.
    pub default_binding: Option<&'static str>,
    /// Builds an instance of the command's action for keymap registration.
    /// `None` for commands that only ever appear in the toolbar.
    pub action: Option<fn() -> Box<dyn Action>>,
}

impl CommandSpec {
    /// Whether this command can carry a key binding at all.
    pub fn is_bindable(&self) -> bool {
        self.action.is_some()
    }
}

/// Build a boxed action instance from a unit action type.
fn action<A: Action + Default>() -> Box<dyn Action> {
    Box::new(A::default())
}

/// The detail-pane modes carry a payload, so they need fixed-mode
/// constructors instead of [`action`].
fn detail_text() -> Box<dyn Action> {
    Box::new(SetDetailMode {
        mode: DetailMode::Text,
    })
}

fn detail_hex() -> Box<dyn Action> {
    Box::new(SetDetailMode {
        mode: DetailMode::Hex,
    })
}

fn detail_alignment() -> Box<dyn Action> {
    Box::new(SetDetailMode {
        mode: DetailMode::Alignment,
    })
}

/// Every command exposed to the Options dialog, in display order.
///
/// The list is the single source for menu and context-menu visibility
/// and for key bindings (spec section 7.4-4). Commands whose handlers
/// arrive in a later phase are registered with `action: None` and stay
/// out of the menus until their view exists.
pub fn all_commands() -> Vec<CommandSpec> {
    vec![
        // Session
        CommandSpec {
            id: "session.new_compare",
            label: "New compare…",
            category: "Session",
            default_binding: Some("cmd-n"),
            action: Some(action::<NewCompare>),
        },
        CommandSpec {
            id: "session.open",
            label: "Open session…",
            category: "Session",
            default_binding: Some("cmd-o"),
            action: Some(action::<OpenSession>),
        },
        CommandSpec {
            id: "session.save",
            label: "Save session",
            category: "Session",
            default_binding: Some("cmd-s"),
            action: Some(action::<SaveSession>),
        },
        CommandSpec {
            id: "session.save_as",
            label: "Save session as…",
            category: "Session",
            default_binding: Some("cmd-shift-s"),
            action: Some(action::<SaveSessionAs>),
        },
        CommandSpec {
            id: "session.settings",
            label: "Session settings…",
            category: "Session",
            default_binding: None,
            action: Some(action::<SessionSettings>),
        },
        CommandSpec {
            id: "session.back",
            label: "Back",
            category: "Session",
            default_binding: None,
            action: Some(action::<GoBack>),
        },
        CommandSpec {
            id: "session.forward",
            label: "Forward",
            category: "Session",
            default_binding: None,
            action: Some(action::<GoForward>),
        },
        CommandSpec {
            id: "session.swap_sides",
            label: "Swap sides",
            category: "Session",
            default_binding: None,
            action: Some(action::<SwapSides>),
        },
        CommandSpec {
            id: "session.reload",
            label: "Reload / recompare files",
            category: "Session",
            default_binding: Some("cmd-shift-r"),
            action: Some(action::<ReloadCompare>),
        },
        CommandSpec {
            id: "session.stop",
            label: "Stop comparison",
            category: "Session",
            default_binding: None,
            action: Some(action::<StopCompare>),
        },
        CommandSpec {
            id: "session.report",
            label: "Report…",
            category: "Session",
            default_binding: None,
            action: Some(action::<SessionReport>),
        },
        CommandSpec {
            id: "session.info",
            label: "Info…",
            category: "Session",
            default_binding: None,
            action: Some(action::<SessionInfo>),
        },
        CommandSpec {
            id: "session.merge",
            label: "Merge files…",
            category: "Session",
            default_binding: None,
            action: Some(action::<MergeFiles>),
        },
        CommandSpec {
            id: "session.merge_ancestor",
            label: "Merge with ancestor…",
            category: "Session",
            default_binding: None,
            // Invoked only from the folder row's context menu, which supplies
            // the file pair, so it has no command-palette action.
            action: None,
        },
        CommandSpec {
            id: "session.compare_parent",
            label: "Compare parent folders",
            category: "Session",
            default_binding: None,
            action: Some(action::<CompareParentFolders>),
        },
        CommandSpec {
            id: "session.compare_as_text",
            label: "Compare files using text",
            category: "Session",
            default_binding: None,
            action: Some(action::<CompareUsingText>),
        },
        CommandSpec {
            id: "session.compare_as_table",
            label: "Compare files using table",
            category: "Session",
            default_binding: None,
            action: Some(action::<CompareUsingTable>),
        },
        CommandSpec {
            id: "session.compare_as_hex",
            label: "Compare files using hex",
            category: "Session",
            default_binding: None,
            action: Some(action::<CompareUsingHex>),
        },
        CommandSpec {
            id: "session.compare_as_picture",
            label: "Compare files using picture",
            category: "Session",
            default_binding: None,
            action: Some(action::<CompareUsingPicture>),
        },
        CommandSpec {
            id: "session.close",
            label: "Close session",
            category: "Session",
            default_binding: Some("cmd-w"),
            action: Some(action::<CloseSession>),
        },
        CommandSpec {
            id: "session.close_all",
            label: "Close all sessions",
            category: "Session",
            default_binding: None,
            action: Some(action::<CloseAllSessions>),
        },
        CommandSpec {
            id: "session.save_workspace",
            label: "Save workspace…",
            category: "Session",
            default_binding: None,
            action: Some(action::<SaveWorkspace>),
        },
        CommandSpec {
            id: "session.restore_workspace",
            label: "Restore workspace…",
            category: "Session",
            default_binding: None,
            action: Some(action::<RestoreWorkspace>),
        },
        CommandSpec {
            id: "session.quit",
            label: "Quit",
            category: "Session",
            default_binding: None,
            action: Some(action::<Quit>),
        },
        // File
        CommandSpec {
            id: "file.open_file",
            label: "Open file…",
            category: "File",
            default_binding: None,
            action: Some(action::<OpenFile>),
        },
        CommandSpec {
            id: "file.open_clipboard",
            label: "Open clipboard",
            category: "File",
            default_binding: None,
            action: Some(action::<OpenClipboard>),
        },
        CommandSpec {
            id: "file.open_with_app",
            label: "Open with: associated app",
            category: "File",
            default_binding: None,
            action: Some(action::<OpenWithAssociatedApp>),
        },
        CommandSpec {
            id: "file.open_with_text_edit",
            label: "Open with: text edit",
            category: "File",
            default_binding: None,
            action: Some(action::<OpenWithTextEdit>),
        },
        CommandSpec {
            id: "file.open_with_text_patch",
            label: "Open with: text patch",
            category: "File",
            default_binding: None,
            action: Some(action::<OpenWithTextPatch>),
        },
        CommandSpec {
            id: "file.save_file",
            label: "Save file",
            category: "File",
            default_binding: None,
            action: Some(action::<SaveFile>),
        },
        CommandSpec {
            id: "file.save_file_as",
            label: "Save file as…",
            category: "File",
            default_binding: None,
            action: Some(action::<SaveFileAs>),
        },
        // File: reconcile, to-folder, sync, and step commands
        // Edit
        CommandSpec {
            id: "edit.copy_path_left",
            label: "Copy left path",
            category: "Edit",
            default_binding: None,
            action: Some(action::<crate::menus::CopyPathLeft>),
        },
        CommandSpec {
            id: "edit.copy_path_right",
            label: "Copy right path",
            category: "Edit",
            default_binding: None,
            action: Some(action::<crate::menus::CopyPathRight>),
        },
        CommandSpec {
            id: "edit.select_all_entries",
            label: "Select all entries",
            category: "Edit",
            default_binding: None,
            action: Some(action::<crate::menus::SelectAllEntries>),
        },
        CommandSpec {
            id: "edit.undo",
            label: "Undo",
            category: "Edit",
            default_binding: None,
            action: Some(action::<TextUndo>),
        },
        CommandSpec {
            id: "edit.redo",
            label: "Redo",
            category: "Edit",
            default_binding: None,
            action: Some(action::<TextRedo>),
        },
        CommandSpec {
            id: "edit.cut",
            label: "Cut",
            category: "Edit",
            default_binding: None,
            action: Some(action::<TextCut>),
        },
        CommandSpec {
            id: "edit.copy",
            label: "Copy",
            category: "Edit",
            default_binding: None,
            action: Some(action::<TextCopy>),
        },
        CommandSpec {
            id: "edit.paste",
            label: "Paste",
            category: "Edit",
            default_binding: None,
            action: Some(action::<TextPaste>),
        },
        CommandSpec {
            id: "edit.delete",
            label: "Delete",
            category: "Edit",
            default_binding: None,
            action: Some(action::<TextDelete>),
        },
        CommandSpec {
            id: "edit.select_all_text",
            label: "Select all text",
            category: "Edit",
            default_binding: None,
            action: Some(action::<TextSelectAll>),
        },
        CommandSpec {
            id: "edit.select_section",
            label: "Select section",
            category: "Edit",
            default_binding: None,
            action: Some(action::<SelectSection>),
        },
        CommandSpec {
            id: "edit.delete_line",
            label: "Delete line",
            category: "Edit",
            default_binding: None,
            action: Some(action::<DeleteLine>),
        },
        CommandSpec {
            id: "edit.delete_to_end",
            label: "Delete to line end",
            category: "Edit",
            default_binding: None,
            action: Some(action::<DeleteToEndOfLine>),
        },
        CommandSpec {
            id: "edit.delete_to_start",
            label: "Delete to line start",
            category: "Edit",
            default_binding: None,
            action: Some(action::<DeleteToBeginningOfLine>),
        },
        CommandSpec {
            id: "edit.insert_before",
            label: "Insert line before",
            category: "Edit",
            default_binding: None,
            action: Some(action::<InsertLineBefore>),
        },
        CommandSpec {
            id: "edit.insert_after",
            label: "Insert line after",
            category: "Edit",
            default_binding: None,
            action: Some(action::<InsertLineAfter>),
        },
        CommandSpec {
            id: "edit.indent",
            label: "Indent",
            category: "Edit",
            default_binding: None,
            action: Some(action::<TextIndent>),
        },
        CommandSpec {
            id: "edit.outdent",
            label: "Outdent",
            category: "Edit",
            default_binding: None,
            action: Some(action::<TextOutdent>),
        },
        CommandSpec {
            id: "edit.convert_trim",
            label: "Convert: trim trailing whitespace",
            category: "Edit",
            default_binding: None,
            action: Some(action::<ConvertTrimTrailing>),
        },
        CommandSpec {
            id: "edit.convert_spaces_to_tabs",
            label: "Convert: spaces to tabs",
            category: "Edit",
            default_binding: None,
            action: Some(action::<ConvertSpacesToTabs>),
        },
        CommandSpec {
            id: "edit.convert_tabs_to_spaces",
            label: "Convert: tabs to spaces",
            category: "Edit",
            default_binding: None,
            action: Some(action::<ConvertTabsToSpaces>),
        },
        CommandSpec {
            id: "edit.convert_endings_pc",
            label: "Convert: line endings PC",
            category: "Edit",
            default_binding: None,
            action: Some(action::<ConvertEndingsPc>),
        },
        CommandSpec {
            id: "edit.convert_endings_mac",
            label: "Convert: line endings Mac",
            category: "Edit",
            default_binding: None,
            action: Some(action::<ConvertEndingsMac>),
        },
        CommandSpec {
            id: "edit.convert_endings_unix",
            label: "Convert: line endings Unix",
            category: "Edit",
            default_binding: None,
            action: Some(action::<ConvertEndingsUnix>),
        },
        CommandSpec {
            id: "edit.compare_selection",
            label: "Compare selection to clipboard",
            category: "Edit",
            default_binding: None,
            action: Some(action::<CompareSelectionToClipboard>),
        },
        CommandSpec {
            id: "edit.align_with",
            label: "Align with marked line",
            category: "Edit",
            default_binding: None,
            action: Some(action::<AlignWithAnchor>),
        },
        CommandSpec {
            id: "edit.clear_alignments",
            label: "Clear alignment overrides",
            category: "Edit",
            default_binding: None,
            action: Some(action::<ClearAlignments>),
        },
        CommandSpec {
            id: "edit.isolate_section",
            label: "Isolate section",
            category: "Edit",
            default_binding: None,
            action: Some(action::<IsolateSelection>),
        },
        // File: reconcile, to-folder, sync, and step commands
        CommandSpec {
            id: "file.copy_right",
            label: "Copy to right",
            category: "File",
            default_binding: Some("cmd-r"),
            action: Some(action::<CopyToRight>),
        },
        CommandSpec {
            id: "file.copy_left",
            label: "Copy to left",
            category: "File",
            default_binding: Some("cmd-l"),
            action: Some(action::<CopyToLeft>),
        },
        CommandSpec {
            id: "file.move_right",
            label: "Move to right",
            category: "File",
            default_binding: None,
            action: Some(action::<MoveToRight>),
        },
        CommandSpec {
            id: "file.move_left",
            label: "Move to left",
            category: "File",
            default_binding: None,
            action: Some(action::<MoveToLeft>),
        },
        CommandSpec {
            id: "file.delete_right",
            label: "Delete from right",
            category: "File",
            default_binding: None,
            action: Some(action::<DeleteRightSide>),
        },
        CommandSpec {
            id: "file.delete_left",
            label: "Delete from left",
            category: "File",
            default_binding: None,
            action: Some(action::<DeleteLeftSide>),
        },
        CommandSpec {
            // Deleting both sides runs the two single-side delete items
            // as one batched transfer, behind the same confirm dialog.
            id: "file.delete_both",
            label: "Delete from both sides",
            category: "File",
            default_binding: None,
            action: Some(action::<DeleteBothSide>),
        },
        CommandSpec {
            id: "file.exchange",
            label: "Exchange",
            category: "File",
            default_binding: None,
            action: Some(action::<ExchangeEntry>),
        },
        CommandSpec {
            id: "file.copy_to_other_side",
            label: "Copy to other side",
            category: "File",
            default_binding: None,
            action: Some(action::<CopyToOtherSide>),
        },
        CommandSpec {
            id: "file.copy_to_folder",
            label: "Copy to folder…",
            category: "File",
            default_binding: None,
            action: Some(action::<CopyToFolder>),
        },
        CommandSpec {
            id: "file.move_to_folder",
            label: "Move to folder…",
            category: "File",
            default_binding: None,
            action: Some(action::<MoveToFolder>),
        },
        CommandSpec {
            id: "file.sync_update_right",
            label: "Synchronize: update right",
            category: "File",
            default_binding: None,
            action: Some(action::<SyncUpdateRight>),
        },
        CommandSpec {
            id: "file.sync_update_left",
            label: "Synchronize: update left",
            category: "File",
            default_binding: None,
            action: Some(action::<SyncUpdateLeft>),
        },
        CommandSpec {
            id: "file.sync_update_both",
            label: "Synchronize: update both",
            category: "File",
            default_binding: None,
            action: Some(action::<SyncUpdateBoth>),
        },
        CommandSpec {
            id: "file.sync_mirror_right",
            label: "Synchronize: mirror to right",
            category: "File",
            default_binding: None,
            action: Some(action::<SyncMirrorRight>),
        },
        CommandSpec {
            id: "file.sync_mirror_left",
            label: "Synchronize: mirror to left",
            category: "File",
            default_binding: None,
            action: Some(action::<SyncMirrorLeft>),
        },
        CommandSpec {
            id: "file.copy_file_right_next",
            label: "Copy file to right and open next",
            category: "File",
            default_binding: None,
            action: Some(action::<CopyFileRightNext>),
        },
        CommandSpec {
            id: "file.copy_file_left_next",
            label: "Copy file to left and open next",
            category: "File",
            default_binding: None,
            action: Some(action::<CopyFileLeftNext>),
        },
        CommandSpec {
            id: "file.copy_section_right",
            label: "Copy section to right",
            category: "File",
            default_binding: None,
            action: Some(action::<CopySectionRight>),
        },
        CommandSpec {
            id: "file.copy_section_left",
            label: "Copy section to left",
            category: "File",
            default_binding: None,
            action: Some(action::<CopySectionLeft>),
        },
        CommandSpec {
            id: "merge.take_left",
            label: "Take left",
            category: "Merge",
            default_binding: None,
            action: Some(action::<TakeLeft>),
        },
        CommandSpec {
            id: "merge.take_right",
            label: "Take right",
            category: "Merge",
            default_binding: None,
            action: Some(action::<TakeRight>),
        },
        CommandSpec {
            id: "merge.take_ancestor",
            label: "Take ancestor",
            category: "Merge",
            default_binding: None,
            action: Some(action::<TakeAncestor>),
        },
        CommandSpec {
            id: "merge.save_output",
            label: "Save merge output…",
            category: "Merge",
            default_binding: None,
            action: Some(action::<SaveMergeOutput>),
        },
        CommandSpec {
            id: "merge.set_ancestor",
            label: "Set ancestor…",
            category: "Merge",
            default_binding: None,
            action: Some(action::<SetMergeAncestor>),
        },
        // Search
        CommandSpec {
            id: "search.next_difference",
            label: "Next difference",
            category: "Search",
            default_binding: Some("cmd-]"),
            action: Some(action::<NextDifference>),
        },
        CommandSpec {
            id: "search.prev_difference",
            label: "Previous difference",
            category: "Search",
            default_binding: Some("cmd-["),
            action: Some(action::<PrevDifference>),
        },
        CommandSpec {
            id: "search.next_section",
            label: "Next difference section",
            category: "Search",
            default_binding: None,
            action: Some(action::<NextSection>),
        },
        CommandSpec {
            id: "search.prev_section",
            label: "Previous difference section",
            category: "Search",
            default_binding: None,
            action: Some(action::<PrevSection>),
        },
        CommandSpec {
            id: "search.next_difference_files",
            label: "Next difference files",
            category: "Search",
            default_binding: Some("cmd-alt-]"),
            action: Some(action::<NextDifferenceFiles>),
        },
        CommandSpec {
            id: "search.prev_difference_files",
            label: "Previous difference files",
            category: "Search",
            default_binding: Some("cmd-alt-["),
            action: Some(action::<PrevDifferenceFiles>),
        },
        CommandSpec {
            id: "search.find_filename",
            label: "Find filename…",
            category: "Search",
            default_binding: None,
            action: None,
        },
        CommandSpec {
            id: "search.replace",
            label: "Replace…",
            category: "Search",
            default_binding: None,
            action: None,
        },
        // View
        CommandSpec {
            id: "view.expand_dir",
            label: "Expand directory",
            category: "View",
            default_binding: None,
            action: Some(action::<ExpandDir>),
        },
        CommandSpec {
            id: "view.collapse_dir",
            label: "Collapse directory",
            category: "View",
            default_binding: None,
            action: Some(action::<CollapseDir>),
        },
        CommandSpec {
            id: "view.expand_all",
            label: "Expand all",
            category: "View",
            default_binding: None,
            action: Some(action::<ExpandAll>),
        },
        CommandSpec {
            id: "view.collapse_all",
            label: "Collapse all",
            category: "View",
            default_binding: None,
            action: Some(action::<CollapseAll>),
        },
        CommandSpec {
            id: "view.show_hidden",
            label: "Show/hide hidden items",
            category: "View",
            default_binding: None,
            action: Some(action::<ToggleShowHidden>),
        },
        CommandSpec {
            id: "view.show_same",
            label: "Show/hide identical items",
            category: "View",
            default_binding: None,
            action: Some(action::<ToggleShowSame>),
        },
        CommandSpec {
            id: "view.show_different",
            label: "Show/hide different items",
            category: "View",
            default_binding: None,
            action: Some(action::<ToggleShowDifferent>),
        },
        CommandSpec {
            id: "view.show_left_newer",
            label: "Show/hide left newer items",
            category: "View",
            default_binding: None,
            action: Some(action::<ToggleShowLeftNewer>),
        },
        CommandSpec {
            id: "view.show_right_newer",
            label: "Show/hide right newer items",
            category: "View",
            default_binding: None,
            action: Some(action::<ToggleShowRightNewer>),
        },
        CommandSpec {
            id: "view.show_orphans",
            label: "Show/hide orphan items",
            category: "View",
            default_binding: None,
            action: Some(action::<ToggleShowOrphans>),
        },
        CommandSpec {
            id: "view.show_left_only",
            label: "Show/hide left-only items",
            category: "View",
            default_binding: None,
            action: Some(action::<ToggleShowLeftOnly>),
        },
        CommandSpec {
            id: "view.show_right_only",
            label: "Show/hide right-only items",
            category: "View",
            default_binding: None,
            action: Some(action::<ToggleShowRightOnly>),
        },
        CommandSpec {
            id: "view.show_all",
            label: "Show all",
            category: "View",
            default_binding: None,
            action: Some(action::<ShowAllFilters>),
        },
        CommandSpec {
            id: "view.word_wrap",
            label: "Word wrap",
            category: "View",
            default_binding: None,
            action: Some(action::<ToggleWordWrap>),
        },
        CommandSpec {
            id: "view.show_ancestor",
            label: "Show ancestor pane",
            category: "View",
            default_binding: None,
            action: Some(action::<ToggleMergeAncestor>),
        },
        CommandSpec {
            id: "view.details_text",
            label: "Text details",
            category: "View",
            default_binding: None,
            action: Some(detail_text),
        },
        CommandSpec {
            id: "view.details_hex",
            label: "Hex details",
            category: "View",
            default_binding: None,
            action: Some(detail_hex),
        },
        CommandSpec {
            id: "view.details_alignment",
            label: "Alignment details",
            category: "View",
            default_binding: None,
            action: Some(detail_alignment),
        },
        CommandSpec {
            id: "view.thumbnails",
            label: "Show thumbnails",
            category: "View",
            default_binding: None,
            action: Some(action::<crate::menus::ShowThumbnails>),
        },
        CommandSpec {
            id: "view.log_pane",
            label: "Log pane",
            category: "View",
            default_binding: None,
            action: Some(action::<ToggleLogPane>),
        },
        CommandSpec {
            id: "view.single_row_toolbar",
            label: "Single-row toolbar",
            category: "View",
            default_binding: None,
            action: Some(action::<ToggleSingleRowToolbar>),
        },
        // Folder row context menu
        CommandSpec {
            id: "folder.open",
            label: "Open",
            category: "Folder view",
            default_binding: None,
            action: None,
        },
        CommandSpec {
            id: "folder.quick_compare",
            label: "Quick compare",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<QuickCompare>),
        },
        CommandSpec {
            id: "folder.compare_hash",
            label: "Compare contents by hash",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<CompareContentsHash>),
        },
        CommandSpec {
            id: "folder.compare_meta",
            label: "Compare contents by size & date",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<CompareContentsMeta>),
        },
        CommandSpec {
            id: "folder.rename",
            label: "Rename…",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<RenameEntry>),
        },
        CommandSpec {
            id: "folder.attributes",
            label: "Attributes",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<ShowAttributes>),
        },
        CommandSpec {
            id: "folder.touch",
            label: "Touch",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<TouchEntry>),
        },
        CommandSpec {
            id: "folder.exclude",
            label: "Exclude",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<ExcludeEntry>),
        },
        CommandSpec {
            id: "folder.ignored",
            label: "Ignored",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<ToggleIgnored>),
        },
        CommandSpec {
            id: "folder.new_folder",
            label: "New folder",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<NewFolder>),
        },
        CommandSpec {
            id: "folder.copy_filename",
            label: "Copy filename",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<CopyFilename>),
        },
        CommandSpec {
            id: "folder.refresh",
            label: "Refresh selection",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<RefreshSelection>),
        },
        CommandSpec {
            id: "folder.reveal",
            label: "Reveal in explorer",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<RevealInExplorer>),
        },
        CommandSpec {
            id: "folder.set_base_left",
            label: "Set as base folder (left)",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<SetBaseLeft>),
        },
        CommandSpec {
            id: "folder.set_base_right",
            label: "Set as base folder (right)",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<SetBaseRight>),
        },
        CommandSpec {
            id: "folder.set_base_both",
            label: "Set as base folder (both)",
            category: "Folder view",
            default_binding: None,
            action: Some(action::<SetBaseBoth>),
        },
        // Toolbar
        CommandSpec {
            id: "toolbar.home",
            label: "Home button",
            category: "Toolbar",
            default_binding: None,
            action: None,
        },
        CommandSpec {
            id: "toolbar.sessions",
            label: "Sessions menu",
            category: "Toolbar",
            default_binding: None,
            action: None,
        },
        CommandSpec {
            id: "toolbar.filters",
            label: "Display filters",
            category: "Toolbar",
            default_binding: None,
            action: None,
        },
        CommandSpec {
            id: "toolbar.structure",
            label: "Structure menu",
            category: "Toolbar",
            default_binding: None,
            action: None,
        },
        CommandSpec {
            id: "toolbar.context",
            label: "Context menu",
            category: "Toolbar",
            default_binding: None,
            action: None,
        },
        CommandSpec {
            id: "toolbar.expand",
            label: "Expand all directories",
            category: "Toolbar",
            default_binding: None,
            action: None,
        },
        CommandSpec {
            id: "toolbar.collapse",
            label: "Collapse all directories",
            category: "Toolbar",
            default_binding: None,
            action: None,
        },
        CommandSpec {
            id: "toolbar.select",
            label: "Select menu",
            category: "Toolbar",
            default_binding: None,
            action: None,
        },
        CommandSpec {
            id: "toolbar.files",
            label: "Files menu",
            category: "Toolbar",
            default_binding: None,
            action: None,
        },
        CommandSpec {
            id: "toolbar.refresh",
            label: "Reload comparison",
            category: "Toolbar",
            default_binding: None,
            action: None,
        },
        CommandSpec {
            id: "toolbar.rules",
            label: "Session rules",
            category: "Toolbar",
            default_binding: None,
            action: None,
        },
        // Options
        CommandSpec {
            id: "options.open",
            label: "Options…",
            category: "Options",
            default_binding: None,
            action: Some(action::<Options>),
        },
        CommandSpec {
            id: "options.file_formats",
            label: "File formats…",
            category: "Options",
            default_binding: None,
            action: Some(action::<FileFormats>),
        },
        CommandSpec {
            id: "options.sync_presets",
            label: "Folder Sync presets…",
            category: "Options",
            default_binding: None,
            action: Some(action::<SyncPresets>),
        },
        CommandSpec {
            id: "options.profiles",
            label: "Profiles…",
            category: "Options",
            default_binding: None,
            action: Some(action::<Profiles>),
        },
        // Help
        CommandSpec {
            id: "help.contents",
            label: "Help contents",
            category: "Help",
            default_binding: None,
            action: Some(action::<HelpContents>),
        },
        CommandSpec {
            id: "help.about",
            label: "About cocomo",
            category: "Help",
            default_binding: None,
            action: Some(action::<About>),
        },
    ]
}

/// Look up a command by its persisted id.
pub fn find_command(id: &str) -> Option<CommandSpec> {
    all_commands().into_iter().find(|spec| spec.id == id)
}

/// The view-scoped bindings that used to be registered by the views
/// themselves.
///
/// They live here because [`rebuild_keymap`] clears the whole keymap: any
/// binding not re-added here would silently disappear the first time the
/// user changes a binding.
pub fn view_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("j", NextLine, Some("TextCompare")),
        KeyBinding::new("k", PrevLine, Some("TextCompare")),
        KeyBinding::new("down", NextLine, Some("TextCompare")),
        KeyBinding::new("up", PrevLine, Some("TextCompare")),
        KeyBinding::new("cmd-shift-]", NextSection, Some("TextCompare")),
        KeyBinding::new("cmd-shift-[", PrevSection, Some("TextCompare")),
        KeyBinding::new("cmd-alt-]", NextDifferenceFiles, Some("TextCompare")),
        KeyBinding::new("cmd-alt-[", PrevDifferenceFiles, Some("TextCompare")),
        KeyBinding::new("j", NextMergeSection, Some("TextMerge")),
        KeyBinding::new("k", PrevMergeSection, Some("TextMerge")),
        KeyBinding::new("down", NextMergeSection, Some("TextMerge")),
        KeyBinding::new("up", PrevMergeSection, Some("TextMerge")),
        KeyBinding::new("cmd-shift-]", NextConflict, Some("TextMerge")),
        KeyBinding::new("cmd-shift-[", PrevConflict, Some("TextMerge")),
    ]
}

/// Every binding the application registers at startup, before overrides.
pub fn base_bindings() -> Vec<KeyBinding> {
    let mut bindings = menu_bindings();
    bindings.extend(folder_compare_bindings());
    bindings.extend(home::home_bindings());
    bindings.extend(view_bindings());
    bindings
}

/// Parse a user-entered keystroke sequence into a binding for `spec`.
///
/// Returns a readable message rather than panicking, because the input
/// comes from the Options dialog.
pub fn parse_binding(
    spec: &CommandSpec,
    keystrokes: &str,
    cx: &App,
) -> std::result::Result<KeyBinding, String> {
    let action = spec
        .action
        .ok_or_else(|| "This command has no key binding.".to_string())?;
    KeyBinding::load(keystrokes.trim(), action(), None, false, None, cx)
        .map_err(|err: InvalidKeystrokeError| err.to_string())
}

/// Whether the command should appear in the menus and toolbar.
pub fn is_visible(id: &str, cx: &App) -> bool {
    options::command_visible(id, cx)
}

/// The bindings the framework registered during `gpui::init`.
///
/// [`rebuild_keymap`] has to clear the keymap to replace a binding, and
/// clearing wipes everything — including the input, dialog, and popup
/// bindings the component layer owns. Capturing them once at startup lets
/// the rebuild restore them verbatim.
pub struct BaseKeymap(Vec<KeyBinding>);

impl gpui::Global for BaseKeymap {}

/// Capture the framework bindings.
///
/// Must run after `gpui::init` and before the application registers any
/// binding of its own, so the snapshot holds only the framework's.
pub fn capture_framework_bindings(cx: &mut App) {
    let bindings: Vec<KeyBinding> =
        cx.key_bindings().borrow().bindings().cloned().collect();
    cx.set_global(BaseKeymap(bindings));
}

/// Rebuild the global keymap from the base bindings plus the user's
/// overrides, then apply the resulting keymap to every window.
pub fn rebuild_keymap(cx: &mut App) {
    let framework = cx.global::<BaseKeymap>().0.clone();
    let opts = cx.global::<AppOptions>().clone();
    let specs = all_commands();

    // Actions whose binding the user replaced or cleared must not keep
    // their default binding around: GPUI would match both.
    let mut cleared: Vec<TypeId> = Vec::new();
    let mut additions: Vec<KeyBinding> = Vec::new();
    for spec in &specs {
        let Some(keystrokes) = opts.command_binding(spec.id) else {
            continue;
        };
        let Some(action) = spec.action else { continue };
        if keystrokes.is_empty() {
            // The sentinel empty binding means "unbound".
            cleared.push(action().type_id());
            continue;
        }
        let Ok(binding) = parse_binding(spec, keystrokes, cx) else {
            // An unparseable override is ignored; the default stays.
            continue;
        };
        cleared.push(binding.action().type_id());
        additions.push(binding);
    }

    let mut bindings = framework;
    bindings.extend(base_bindings());
    bindings.retain(|binding| !cleared.contains(&binding.action().type_id()));
    bindings.extend(additions);

    cx.clear_key_bindings();
    cx.bind_keys(bindings);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_ids_are_unique() {
        let specs = all_commands();
        let mut ids: Vec<&str> = specs.iter().map(|s| s.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate command id");
    }

    #[gpui::test]
    fn every_default_binding_parses(cx: &mut App) {
        for spec in all_commands() {
            if let Some(keystrokes) = spec.default_binding {
                assert!(
                    spec.is_bindable(),
                    "{} declares a binding but no action",
                    spec.id
                );
                assert!(
                    parse_binding(&spec, keystrokes, cx).is_ok(),
                    "{} has an unparseable default binding",
                    spec.id
                );
            }
        }
    }

    #[gpui::test]
    fn an_unparseable_override_is_rejected_not_panicking(cx: &mut App) {
        let spec = find_command("session.save").unwrap();
        // A trailing dash is the literal `-` key; only a dangling modifier
        // chain like this fails to parse.
        assert!(parse_binding(&spec, "cmd-a-b", cx).is_err());
    }

    #[test]
    fn base_bindings_cover_the_menu_shortcuts() {
        let bindings = base_bindings();
        let actions: Vec<&str> =
            bindings.iter().map(|b| b.action().name()).collect();
        for expected in [
            "menu::NewCompare",
            "menu::OpenSession",
            "menu::SaveSession",
            "menu::CloseSession",
        ] {
            assert!(
                actions.contains(&expected),
                "missing default binding for {expected}"
            );
        }
    }
}
