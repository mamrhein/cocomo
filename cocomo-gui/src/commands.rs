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

use gpui::{Action, App, InvalidKeystrokeError, KeyBinding};

use crate::{
    home,
    menus::{
        About, CloseSession, FileFormats, MergeFiles, NewCompare, OpenSession,
        Options, ReloadCompare, RestoreWorkspace, SaveFile, SaveFileAs,
        SaveSession, SaveSessionAs, SaveWorkspace, SessionSettings,
        ToggleLogPane, ToggleWordWrap, menu_bindings,
    },
    options::{self, AppOptions},
    text_diff, text_merge,
    ui::{
        CopyToLeft, CopyToRight, DeleteLeftSide, DeleteRightSide, MoveToLeft,
        MoveToRight, NextDifference, PrevDifference, folder_compare_bindings,
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

/// Every command exposed to the Options dialog, in display order.
pub fn all_commands() -> Vec<CommandSpec> {
    vec![
        // Session
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
            id: "session.reload",
            label: "Reload / recompare files",
            category: "Session",
            default_binding: Some("cmd-shift-r"),
            action: Some(action::<ReloadCompare>),
        },
        CommandSpec {
            id: "session.merge",
            label: "Merge files…",
            category: "Session",
            default_binding: None,
            action: Some(action::<MergeFiles>),
        },
        // File
        CommandSpec {
            id: "file.new_compare",
            label: "New compare…",
            category: "File",
            default_binding: Some("cmd-n"),
            action: Some(action::<NewCompare>),
        },
        CommandSpec {
            id: "file.open_session",
            label: "Open session…",
            category: "File",
            default_binding: Some("cmd-o"),
            action: Some(action::<OpenSession>),
        },
        CommandSpec {
            id: "file.save_workspace",
            label: "Save workspace…",
            category: "File",
            default_binding: None,
            action: Some(action::<SaveWorkspace>),
        },
        CommandSpec {
            id: "file.restore_workspace",
            label: "Restore workspace…",
            category: "File",
            default_binding: None,
            action: Some(action::<RestoreWorkspace>),
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
        CommandSpec {
            id: "file.close_session",
            label: "Close session",
            category: "File",
            default_binding: Some("cmd-w"),
            action: Some(action::<CloseSession>),
        },
        // Actions
        CommandSpec {
            id: "actions.copy_left",
            label: "Copy to left",
            category: "Actions",
            default_binding: Some("cmd-l"),
            action: Some(action::<CopyToLeft>),
        },
        CommandSpec {
            id: "actions.copy_right",
            label: "Copy to right",
            category: "Actions",
            default_binding: Some("cmd-r"),
            action: Some(action::<CopyToRight>),
        },
        CommandSpec {
            id: "actions.move_left",
            label: "Move to left",
            category: "Actions",
            default_binding: None,
            action: Some(action::<MoveToLeft>),
        },
        CommandSpec {
            id: "actions.move_right",
            label: "Move to right",
            category: "Actions",
            default_binding: None,
            action: Some(action::<MoveToRight>),
        },
        CommandSpec {
            id: "actions.delete_left",
            label: "Delete from left",
            category: "Actions",
            default_binding: None,
            action: Some(action::<DeleteLeftSide>),
        },
        CommandSpec {
            id: "actions.delete_right",
            label: "Delete from right",
            category: "Actions",
            default_binding: None,
            action: Some(action::<DeleteRightSide>),
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
        // View
        CommandSpec {
            id: "view.word_wrap",
            label: "Word wrap",
            category: "View",
            default_binding: None,
            action: Some(action::<ToggleWordWrap>),
        },
        CommandSpec {
            id: "view.thumbnails",
            label: "Show thumbnails",
            category: "View",
            default_binding: None,
            action: None,
        },
        CommandSpec {
            id: "view.log_pane",
            label: "Log pane",
            category: "View",
            default_binding: None,
            action: Some(action::<ToggleLogPane>),
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
        // Tools
        CommandSpec {
            id: "tools.options",
            label: "Options…",
            category: "Tools",
            default_binding: None,
            action: Some(action::<Options>),
        },
        CommandSpec {
            id: "tools.file_formats",
            label: "File formats…",
            category: "Tools",
            default_binding: None,
            action: Some(action::<FileFormats>),
        },
        // Help
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
#[allow(dead_code)]
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
        KeyBinding::new("j", text_diff::NextLine, Some("TextCompare")),
        KeyBinding::new("k", text_diff::PrevLine, Some("TextCompare")),
        KeyBinding::new("down", text_diff::NextLine, Some("TextCompare")),
        KeyBinding::new("up", text_diff::PrevLine, Some("TextCompare")),
        KeyBinding::new(
            "cmd-shift-]",
            crate::menus::NextSection,
            Some("TextCompare"),
        ),
        KeyBinding::new(
            "cmd-shift-[",
            crate::menus::PrevSection,
            Some("TextCompare"),
        ),
        KeyBinding::new("j", text_merge::NextMergeSection, Some("TextMerge")),
        KeyBinding::new("k", text_merge::PrevMergeSection, Some("TextMerge")),
        KeyBinding::new(
            "down",
            text_merge::NextMergeSection,
            Some("TextMerge"),
        ),
        KeyBinding::new("up", text_merge::PrevMergeSection, Some("TextMerge")),
        KeyBinding::new(
            "cmd-shift-]",
            text_merge::NextConflict,
            Some("TextMerge"),
        ),
        KeyBinding::new(
            "cmd-shift-[",
            text_merge::PrevConflict,
            Some("TextMerge"),
        ),
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
