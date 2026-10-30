// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! UI components for the folder compare view.
//!
//! This module provides the directory comparison view, including
//! status indicators, sortable entry columns, navigation, and entry
//! rendering.

use cocomo_lib::{DirEntryStatus, Endpoint, TransferAction};
use gpui::{
    Action, Anchor, AnyElement, App, ClipboardItem, Context, Div, Entity,
    FocusHandle, Focusable, FontWeight, Hsla, KeyBinding, Render,
    ScrollStrategy, SharedString, Subscription, UniformListScrollHandle,
    Window, actions,
    assets::IconName,
    base::Disableable as _,
    component::{
        ActiveTheme, Sizable as _, Theme, WindowExt as _,
        breadcrumb::{Breadcrumb, BreadcrumbItem},
        button::{Button, ButtonVariant, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{Input, InputEvent, InputState},
        menu::{DropdownMenu as _, PopupMenuItem},
        popover::Popover,
        spinner::Spinner,
        status_bar::StatusBar,
        toolbar::{Toolbar, ToolbarGroup},
        v_flex,
    },
    div,
    prelude::*,
};

use crate::{
    color::Side,
    log::{self, LogLevel},
    menus::{
        CopyPathLeft, CopyPathRight, ReloadCompare, SaveSession, StopCompare,
        ToggleShowDifferent, ToggleShowOrphans, ToggleShowSame,
    },
    session_manager::GuiSessionManager,
    state::{AppState, BaseSide, StatusCategory},
};

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

actions!(
    folder_compare,
    [
        SelectNext,
        SelectPrev,
        EnterDir,
        LeaveDir,
        ToggleExpand,
        Reload,
        NextDifference,
        PrevDifference,
        SwapSides,
        CopyToLeft,
        CopyToRight,
        CopyToOtherSide,
        MoveToLeft,
        MoveToRight,
        ExchangeEntry,
        DeleteLeftSide,
        DeleteRightSide,
        CopyToFolder,
        MoveToFolder,
        RenameEntry,
        ShowAttributes,
        TouchEntry,
        ExcludeEntry,
        ToggleIgnored,
        NewFolder,
        CopyFilename,
        RefreshSelection,
        QuickCompare,
        CompareContentsHash,
        CompareContentsMeta,
        RevealInExplorer,
        SyncUpdateRight,
        SyncUpdateLeft,
        SyncUpdateBoth,
        SyncMirrorRight,
        SyncMirrorLeft,
        SetBaseLeft,
        SetBaseRight,
        SetBaseBoth,
        GoBack,
        GoForward,
    ]
);

/// Action that points one side of the comparison at a new path, from the
/// per-pane path bar's history dropdown.
#[derive(Action, Clone, Debug, PartialEq)]
#[action(namespace = folder_compare, no_json)]
pub struct SetSidePath {
    /// The side whose endpoint changes.
    pub side: crate::color::Side,
    /// The new path for that side.
    pub path: String,
}

/// Action to open a text diff for a file in a new tab.
#[derive(Action, Clone, Debug, PartialEq)]
#[action(namespace = folder_compare, no_json)]
pub struct OpenFileDiff {
    /// Left file endpoint.
    pub left_path: Endpoint,
    /// Right file endpoint (optional).
    pub right_path: Option<Endpoint>,
}

/// Action to open a directory compare in a new tab.
#[derive(Action, Clone, Debug, PartialEq)]
#[action(namespace = folder_compare, no_json)]
pub struct OpenDirDiff {
    /// Left directory endpoint.
    pub left_path: Endpoint,
    /// Right directory endpoint.
    pub right_path: Endpoint,
}
// ---------------------------------------------------------------------------

/// Register key bindings for folder comparison navigation.
pub fn folder_compare_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("j", SelectNext, None),
        KeyBinding::new("k", SelectPrev, None),
        KeyBinding::new("down", SelectNext, None),
        KeyBinding::new("up", SelectPrev, None),
        KeyBinding::new("enter", EnterDir, None),
        KeyBinding::new("backspace", LeaveDir, None),
        // Explorer-style tree expansion on the horizontal arrows.
        KeyBinding::new("right", ToggleExpand, None),
        KeyBinding::new("left", ToggleExpand, None),
        KeyBinding::new("r", Reload, None),
        // Walk the differences: the single highest-value navigation,
        // matching the familiar next/previous-difference commands.
        KeyBinding::new("cmd-]", NextDifference, None),
        KeyBinding::new("f7", NextDifference, None),
        KeyBinding::new("cmd-[", PrevDifference, None),
        KeyBinding::new("shift-f7", PrevDifference, None),
        // Reconcile the selected entry toward each side. The spec pins
        // `cmd-r`/`ctrl-r` to copy-right and `cmd-l` to copy-left; reload
        // lives on `cmd-shift-r` (registered with the menu bindings).
        KeyBinding::new("cmd-1", CopyToLeft, None),
        KeyBinding::new("cmd-2", CopyToRight, None),
        KeyBinding::new("cmd-l", CopyToLeft, None),
        KeyBinding::new("ctrl-l", CopyToLeft, None),
        KeyBinding::new("cmd-r", CopyToRight, None),
        KeyBinding::new("ctrl-r", CopyToRight, None),
        // Step through the history of compared base-folder pairs.
        KeyBinding::new("alt-left", GoBack, None),
        KeyBinding::new("alt-right", GoForward, None),
    ]
}

// ---------------------------------------------------------------------------
// Folder Compare View
// ---------------------------------------------------------------------------

/// The main folder comparison view.
///
/// Renders only its own regions (path header, entry list, status bar).
/// The window chrome (tab bar, toolbar) and keyboard focus are owned by
/// [`crate::tabview::WindowRoot`]; this view never requests focus from
/// `render`.
pub struct FolderCompareView {
    /// Handle to the application state.
    state: Entity<AppState>,
    /// Handle to the session manager.
    session_manager: Entity<GuiSessionManager>,
    /// Scroll handle shared by both tree panes.
    scroll_handle: UniformListScrollHandle,
    /// Editable name-filter combo in toolbar row 2.
    ///
    /// Created lazily on the first row-2 render: the row is rendered by
    /// `WindowRoot`, which is where a window handle is available for
    /// `InputState`.
    filter_input: Option<Entity<InputState>>,
    /// Subscription that commits the filter input on Enter.
    _filter_subscription: Option<Subscription>,
    /// Editable left-side path field of the pane path bar.
    left_path_input: Option<Entity<InputState>>,
    /// Editable right-side path field of the pane path bar.
    right_path_input: Option<Entity<InputState>>,
    /// Subscriptions committing the path fields on Enter.
    _path_subscriptions: Vec<Subscription>,
}

impl FolderCompareView {
    /// Create a new folder compare view.
    pub fn new(
        state: Entity<AppState>,
        session_manager: Entity<GuiSessionManager>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Trigger auto-load of comparison if not already loaded.
        state.update(cx, |state, cx| {
            state.trigger_auto_load(cx);
        });

        Self {
            state,
            session_manager,
            scroll_handle: UniformListScrollHandle::new(),
            filter_input: None,
            _filter_subscription: None,
            left_path_input: None,
            right_path_input: None,
            _path_subscriptions: Vec::new(),
        }
    }

    /// Return the focus handle for keyboard navigation.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle(cx).clone()
    }

    /// Return the application-state entity this view renders.
    ///
    /// The window root reads the filter state from here to keep the View
    /// menu's checkmarks in sync with the active tab.
    pub fn folder_state(&self) -> Entity<AppState> {
        self.state.clone()
    }

    /// Render toolbar row 2 for this folder tab: pair controls, the
    /// name-filter combo with history, the filter-editor trigger,
    /// section navigation, and reload.
    ///
    /// Called by `WindowRoot` through [`crate::tabview::TabEntity`]; the
    /// controls update [`AppState`] directly rather than dispatching
    /// actions, because a toolbar click does not move keyboard focus to
    /// the (possibly unfocused) view.
    pub fn render_view_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Create the filter input on first render and commit its value
        // when the user presses Enter.
        if self.filter_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Name filter, e.g. *.rs")
            });
            self._filter_subscription =
                Some(cx.subscribe(&input, |this, input, event, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        let value = input.read(cx).value().to_string();
                        this.state.update(cx, |state, cx| {
                            state.push_filter_pattern(&value, cx);
                        });
                    }
                }));
            self.filter_input = Some(input);
        }
        let input = self.filter_input.clone().expect("just created");

        let (pattern, history) = {
            let state = self.state.read(cx);
            (
                state.filter_pattern().unwrap_or("").to_string(),
                state.filter_history().to_vec(),
            )
        };
        // Seed the input once from the session's stored pattern.
        if !pattern.is_empty() && input.read(cx).value().is_empty() {
            input.update(cx, |state, cx| {
                state.set_value(pattern.clone(), window, cx);
            });
        }

        let theme = cx.theme();

        let swap_btn = Button::new("row2_swap")
            .icon(IconName::ArrowLeftRight)
            .ghost()
            .small()
            .tooltip("Swap sides")
            .accessibility_label("Swap sides")
            .on_click({
                let state = self.state.clone();
                move |_, _, cx| {
                    state.update(cx, |state, cx| {
                        state.swap_sides(cx);
                    });
                }
            });

        // Stop is only offered while a scan or content pass is in
        // flight; it aborts the task and keeps the displayed results.
        let stop_btn = {
            let loading = self.state.read(cx).is_loading();
            Button::new("row2_stop")
                .icon(IconName::CircleStop)
                .ghost()
                .small()
                .disabled(!loading)
                .tooltip("Stop comparison")
                .accessibility_label("Stop comparison")
                .on_click({
                    let state = self.state.clone();
                    move |_, _, cx| {
                        state.update(cx, |state, cx| {
                            state.stop(cx);
                        });
                    }
                })
        };

        // History dropdown: newest first; picking an entry makes it the
        // current pattern again.
        let state_entity = self.state.clone();
        let history_input = input.clone();
        let history_btn = Button::new("row2_filter_history")
            .icon(IconName::ChevronDown)
            .ghost()
            .small()
            .disabled(history.is_empty())
            .tooltip("Filter history")
            .accessibility_label("Filter history")
            .dropdown_menu(move |menu, _, _| {
                let input = history_input.clone();
                let state = state_entity.clone();
                let mut menu = menu;
                for pattern in history.iter().rev() {
                    let input = input.clone();
                    let state = state.clone();
                    let pattern = pattern.clone();
                    menu = menu.item(
                        PopupMenuItem::new(pattern.clone()).on_click(
                            move |_, window, cx| {
                                input.update(cx, |input, cx| {
                                    input.set_value(
                                        pattern.clone(),
                                        window,
                                        cx,
                                    );
                                });
                                state.update(cx, |state, cx| {
                                    state.push_filter_pattern(&pattern, cx);
                                });
                            },
                        ),
                    );
                }
                menu
            });

        // The full filter editor is a later phase; the dialog container
        // already exists so the entry point is discoverable.
        let filters_btn = Button::new("row2_filters_dialog")
            .icon(IconName::Ellipsis)
            .ghost()
            .small()
            .tooltip("Open filter editor")
            .accessibility_label("Open filter editor")
            .on_click(|_, window, cx| {
                window.open_dialog(cx, |dialog, _, _| {
                    dialog.title("Filters").child(
                        "The full filter editor arrives in a later phase.",
                    )
                });
            });

        // Difference sections are a text-view concept; the buttons are
        // placeholders here and wired for text tabs in a later phase.
        let prev_section_btn = Button::new("row2_prev_section")
            .icon(IconName::ChevronsUp)
            .ghost()
            .small()
            .disabled(true)
            .tooltip("Previous difference section (coming soon)")
            .accessibility_label("Previous difference section");
        let next_section_btn = Button::new("row2_next_section")
            .icon(IconName::ChevronsDown)
            .ghost()
            .small()
            .disabled(true)
            .tooltip("Next difference section (coming soon)")
            .accessibility_label("Next difference section");

        let reload_btn = Button::new("row2_reload")
            .icon(IconName::RefreshCw)
            .ghost()
            .small()
            .tooltip("Reload comparison")
            .accessibility_label("Reload comparison")
            .on_click({
                let state = self.state.clone();
                move |_, _, cx| {
                    state.update(cx, |state, cx| {
                        state.reload(cx);
                    });
                }
            });

        Toolbar::new("view-toolbar")
            .child(
                ToolbarGroup::new("view-toolbar-pair")
                    .label("Pair")
                    .child(swap_btn)
                    .child(stop_btn),
            )
            .child(
                ToolbarGroup::new("view-toolbar-filter")
                    .label("Filters")
                    .content(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Input::new(&input)
                                    .small()
                                    .w_64()
                                    .text_color(theme.foreground),
                            )
                            .child(history_btn)
                            .child(filters_btn),
                    ),
            )
            .child(
                ToolbarGroup::new("view-toolbar-sections")
                    .label("Sections")
                    .child(prev_section_btn)
                    .child(next_section_btn),
            )
            .child(
                ToolbarGroup::new("view-toolbar-reload")
                    .label("Reload")
                    .child(reload_btn),
            )
            .into_any_element()
    }

    /// Handle the select-next action.
    fn select_next(
        &mut self,
        _: &SelectNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.select_next(cx);
        });
    }

    /// Handle the select-previous action.
    fn select_prev(
        &mut self,
        _: &SelectPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.select_previous(cx);
        });
    }

    /// Handle the enter action: a directory replaces the current view
    /// with its subtree; a file opens a child text-diff session.
    fn enter_dir(
        &mut self,
        _: &EnterDir,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_dir = self.state.read(cx).selected_entry().is_some_and(|e| {
            e.left.as_ref().is_some_and(|l| l.is_dir)
                || e.right.as_ref().is_some_and(|r| r.is_dir)
        });
        if is_dir {
            self.state.update(cx, |state, cx| {
                state.navigate_into_subdir(cx);
            });
            return;
        }
        let endpoints = self.state.read(cx).selected_endpoints();
        let has_right = self
            .state
            .read(cx)
            .selected_entry()
            .is_some_and(|e| e.right.is_some());
        if let Some((left, right)) = endpoints {
            cx.dispatch_action(&OpenFileDiff {
                left_path: left,
                right_path: has_right.then_some(right),
            });
        }
    }

    /// Handle the expand/collapse action on the selected directory.
    fn toggle_expand(
        &mut self,
        _: &ToggleExpand,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = self.state.read(cx).selected_index();
        let path = {
            let state = self.state.read(cx);
            state.row_at(path).map(|row| row.path().to_string())
        };
        let Some(path) = path else { return };
        let is_dir = self.state.read(cx).selected_entry().is_some_and(|e| {
            e.left.as_ref().is_some_and(|l| l.is_dir)
                || e.right.as_ref().is_some_and(|r| r.is_dir)
        });
        if !is_dir {
            return;
        }
        self.state.update(cx, |state, cx| {
            state.toggle_expanded(&path, cx);
        });
    }

    /// Handle the leave-directory action.
    fn leave_dir(
        &mut self,
        _: &LeaveDir,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.navigate_up(cx);
        });
    }

    /// Handle the reload action.
    fn reload(&mut self, _: &Reload, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.reload(cx);
        });
    }

    /// Handle the ReloadCompare action dispatched from menus or toolbar.
    fn handle_reload_compare(
        &mut self,
        _: &ReloadCompare,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.reload(cx);
        });
    }

    /// Handle the StopCompare action dispatched from the Session menu:
    /// abort the in-flight comparison, keeping the displayed results.
    fn handle_stop_compare(
        &mut self,
        _: &StopCompare,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.stop(cx);
        });
    }

    /// Handle the SaveSession action dispatched from menus or toolbar.
    fn handle_save_session(
        &mut self,
        _: &SaveSession,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.state.read(cx).title().to_string();
        let config = self.state.read(cx).to_config(name.clone());

        self.session_manager.update(cx, |mgr, cx| {
            mgr.update_active_config(config, cx);
        });
    }

    /// Handle OpenFileDiff: route the file pair through the file-format
    /// registry (UI spec section 7.2) and open the matching view.
    fn handle_open_file_diff(
        &mut self,
        action: &OpenFileDiff,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let right_path = action.right_path.clone().unwrap_or_else(|| {
            // Fallback: use the same file on the other side.
            action.left_path.clone()
        });
        let kind = crate::file_formats::FileFormatRegistry::view_kind(
            action.left_path.path(),
            cx,
        );
        if !kind.is_available() {
            // The table, hex, and picture views arrive in later phases;
            // say so instead of silently opening the wrong editor.
            log::push(
                LogLevel::Info,
                format!(
                    "The {kind:?} view is not available yet; {} was not \
                     opened.",
                    action.left_path.display()
                ),
                cx,
            );
            return;
        }
        let parent = self.state.downgrade();
        self.session_manager.update(cx, |mgr, cx| {
            mgr.add_text_diff_tab(
                action.left_path.clone(),
                right_path,
                Some(parent),
                cx,
            );
        });
    }

    /// Handle OpenDirDiff: dispatch to session manager to create a dir diff
    /// tab.
    fn handle_open_dir_diff(
        &mut self,
        action: &OpenDirDiff,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_manager.update(cx, |mgr, cx| {
            mgr.add_dir_diff_tab(
                action.left_path.clone(),
                action.right_path.clone(),
                cx,
            );
        });
    }

    /// Handle the CopyPathLeft action: copy the selected entry's left
    /// path to the clipboard.
    fn handle_copy_path_left(
        &mut self,
        _: &CopyPathLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = self.state.read(cx).selected_left_path() {
            cx.write_to_clipboard(ClipboardItem::new_string(path));
        }
    }

    /// Handle the CopyPathRight action: copy the selected entry's right
    /// path to the clipboard.
    fn handle_copy_path_right(
        &mut self,
        _: &CopyPathRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = self.state.read(cx).selected_right_path() {
            cx.write_to_clipboard(ClipboardItem::new_string(path));
        }
    }

    /// Scroll the list so the current selection is centered. Used after
    /// difference walking, which can jump far from the visible range.
    fn scroll_to_selection(&self, cx: &App) {
        let selected = self.state.read(cx).selected_index();
        self.scroll_handle
            .scroll_to_item(selected, ScrollStrategy::Center);
    }

    /// Handle the next-difference action: jump to the following non-identical
    /// entry among the visible rows.
    fn handle_next_difference(
        &mut self,
        _: &NextDifference,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.select_next_difference(cx);
        });
        self.scroll_to_selection(cx);
    }

    /// Handle the previous-difference action.
    fn handle_prev_difference(
        &mut self,
        _: &PrevDifference,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.select_previous_difference(cx);
        });
        self.scroll_to_selection(cx);
    }

    /// Handle the swap-sides action.
    fn handle_swap_sides(
        &mut self,
        _: &SwapSides,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.swap_sides(cx);
        });
    }

    /// Run the transfer `action` on the selected entry.
    ///
    /// Orphans transfer immediately; operations that overwrite an
    /// existing entry or delete content first ask for confirmation in
    /// an alert dialog, per the spec's overwrite rule.
    fn run_transfer_action(
        &mut self,
        action: TransferAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(plan) = self.state.read(cx).transfer_plan(action) else {
            self.state.update(cx, |state, cx| {
                state.explain_blocked_transfer(cx);
            });
            return;
        };
        if !plan.confirm {
            self.state
                .update(cx, |state, cx| state.run_transfer(plan, cx));
            return;
        }
        let name = plan.item.name.clone();
        let verb = match action {
            TransferAction::DeleteLeft | TransferAction::DeleteRight => {
                "delete"
            }
            TransferAction::MoveLeft | TransferAction::MoveRight => "move",
            _ => "copy",
        };
        let side = match action {
            TransferAction::CopyLeft
            | TransferAction::MoveLeft
            | TransferAction::DeleteLeft => "left",
            _ => "right",
        };
        let (title, description, ok_text, danger) = match verb {
            "delete" => (
                format!("Delete from the {side}?"),
                format!(
                    "\"{name}\" is removed from the {side} side. This cannot \
                     be undone."
                ),
                "Delete",
                true,
            ),
            "move" => (
                format!("Move to the {side}?"),
                if plan.confirm {
                    format!(
                        "\"{name}\" already exists on the {side} side and \
                         will be replaced. The source copy is removed."
                    )
                } else {
                    format!(
                        "\"{name}\" is moved to the {side} side; the source \
                         copy is removed."
                    )
                },
                "Move",
                false,
            ),
            _ => (
                format!("Overwrite on the {side}?"),
                format!(
                    "\"{name}\" already exists on the {side} side and will \
                     be replaced."
                ),
                "Replace",
                false,
            ),
        };
        let state = self.state.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            // Clone per builder call: the builder is an `Fn` closure
            // that may run on every layer rebuild.
            let state = state.clone();
            let plan = plan.clone();
            let mut alert = alert
                .confirm()
                .title(title.clone())
                .description(description.clone())
                .ok_text(ok_text);
            if danger {
                alert = alert.ok_variant(ButtonVariant::Danger);
            }
            alert.on_ok(move |_, _, cx| {
                state.update(cx, |state, cx| {
                    state.run_transfer(plan.clone(), cx);
                });
                true
            })
        });
    }

    /// Handle copy-to-left: reconcile the selected entry by copying it
    /// from the right side to the left.
    fn handle_copy_to_left(
        &mut self,
        _: &CopyToLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_transfer_action(TransferAction::CopyLeft, window, cx);
    }

    /// Handle copy-to-right: reconcile the selected entry by copying it
    /// from the left side to the right.
    fn handle_copy_to_right(
        &mut self,
        _: &CopyToRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_transfer_action(TransferAction::CopyRight, window, cx);
    }

    /// Handle move-to-left: move the right entry to the left side.
    fn handle_move_to_left(
        &mut self,
        _: &MoveToLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_transfer_action(TransferAction::MoveLeft, window, cx);
    }

    /// Handle move-to-right: move the left entry to the right side.
    fn handle_move_to_right(
        &mut self,
        _: &MoveToRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_transfer_action(TransferAction::MoveRight, window, cx);
    }

    /// Handle delete-left: remove the entry from the left side.
    fn handle_delete_left(
        &mut self,
        _: &DeleteLeftSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_transfer_action(TransferAction::DeleteLeft, window, cx);
    }

    /// Handle delete-right: remove the entry from the right side.
    fn handle_delete_right(
        &mut self,
        _: &DeleteRightSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_transfer_action(TransferAction::DeleteRight, window, cx);
    }

    /// Handle the set-base-folder actions: re-root the session at the
    /// selected entry for the requested side(s).
    fn handle_set_base(
        &mut self,
        side: BaseSide,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.set_base_folder(side, cx);
        });
    }

    /// Handle set-base-left.
    fn handle_set_base_left(
        &mut self,
        _: &SetBaseLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_set_base(BaseSide::Left, window, cx);
    }

    /// Handle set-base-right.
    fn handle_set_base_right(
        &mut self,
        _: &SetBaseRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_set_base(BaseSide::Right, window, cx);
    }

    /// Handle set-base-both.
    fn handle_set_base_both(
        &mut self,
        _: &SetBaseBoth,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_set_base(BaseSide::Both, window, cx);
    }

    /// Handle the back action: step to the previously compared
    /// base-folder pair.
    fn handle_go_back(
        &mut self,
        _: &GoBack,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| state.go_back(cx));
    }

    /// Handle the forward action.
    fn handle_go_forward(
        &mut self,
        _: &GoForward,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| state.go_forward(cx));
    }

    // -----------------------------------------------------------------------
    // Row commands (spec section 4.4)
    // -----------------------------------------------------------------------

    /// Handle a path-history selection: point one side at the picked
    /// path and re-compare.
    fn handle_set_side_path(
        &mut self,
        action: &SetSidePath,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = action.path.clone();
        self.state.update(cx, |state, cx| {
            state.set_side_path(action.side, &path, cx);
        });
    }

    /// Handle Quick Compare: open a child session over the selected
    /// entry. Directories get a metadata-only folder compare, files a
    /// text diff.
    fn handle_quick_compare(
        &mut self,
        _: &QuickCompare,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((left, right)) = self.state.read(cx).selected_endpoints()
        else {
            return;
        };
        let (is_dir, has_right) = {
            let state = self.state.read(cx);
            let entry = match state.selected_entry() {
                Some(e) => e,
                None => return,
            };
            (
                entry.left.as_ref().is_some_and(|l| l.is_dir)
                    || entry.right.as_ref().is_some_and(|r| r.is_dir),
                entry.right.is_some(),
            )
        };
        if is_dir {
            self.session_manager.update(cx, |mgr, cx| {
                mgr.add_folder_tab(left, right, false, cx);
            });
        } else {
            let right = if has_right { right.clone() } else { right };
            let parent = self.state.downgrade();
            self.session_manager.update(cx, |mgr, cx| {
                mgr.add_text_diff_tab(left, right, Some(parent), cx);
            });
        }
    }

    /// Switch the comparison strategy to content hashing.
    fn handle_compare_contents_hash(
        &mut self,
        _: &CompareContentsHash,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state
            .update(cx, |state, cx| state.set_compare_files(true, cx));
    }

    /// Switch the comparison strategy to metadata only (size & date).
    fn handle_compare_contents_meta(
        &mut self,
        _: &CompareContentsMeta,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state
            .update(cx, |state, cx| state.set_compare_files(false, cx));
    }

    /// Copy an orphan entry to the side that is missing it.
    fn handle_copy_to_other_side(
        &mut self,
        _: &CopyToOtherSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let has_left = self
            .state
            .read(cx)
            .selected_entry()
            .is_some_and(|e| e.left.is_some());
        self.run_transfer_action(
            if has_left {
                TransferAction::CopyRight
            } else {
                TransferAction::CopyLeft
            },
            window,
            cx,
        );
    }

    /// Exchange the selected entry between the sides after confirmation:
    /// both sides lose their current version.
    fn handle_exchange(
        &mut self,
        _: &ExchangeEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.read(cx).can_exchange() {
            self.state.update(cx, |state, cx| {
                state.explain_blocked_transfer(cx);
            });
            return;
        }
        let name = self
            .state
            .read(cx)
            .selected_entry()
            .map(|e| e.name.clone())
            .unwrap_or_default();
        let state = self.state.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let state = state.clone();
            alert
                .confirm()
                .title("Exchange both versions?")
                .description(format!(
                    "The left and right versions of \"{name}\" swap places. \
                     Both current versions are overwritten."
                ))
                .ok_text("Exchange")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    state.update(cx, |state, cx| {
                        state.exchange_selected(cx);
                    });
                    true
                })
        });
    }

    /// Copy or move the selected entry into a folder picked through the
    /// native directory dialog.
    fn run_copy_to_folder(
        &mut self,
        move_entry: bool,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).selected_entry().is_none() {
            return;
        }
        let state = self.state.clone();
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(
                if move_entry {
                    "Move to folder"
                } else {
                    "Copy to folder"
                }
                .into(),
            ),
        });
        cx.spawn(move |_, cx: &mut gpui::AsyncApp| {
            let cx = cx.clone();
            async move {
                let paths = match receiver.await {
                    Ok(Ok(Some(paths))) => paths,
                    _ => return,
                };
                let Some(dir) = paths.into_iter().next() else {
                    return;
                };
                cx.update(|cx| {
                    state.update(cx, |state, cx| {
                        state.copy_selected_to_folder(dir, move_entry, cx);
                    });
                });
            }
        })
        .detach();
    }

    /// Handle Copy to Folder…
    fn handle_copy_to_folder(
        &mut self,
        _: &CopyToFolder,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_copy_to_folder(false, cx);
    }

    /// Handle Move to Folder…
    fn handle_move_to_folder(
        &mut self,
        _: &MoveToFolder,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_copy_to_folder(true, cx);
    }

    /// Handle Rename…: a dialog prefilled with the entry name; OK
    /// renames the entry on every side that has it.
    fn handle_rename(
        &mut self,
        _: &RenameEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) =
            self.state.read(cx).selected_entry().map(|e| e.name.clone())
        else {
            return;
        };
        let state = self.state.clone();
        let input =
            cx.new(|cx| InputState::new(window, cx).default_value(&name));
        window.open_dialog(cx, move |dialog, _, _| {
            let state = state.clone();
            let input = input.clone();
            let name = name.clone();
            dialog
                .title("Rename entry")
                .child(Input::new(&input))
                .footer(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("rename_cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| {
                                    window.close_dialog(cx);
                                }),
                        )
                        .child(
                            Button::new("rename_ok")
                                .label("Rename")
                                .primary()
                                .on_click(move |_, window, cx| {
                                    let new_name = input
                                        .read(cx)
                                        .value()
                                        .trim()
                                        .to_string();
                                    if !new_name.is_empty() && new_name != name
                                    {
                                        state.update(cx, |state, cx| {
                                            state
                                                .rename_selected(new_name, cx);
                                        });
                                    }
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    /// Handle Attributes: an informational modal over the selected
    /// entry's metadata.
    fn handle_attributes(
        &mut self,
        _: &ShowAttributes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let detail = self.state.read(cx).selected_attributes();
        let Some(detail) = detail else { return };
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title(format!("Attributes — {name}", name = detail.name))
                .child(
                    v_flex()
                        .gap_1()
                        .text_sm()
                        .child(format!("Status: {}", detail.status))
                        .child(format!("Left: {}", detail.left))
                        .child(format!("Right: {}", detail.right)),
                )
        });
    }

    /// Handle Touch: set the selected entry's modification time to now.
    fn handle_touch(
        &mut self,
        _: &TouchEntry,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| state.touch_selected(cx));
    }

    /// Handle Exclude: add the entry name to the session's name filter.
    fn handle_exclude(
        &mut self,
        _: &ExcludeEntry,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state
            .update(cx, |state, cx| state.exclude_selected(cx));
    }

    /// Handle the Ignored toggle for the selected entry.
    fn handle_toggle_ignored(
        &mut self,
        _: &ToggleIgnored,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) =
            self.state.read(cx).selected_entry().map(|e| e.name.clone())
        else {
            return;
        };
        self.state.update(cx, |state, cx| {
            state.toggle_ignored(&name, cx);
        });
    }

    /// Handle New Folder: create a folder on both sides.
    fn handle_new_folder(
        &mut self,
        _: &NewFolder,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| state.new_folder_both(cx));
    }

    /// Handle Copy Filename.
    fn handle_copy_filename(
        &mut self,
        _: &CopyFilename,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(name) =
            self.state.read(cx).selected_entry().map(|e| e.name.clone())
        {
            cx.write_to_clipboard(ClipboardItem::new_string(name));
        }
    }

    /// Handle Refresh Selection: re-read the pair from disk.
    fn handle_refresh_selection(
        &mut self,
        _: &RefreshSelection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| state.reload(cx));
    }

    /// Run a whole-comparison sync after confirmation; mirrors confirm
    /// as destructive because they delete the target side's extras.
    fn run_sync_action(
        &mut self,
        kind: crate::state::SyncKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count =
            self.state.read(cx).sync_plan(kind).map(|items| items.len());
        let Some(count) = count else {
            self.state.update(cx, |state, cx| {
                state.set_notice("Nothing to synchronize".to_string(), cx);
            });
            return;
        };
        let state = self.state.clone();
        let label = kind.label();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let state = state.clone();
            let mut alert = alert
                .confirm()
                .title(format!("{label}?"))
                .description(format!(
                    "{count} operations will run over the whole comparison. \
                     This cannot be undone."
                ))
                .ok_text(label);
            if kind.destructive() {
                alert = alert.ok_variant(ButtonVariant::Danger);
            }
            alert.on_ok(move |_, _, cx| {
                state.update(cx, |state, cx| state.run_sync(kind, cx));
                true
            })
        });
    }

    /// Handle Synchronize ▸ Update Right.
    fn handle_sync_update_right(
        &mut self,
        _: &SyncUpdateRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_sync_action(crate::state::SyncKind::UpdateRight, window, cx);
    }

    /// Handle Synchronize ▸ Update Left.
    fn handle_sync_update_left(
        &mut self,
        _: &SyncUpdateLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_sync_action(crate::state::SyncKind::UpdateLeft, window, cx);
    }

    /// Handle Synchronize ▸ Update Both.
    fn handle_sync_update_both(
        &mut self,
        _: &SyncUpdateBoth,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_sync_action(crate::state::SyncKind::UpdateBoth, window, cx);
    }

    /// Handle Synchronize ▸ Mirror to Right.
    fn handle_sync_mirror_right(
        &mut self,
        _: &SyncMirrorRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_sync_action(crate::state::SyncKind::MirrorRight, window, cx);
    }

    /// Handle Synchronize ▸ Mirror to Left.
    fn handle_sync_mirror_left(
        &mut self,
        _: &SyncMirrorLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_sync_action(crate::state::SyncKind::MirrorLeft, window, cx);
    }

    /// Handle Explorer: open the selected entry in the file explorer.
    fn handle_reveal(
        &mut self,
        _: &RevealInExplorer,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let endpoint = match self.state.read(cx).selected_endpoints() {
            Some((left, _)) => left,
            None => return,
        };
        if let Err(e) = crate::fsops::reveal_in_explorer(&endpoint) {
            self.state.update(cx, |state, cx| {
                state.set_notice(e.clone(), cx);
            });
            crate::menus::notify_error(e, cx);
        }
    }

    /// Handle a display-filter toggle dispatched from the View menu or the
    /// toolbar filter dropdown. The filter state lives in [`AppState`].
    fn handle_toggle_show_same(
        &mut self,
        _: &ToggleShowSame,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.toggle_filter(StatusCategory::Same, cx);
        });
    }

    /// Handle the show-different filter toggle.
    fn handle_toggle_show_different(
        &mut self,
        _: &ToggleShowDifferent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.toggle_filter(StatusCategory::Different, cx);
        });
    }

    /// Handle the show-orphans filter toggle.
    fn handle_toggle_show_orphans(
        &mut self,
        _: &ToggleShowOrphans,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.toggle_filter(StatusCategory::Orphan, cx);
        });
    }

    // -----------------------------------------------------------------------
    // Session callbacks (used by TabBar)
    // -----------------------------------------------------------------------

    /// Save the current session.
    #[allow(dead_code)]
    fn save_session(&mut self, cx: &mut Context<Self>) {
        let name = self.state.read(cx).title().to_string();
        let config = self.state.read(cx).to_config(name.clone());

        self.session_manager.update(cx, |mgr, cx| {
            mgr.update_active_config(config, cx);
        });
    }
}

impl Render for FolderCompareView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Key bindings are registered globally at startup; keyboard focus is
        // acquired by `WindowRoot` on tab activation. Tracking the focus
        // handle here makes this region (with its `key_context` and
        // `on_action` handlers) the focused region for keyboard input.
        let focus_handle = self.focus_handle(cx);

        div()
            .key_context("FolderCompare")
            .track_focus(&focus_handle)
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            // Header bar
            .child(self.render_header(cx))
            // Main content area
            .child(self.render_content(window, cx))
            // Status bar
            .child(self.render_status_bar(cx))
            // View-scoped action handlers (keyboard shortcuts).
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::enter_dir))
            .on_action(cx.listener(Self::leave_dir))
            .on_action(cx.listener(Self::toggle_expand))
            .on_action(cx.listener(Self::reload))
            .on_action(cx.listener(Self::handle_next_difference))
            .on_action(cx.listener(Self::handle_prev_difference))
            .on_action(cx.listener(Self::handle_swap_sides))
            .on_action(cx.listener(Self::handle_copy_to_left))
            .on_action(cx.listener(Self::handle_copy_to_right))
            .on_action(cx.listener(Self::handle_move_to_left))
            .on_action(cx.listener(Self::handle_move_to_right))
            .on_action(cx.listener(Self::handle_delete_left))
            .on_action(cx.listener(Self::handle_delete_right))
            .on_action(cx.listener(Self::handle_set_base_left))
            .on_action(cx.listener(Self::handle_set_base_right))
            .on_action(cx.listener(Self::handle_set_base_both))
            .on_action(cx.listener(Self::handle_go_back))
            .on_action(cx.listener(Self::handle_go_forward))
            // Row commands (context menu).
            .on_action(cx.listener(Self::handle_set_side_path))
            .on_action(cx.listener(Self::handle_quick_compare))
            .on_action(cx.listener(Self::handle_compare_contents_hash))
            .on_action(cx.listener(Self::handle_compare_contents_meta))
            .on_action(cx.listener(Self::handle_copy_to_other_side))
            .on_action(cx.listener(Self::handle_exchange))
            .on_action(cx.listener(Self::handle_copy_to_folder))
            .on_action(cx.listener(Self::handle_move_to_folder))
            .on_action(cx.listener(Self::handle_rename))
            .on_action(cx.listener(Self::handle_attributes))
            .on_action(cx.listener(Self::handle_touch))
            .on_action(cx.listener(Self::handle_exclude))
            .on_action(cx.listener(Self::handle_toggle_ignored))
            .on_action(cx.listener(Self::handle_new_folder))
            .on_action(cx.listener(Self::handle_copy_filename))
            .on_action(cx.listener(Self::handle_refresh_selection))
            .on_action(cx.listener(Self::handle_sync_update_right))
            .on_action(cx.listener(Self::handle_sync_update_left))
            .on_action(cx.listener(Self::handle_sync_update_both))
            .on_action(cx.listener(Self::handle_sync_mirror_right))
            .on_action(cx.listener(Self::handle_sync_mirror_left))
            .on_action(cx.listener(Self::handle_reveal))
            // Display filters (View menu and toolbar filter dropdown).
            .on_action(cx.listener(Self::handle_toggle_show_same))
            .on_action(cx.listener(Self::handle_toggle_show_different))
            .on_action(cx.listener(Self::handle_toggle_show_orphans))
            // Menu-dispatched actions that need view context.
            .on_action(cx.listener(Self::handle_reload_compare))
            .on_action(cx.listener(Self::handle_stop_compare))
            .on_action(cx.listener(Self::handle_save_session))
            // File/dir diff actions.
            .on_action(cx.listener(Self::handle_open_file_diff))
            .on_action(cx.listener(Self::handle_open_dir_diff))
            // Clipboard actions (Edit menu and row context menu).
            .on_action(cx.listener(Self::handle_copy_path_left))
            .on_action(cx.listener(Self::handle_copy_path_right))
    }
}

impl FolderCompareView {
    /// Render the header bar: the up-one-level control, the status
    /// legend trigger, and the navigation breadcrumb.
    ///
    /// The compared paths themselves live in the per-pane path bars, so
    /// this header only carries the pair-level controls.
    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (at_root, crumbs, root_label) = {
            let state = self.state.read(cx);
            let crumbs: Vec<(SharedString, usize)> = state
                .nav_steps()
                .iter()
                .enumerate()
                .map(|(i, step)| (step.label.clone(), i + 1))
                .collect();
            (state.nav_steps().is_empty(), crumbs, state.title().clone())
        };
        let theme = cx.theme();
        let state_entity = self.state.clone();

        // Up and the legend act on the pair as a whole; the window title
        // already names the product, so nothing competes with them.
        let path_row = h_flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .bg(theme.muted)
            .border_b_1()
            .border_color(theme.border)
            .child(
                Button::new("up_btn")
                    .icon(IconName::ArrowUpToLine)
                    .ghost()
                    .small()
                    .disabled(at_root)
                    .tooltip("Up one level")
                    .accessibility_label("Up one level")
                    // Update the state directly rather than dispatching
                    // `LeaveDir`: a mouse click on the header does not move
                    // keyboard focus to the list, and the action is owned by
                    // the (possibly unfocused) view.
                    .on_click({
                        let state = state_entity.clone();
                        move |_, _, cx| {
                            state.update(cx, |state, cx| {
                                state.navigate_up(cx);
                            });
                        }
                    }),
            )
            .child(div().flex_1())
            // The legend explains every status marker; it belongs to this
            // view (the toolbar is shared with the text-diff tabs), so the
            // trigger sits at the trailing edge of the header.
            .child(
                Popover::new("status-legend")
                    .anchor(Anchor::TopRight)
                    .arrow(true)
                    .trigger(
                        Button::new("legend_btn")
                            .icon(IconName::CircleQuestionMark)
                            .ghost()
                            .small()
                            .tooltip("Status legend")
                            .accessibility_label("Status legend"),
                    )
                    .child(status_legend(theme)),
            );

        // The breadcrumb appears only once the pair has descended into a
        // subfolder: the root crumb re-roots the view, later crumbs step
        // back partway. The current (last) crumb is not clickable.
        let depth = crumbs.len();
        let breadcrumb = (!crumbs.is_empty()).then(|| {
            let mut trail = Breadcrumb::new().child({
                let root_state = state_entity.clone();
                BreadcrumbItem::new(root_label.clone()).on_click(
                    move |_, _, cx| {
                        root_state.update(cx, |state, cx| {
                            state.navigate_to_depth(0, cx);
                        });
                    },
                )
            });
            for (i, (label, target)) in crumbs.into_iter().enumerate() {
                let crumb_state = state_entity.clone();
                let is_current = i + 1 == depth;
                trail = trail.child(
                    BreadcrumbItem::new(label).disabled(is_current).on_click(
                        move |_, _, cx| {
                            crumb_state.update(cx, |state, cx| {
                                state.navigate_to_depth(target, cx);
                            });
                        },
                    ),
                );
            }
            v_flex().px_3().py_1().child(trail)
        });

        v_flex().child(path_row).children(breadcrumb)
    }

    /// Render the main content area: the two mirrored tree panes.
    fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (left_path, right_path) = self.ensure_path_inputs(window, cx);
        // Keep the path fields showing the compared endpoints unless the
        // user is mid-edit in one of them.
        self.sync_path_input(&left_path, Side::Left, window, cx);
        self.sync_path_input(&right_path, Side::Right, window, cx);

        let focus_handle = self.focus_handle(cx);
        let theme = cx.theme().clone();
        let list_focused = focus_handle.is_focused(window);

        div()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .p_1()
            .rounded_md()
            .border_1()
            .border_color(if list_focused {
                theme.ring
            } else {
                theme.border
            })
            .bg(theme.background)
            .child(crate::folder_panes::render_folder_panes(
                &self.state,
                &self.scroll_handle,
                &focus_handle,
                &left_path,
                &right_path,
                cx,
            ))
    }

    /// Create the two path-field states on first use and subscribe their
    /// Enter key to "compare now" for the respective side.
    fn ensure_path_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Entity<InputState>) {
        if self.left_path_input.is_none() {
            let left = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Left path")
            });
            let right = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Right path")
            });
            for (side, input) in [(Side::Left, &left), (Side::Right, &right)] {
                let state = self.state.clone();
                let sub = cx.subscribe(input, {
                    let input = input.clone();
                    move |_, _, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            let value = input.read(cx).value().to_string();
                            state.update(cx, |state, cx| {
                                state.set_side_path(side, &value, cx);
                            });
                        }
                    }
                });
                self._path_subscriptions.push(sub);
            }
            self.left_path_input = Some(left);
            self.right_path_input = Some(right);
        }
        (
            self.left_path_input.clone().expect("created above"),
            self.right_path_input.clone().expect("created above"),
        )
    }

    /// Refresh one path field from the state's endpoint when it is not
    /// focused, so navigation updates the bar without fighting an edit.
    fn sync_path_input(
        &self,
        input: &Entity<InputState>,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = match side {
            Side::Left => self.state.read(cx).left().display(),
            Side::Right => self.state.read(cx).right().display(),
        };
        if input.read(cx).value() == current {
            return;
        }
        if input.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        input.update(cx, |input, cx| {
            input.set_value(current, window, cx);
        });
    }

    /// Render the status bar at the bottom.
    ///
    /// Left region: summary counts over the visible rows, so they stay
    /// honest while a display filter is active. Right region: what is
    /// happening now — a spinner with the in-flight label, the error in
    /// danger color, the last transfer outcome, or the selection position
    /// within the visible list.
    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let theme = cx.theme();

        let mut bar = StatusBar::new();

        if state.comparison().is_some() {
            let summary = state.visible_summary();
            bar = bar.left(
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(status_count("Dirs", summary.dirs, theme))
                    .child(status_count("Files", summary.files, theme))
                    .child(status_count("Same", summary.same, theme))
                    .child(status_count("Diff", summary.different, theme))
                    .child(status_count("Orphans", summary.orphans, theme)),
            );
        }

        let visible = state.entry_count();
        let status = if state.is_loading() {
            let label = state.busy_label().unwrap_or("Loading comparison…");
            h_flex()
                .items_center()
                .gap_1p5()
                .child(Spinner::new().xsmall())
                .child(label.to_string())
                .into_any_element()
        } else if let Some(err) = state.error() {
            div()
                .text_color(theme.danger)
                .child(err.to_string())
                .into_any_element()
        } else if let Some(notice) = state.notice() {
            div()
                .text_color(theme.muted_foreground)
                .child(notice.to_string())
                .into_any_element()
        } else if state.comparison().is_some() {
            if visible == 0 {
                div().child("No entries").into_any_element()
            } else {
                div()
                    .child(format!(
                        "{} of {}",
                        state.selected_index() + 1,
                        visible
                    ))
                    .into_any_element()
            }
        } else {
            div().child("Ready").into_any_element()
        };

        bar.right(status)
    }
}

/// A neutral count in the status bar: muted label, emphasized number.
fn status_count(
    label: &'static str,
    count: usize,
    theme: &Theme,
) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_1()
        .child(div().child(label))
        .child(div().text_color(theme.foreground).child(count.to_string()))
}

// ---------------------------------------------------------------------------
// Status legend
// ---------------------------------------------------------------------------

/// Status lane: colored dot plus the color-independent glyph, used by
/// the legend popover.
fn status_lane() -> Div {
    div().w_16().flex_shrink_0().flex().items_center().gap_1()
}

/// Semantic color role for a directory entry status.
///
/// Statuses map onto a small set of semantic roles instead of a fixed RGB
/// palette, so entry colors follow the active theme (light, dark, and
/// custom). The glyph from [`status_indicator`] remains as a secondary,
/// color-independent cue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusRole {
    /// Entry matches on both sides.
    Success,
    /// Entry differs in a way that may still be benign.
    Warning,
    /// Entry differs or cannot be reconciled.
    Danger,
    /// Entry exists on only one side, or needs a three-way decision.
    Info,
}

impl StatusRole {
    /// Resolve this role to a color from the active theme.
    fn color(self, theme: &Theme) -> Hsla {
        match self {
            Self::Success => theme.success,
            Self::Warning => theme.warning,
            Self::Danger => theme.danger,
            Self::Info => theme.info,
        }
    }
}

/// Map an entry status to its semantic color role.
fn status_role(status: &DirEntryStatus) -> StatusRole {
    match status {
        DirEntryStatus::Same | DirEntryStatus::SameBinary => {
            StatusRole::Success
        }
        DirEntryStatus::Similar => StatusRole::Warning,
        DirEntryStatus::Different | DirEntryStatus::Conflict => {
            StatusRole::Danger
        }
        DirEntryStatus::LeftOnly
        | DirEntryStatus::RightOnly
        | DirEntryStatus::CenterOnly
        | DirEntryStatus::Mergeable => StatusRole::Info,
    }
}

/// Get the status indicator character for an entry.
fn status_indicator(status: &DirEntryStatus) -> &'static str {
    match status {
        DirEntryStatus::Same | DirEntryStatus::SameBinary => "=",
        DirEntryStatus::Similar => "~",
        DirEntryStatus::Different => "!",
        DirEntryStatus::LeftOnly => "<",
        DirEntryStatus::RightOnly => ">",
        DirEntryStatus::CenterOnly => "^",
        DirEntryStatus::Mergeable => "M",
        DirEntryStatus::Conflict => "C",
    }
}

/// Compact display for an RFC 3339 modification timestamp.
///
/// Renders `2026-10-27T21:50:51+01:00` as `2026-10-27 21:50`; seconds and
/// offset carry no comparison signal and the full string would dominate
/// the date lanes. Anything shorter than a timestamp is passed through.
pub(crate) fn format_modified(modified: &str) -> String {
    if modified.len() >= 16
        && modified.is_char_boundary(10)
        && modified.is_char_boundary(16)
    {
        format!("{} {}", &modified[..10], &modified[11..16])
    } else {
        modified.to_string()
    }
}

/// Legend rows for the status markers, in status sort order.
///
/// Every [`DirEntryStatus`] value the table can show appears here, so the
/// dot and glyph are never a color-only or cryptic-symbol-only cue.
const STATUS_LEGEND: [(DirEntryStatus, &str, &str); 9] = [
    (
        DirEntryStatus::Same,
        "Same",
        "Text files with equal content",
    ),
    (
        DirEntryStatus::SameBinary,
        "Same (binary)",
        "Binary files with equal content",
    ),
    (
        DirEntryStatus::Similar,
        "Similar",
        "Same name, size within tolerance, content differs",
    ),
    (
        DirEntryStatus::Different,
        "Different",
        "Same name, different content",
    ),
    (
        DirEntryStatus::LeftOnly,
        "Left only",
        "Exists only on the left side",
    ),
    (
        DirEntryStatus::RightOnly,
        "Right only",
        "Exists only on the right side",
    ),
    (
        DirEntryStatus::CenterOnly,
        "Center only",
        "Exists only on the center side (3-way)",
    ),
    (
        DirEntryStatus::Mergeable,
        "Mergeable",
        "Non-conflicting changes on both sides",
    ),
    (
        DirEntryStatus::Conflict,
        "Conflict",
        "Conflicting changes in a 3-way merge",
    ),
];

/// Popover content documenting every status marker the table renders.
fn status_legend(theme: &Theme) -> impl IntoElement {
    v_flex()
        .gap_2()
        .text_sm()
        .child(div().font_weight(FontWeight(600.)).child("Status legend"))
        .child(v_flex().gap_1().children(STATUS_LEGEND.iter().map(
            |(status, label, description)| {
                let color = status_role(status).color(theme);
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        status_lane()
                            .child(
                                div()
                                    .size_2()
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .bg(color),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight(700.))
                                    .text_color(color)
                                    .child(status_indicator(status)),
                            ),
                    )
                    .child(
                        v_flex().child(div().child(*label)).child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(*description),
                        ),
                    )
            },
        )))
}

// ---------------------------------------------------------------------------
// Session rules dialog
// ---------------------------------------------------------------------------

/// Body of the folder session's rules dialog. A `Render` entity so the
/// checkboxes repaint as they are toggled.
struct FolderRulesBody {
    state: Entity<AppState>,
    compare_files: bool,
    compare_structure: bool,
    ignore_whitespace: bool,
    skip_blank_lines: bool,
    skip_comments: bool,
    name_filter: Entity<InputState>,
}

impl FolderRulesBody {
    fn new(
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = state.read(cx).settings().clone();
        Self {
            compare_files: settings.compare_files,
            compare_structure: settings.compare_structure,
            ignore_whitespace: settings.ignore_whitespace,
            skip_blank_lines: settings.skip_blank_lines,
            skip_comments: settings.skip_comments,
            name_filter: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Name filters (comma-separated globs)")
                    .default_value(settings.name_filter.join(", "))
            }),
            state,
        }
    }

    /// Push the drafted rules into the session and re-compare.
    fn apply(&mut self, cx: &mut Context<Self>) {
        let mut settings = self.state.read(cx).settings().clone();
        settings.compare_files = self.compare_files;
        settings.compare_structure = self.compare_structure;
        settings.ignore_whitespace = self.ignore_whitespace;
        settings.skip_blank_lines = self.skip_blank_lines;
        settings.skip_comments = self.skip_comments;
        settings.name_filter = self
            .name_filter
            .read(cx)
            .value()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self.state
            .update(cx, |state, cx| state.update_settings(settings, cx));
    }
}

impl Render for FolderRulesBody {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let row = |label: &'static str,
                   checked: bool,
                   handler: fn(&mut Self, bool)| {
            Checkbox::new(label)
                .label(label)
                .checked(checked)
                .on_change(cx.listener(move |this, checked, _, cx| {
                    handler(this, *checked);
                    cx.notify();
                }))
        };
        v_flex()
            .w_full()
            .gap_2()
            .child(row(
                "Compare file contents",
                self.compare_files,
                |this, v| this.compare_files = v,
            ))
            .child(row(
                "Compare directory structure",
                self.compare_structure,
                |this, v| this.compare_structure = v,
            ))
            .child(row(
                "Ignore whitespace",
                self.ignore_whitespace,
                |this, v| this.ignore_whitespace = v,
            ))
            .child(row(
                "Skip blank lines",
                self.skip_blank_lines,
                |this, v| this.skip_blank_lines = v,
            ))
            .child(row("Skip comment lines", self.skip_comments, |this, v| {
                this.skip_comments = v
            }))
            .child(Input::new(&self.name_filter).small())
            .child(
                h_flex().justify_end().child(
                    Button::new("folder_rules_apply")
                        .label("Apply")
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.apply(cx);
                        })),
                ),
            )
    }
}

/// Open the rules dialog for a folder compare session.
pub(crate) fn open_folder_rules_dialog(
    state: Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let body = cx.new(|cx| FolderRulesBody::new(state, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Session rules")
            .w(gpui::px(460.))
            .child(body.clone())
    });
}

#[cfg(test)]
mod tests {
    use cocomo_lib::{DirEntry, EntryInfo};
    use gpui::{ElementId, TestAppContext};

    use super::*;
    use crate::state::SortKey;

    fn entry_info(path: &str, is_dir: bool) -> EntryInfo {
        EntryInfo {
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            path: path.to_string(),
            size: 1,
            modified: String::new(),
            is_dir,
            hash: None,
        }
    }

    fn entry_info_sized(path: &str, size: u64) -> EntryInfo {
        EntryInfo {
            size,
            ..entry_info(path, false)
        }
    }

    fn entry(
        name: &str,
        left: Option<EntryInfo>,
        right: Option<EntryInfo>,
    ) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            status: DirEntryStatus::Different,
            left,
            right,
            center: None,
            sub_entries: None,
            name_conflict: false,
        }
    }

    #[test]
    fn status_role_maps_every_status() {
        assert_eq!(status_role(&DirEntryStatus::Same), StatusRole::Success);
        assert_eq!(
            status_role(&DirEntryStatus::SameBinary),
            StatusRole::Success
        );
        assert_eq!(status_role(&DirEntryStatus::Similar), StatusRole::Warning);
        assert_eq!(
            status_role(&DirEntryStatus::Different),
            StatusRole::Danger
        );
        assert_eq!(status_role(&DirEntryStatus::Conflict), StatusRole::Danger);
        assert_eq!(status_role(&DirEntryStatus::LeftOnly), StatusRole::Info);
        assert_eq!(status_role(&DirEntryStatus::RightOnly), StatusRole::Info);
        assert_eq!(status_role(&DirEntryStatus::CenterOnly), StatusRole::Info);
        assert_eq!(status_role(&DirEntryStatus::Mergeable), StatusRole::Info);
    }

    #[test]
    fn format_modified_keeps_date_and_minutes() {
        assert_eq!(
            format_modified("2026-10-27T21:50:51+01:00"),
            "2026-10-27 21:50"
        );
        // Values that are not full timestamps pass through unchanged.
        assert_eq!(format_modified(""), "");
        assert_eq!(format_modified("unknown"), "unknown");
    }

    #[test]
    fn status_legend_documents_every_status() {
        let all = [
            DirEntryStatus::Same,
            DirEntryStatus::SameBinary,
            DirEntryStatus::Similar,
            DirEntryStatus::Different,
            DirEntryStatus::LeftOnly,
            DirEntryStatus::RightOnly,
            DirEntryStatus::CenterOnly,
            DirEntryStatus::Mergeable,
            DirEntryStatus::Conflict,
        ];
        for status in all {
            assert!(
                STATUS_LEGEND.iter().any(|(s, ..)| *s == status),
                "legend is missing a row for {status:?}"
            );
        }
        assert_eq!(
            STATUS_LEGEND.len(),
            all.len(),
            "legend has a duplicate or stale row"
        );
    }

    /// UI integration test: the header cell is a real control. Renders the
    /// production view in a headless window, clicks the left pane's
    /// "Size" header, and verifies the table reorders through `AppState`.
    #[gpui::test]
    fn clicking_a_column_header_sorts_the_table(cx: &mut TestAppContext) {
        use cocomo_lib::DirComparison;
        use gpui::{Bounds, WindowBounds, WindowOptions, test::TestWindowExt};

        cx.update(gpui::init);

        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(DirComparison {
                entries: vec![
                    entry(
                        "bravo.txt",
                        Some(entry_info_sized("bravo.txt", 100)),
                        Some(entry_info_sized("bravo.txt", 100)),
                    ),
                    entry(
                        "alpha.txt",
                        Some(entry_info_sized("alpha.txt", 5000)),
                        Some(entry_info_sized("alpha.txt", 5000)),
                    ),
                ],
                counts: Default::default(),
                errors: Vec::new(),
            });
            state
        });
        let manager = cx.new(|_| {
            GuiSessionManager::new(std::path::PathBuf::from(
                "/tmp/cocomo-ui-tests",
            ))
        });

        let (window, _view) = cx.update(|cx| {
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(
                        Bounds::maximized(None, cx),
                    )),
                    ..Default::default()
                },
                cx,
                |_, cx| {
                    cx.new(|cx| {
                        FolderCompareView::new(
                            state.clone(),
                            manager.clone(),
                            cx,
                        )
                    })
                },
            )
            .expect("failed to open the test window")
        });

        // Default order is by name; clicking the size header reorders by
        // size, putting the smaller entry first.
        let size_header =
            (ElementId::from("sort_header"), "Left:Size".to_string());
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(state.read(cx).entry_at(0).unwrap().name, "alpha.txt");
            window.click(size_header.clone(), cx);
        })
        .expect("failed to click the column header");

        cx.update(|cx| {
            let state = state.read(cx);
            assert_eq!(state.sort_key(), SortKey::SizeLeft);
            assert!(!state.is_sort_descending());
            assert_eq!(state.entry_at(0).unwrap().name, "bravo.txt");
        });

        // A second click on the active column reverses the direction.
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.click(size_header, cx);
        })
        .expect("failed to click the column header again");

        cx.update(|cx| {
            let state = state.read(cx);
            assert_eq!(state.sort_key(), SortKey::SizeLeft);
            assert!(state.is_sort_descending());
            assert_eq!(state.entry_at(0).unwrap().name, "alpha.txt");
        });
    }

    /// UI integration test: the difference-walking and filter actions are
    /// wired to the view. Renders the production view, focuses it, then
    /// dispatches `NextDifference` and `ToggleShowSame` exactly as a
    /// shortcut or menu item would, and checks the state responds.
    #[gpui::test]
    fn difference_walking_and_filtering_dispatch_through_the_view(
        cx: &mut TestAppContext,
    ) {
        use cocomo_lib::{DirComparison, DirEntryStatus};
        use gpui::{Bounds, WindowBounds, WindowOptions, test::TestWindowExt};

        cx.update(gpui::init);

        let same = |name: &str| DirEntry {
            name: name.to_string(),
            status: DirEntryStatus::Same,
            left: Some(entry_info(name, false)),
            right: Some(entry_info(name, false)),
            center: None,
            sub_entries: None,
            name_conflict: false,
        };

        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(DirComparison {
                entries: vec![
                    same("a.txt"),
                    entry(
                        "b.txt",
                        Some(entry_info("b.txt", false)),
                        Some(entry_info("b.txt", false)),
                    ),
                    same("c.txt"),
                ],
                counts: Default::default(),
                errors: Vec::new(),
            });
            state
        });
        let manager = cx.new(|_| {
            GuiSessionManager::new(std::path::PathBuf::from(
                "/tmp/cocomo-ui-tests",
            ))
        });

        let (window, view) = cx.update(|cx| {
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(
                        Bounds::maximized(None, cx),
                    )),
                    ..Default::default()
                },
                cx,
                |_, cx| {
                    cx.new(|cx| {
                        FolderCompareView::new(
                            state.clone(),
                            manager.clone(),
                            cx,
                        )
                    })
                },
            )
            .expect("failed to open the test window")
        });

        // Focus the view so a dispatched action reaches its handlers, then
        // walk to the only difference (the identical rows are skipped).
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let focus = view.read(cx).focus_handle(cx);
            focus.focus(window, cx);
            window.dispatch_action(Box::new(NextDifference), cx);
        })
        .expect("failed to dispatch the next-difference action");

        cx.update(|cx| {
            assert_eq!(state.read(cx).selected_entry().unwrap().name, "b.txt");
        });

        // Toggling the Same filter through the action hides the identical
        // rows, leaving only the difference visible.
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.dispatch_action(Box::new(ToggleShowSame), cx);
        })
        .expect("failed to dispatch the show-same filter action");

        cx.update(|cx| {
            let state = state.read(cx);
            assert!(!state.filters().show_same);
            assert_eq!(state.entry_count(), 1);
            assert_eq!(state.entry_at(0).unwrap().name, "b.txt");
        });
    }
}
