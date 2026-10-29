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

use std::{ops::Range, sync::Arc};

use cocomo_lib::{DirEntry, DirEntryStatus, Endpoint, TransferAction};
use gpui::{
    Action, Anchor, AnyElement, App, ClickEvent, ClipboardItem, Context, Div,
    ElementId, Entity, FocusHandle, FontFeatures, FontWeight, Hsla,
    KeyBinding, MouseButton, Render, Role, ScrollStrategy, SharedString,
    Subscription, TestSupportExt as _, UniformListScrollHandle, Window,
    actions,
    assets::IconName,
    base::Disableable as _,
    component::{
        ActiveTheme, Sizable as _, Theme, WindowExt as _,
        breadcrumb::{Breadcrumb, BreadcrumbItem},
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem},
        popover::Popover,
        spinner::Spinner,
        status_bar::StatusBar,
        toolbar::{Toolbar, ToolbarGroup},
        tooltip::Tooltip,
        v_flex,
    },
    div,
    prelude::*,
    uniform_list,
};

use crate::{
    menus::{
        CopyPathLeft, CopyPathRight, ReloadCompare, SaveSession,
        ToggleShowDifferent, ToggleShowOrphans, ToggleShowSame,
    },
    session_manager::GuiSessionManager,
    state::{AppState, SortKey, StatusCategory, entry_endpoints},
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
        Reload,
        NextDifference,
        PrevDifference,
        SwapSides,
        CopyToLeft,
        CopyToRight,
    ]
);

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
        KeyBinding::new("r", Reload, None),
        // Walk the differences: the single highest-value navigation,
        // matching the familiar next/previous-difference commands.
        KeyBinding::new("cmd-]", NextDifference, None),
        KeyBinding::new("f7", NextDifference, None),
        KeyBinding::new("cmd-[", PrevDifference, None),
        KeyBinding::new("shift-f7", PrevDifference, None),
        // Reconcile a selected orphan toward each side.
        KeyBinding::new("cmd-1", CopyToLeft, None),
        KeyBinding::new("cmd-2", CopyToRight, None),
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
    /// Scroll handle for the entry list.
    scroll_handle: UniformListScrollHandle,
    /// Editable name-filter combo in toolbar row 2.
    ///
    /// Created lazily on the first row-2 render: the row is rendered by
    /// `WindowRoot`, which is where a window handle is available for
    /// `InputState`.
    filter_input: Option<Entity<InputState>>,
    /// Subscription that commits the filter input on Enter.
    _filter_subscription: Option<Subscription>,
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

        // Load cancellation arrives with the async-robustness phase.
        let stop_btn = Button::new("row2_stop")
            .icon(IconName::CircleStop)
            .ghost()
            .small()
            .disabled(true)
            .tooltip("Stop comparison (coming soon)")
            .accessibility_label("Stop comparison");

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

    /// Handle the enter-directory action: open a new dir-diff tab.
    fn enter_dir(
        &mut self,
        _: &EnterDir,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = match self.state.read(cx).selected_entry() {
            Some(e) => e,
            None => return,
        };

        // Determine sub-endpoints for both sides. An entry's `path` is
        // relative to the current comparison root, so it must be joined onto
        // the endpoint path rather than used as the endpoint path directly
        // (which would drop the root and resolve to a non-existent location).
        let left_sub = match &entry.left {
            Some(left_info) => {
                self.state.read(cx).left().join(&left_info.path)
            }
            None => self.state.read(cx).left().join(&entry.name),
        };
        let right_sub = match &entry.right {
            Some(right_info) => {
                self.state.read(cx).right().join(&right_info.path)
            }
            None => self.state.read(cx).right().join(&entry.name),
        };

        // Dispatch action to open a new dir-diff tab.
        cx.dispatch_action(&crate::ui::OpenDirDiff {
            left_path: left_sub,
            right_path: right_sub,
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

    /// Handle OpenFileDiff: dispatch to session manager to create a text diff
    /// tab.
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
        self.session_manager.update(cx, |mgr, cx| {
            mgr.add_text_diff_tab(action.left_path.clone(), right_path, cx);
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

    /// Handle copy-to-left: reconcile the selected orphan by copying it
    /// from the right side to the left.
    fn handle_copy_to_left(
        &mut self,
        _: &CopyToLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.transfer_selected(TransferAction::CopyLeft, cx);
        });
    }

    /// Handle copy-to-right: reconcile the selected orphan by copying it
    /// from the left side to the right.
    fn handle_copy_to_right(
        &mut self,
        _: &CopyToRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.transfer_selected(TransferAction::CopyRight, cx);
        });
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
            .on_action(cx.listener(Self::reload))
            .on_action(cx.listener(Self::handle_next_difference))
            .on_action(cx.listener(Self::handle_prev_difference))
            .on_action(cx.listener(Self::handle_swap_sides))
            .on_action(cx.listener(Self::handle_copy_to_left))
            .on_action(cx.listener(Self::handle_copy_to_right))
            // Display filters (View menu and toolbar filter dropdown).
            .on_action(cx.listener(Self::handle_toggle_show_same))
            .on_action(cx.listener(Self::handle_toggle_show_different))
            .on_action(cx.listener(Self::handle_toggle_show_orphans))
            // Menu-dispatched actions that need view context.
            .on_action(cx.listener(Self::handle_reload_compare))
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
    /// Render the header bar showing the compared paths, the up-one-level
    /// control, the status legend trigger, and the navigation breadcrumb.
    ///
    /// Pair controls (swap) and display filters live in the toolbar rows;
    /// this header only names what is being compared.
    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (left_path, right_path, at_root, crumbs, root_label) = {
            let state = self.state.read(cx);
            let crumbs: Vec<(SharedString, usize)> = state
                .nav_steps()
                .iter()
                .enumerate()
                .map(|(i, step)| (step.label.clone(), i + 1))
                .collect();
            (
                state.left().display().to_string(),
                state.right().display().to_string(),
                state.nav_steps().is_empty(),
                crumbs,
                state.title().clone(),
            )
        };
        let theme = cx.theme();
        let state_entity = self.state.clone();

        // The two compared roots are the primary information of this view,
        // so they take the full width; the window title already names the
        // product, so no branding competes with the paths. Up and Swap act
        // on both sides at once, mirroring a compare tool's pair controls.
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
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.foreground)
                    .child(left_path),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.foreground)
                    .child(right_path),
            )
            // The legend explains every status marker; it belongs to this
            // view (the toolbar is shared with the text-diff tabs), so the
            // trigger sits at the trailing edge of the path header.
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

    /// Render the main content area with the entry list.
    fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (entry_count, is_loading, sort_key, sort_descending) = {
            let state = self.state.read(cx);
            (
                state.entry_count(),
                state.is_loading(),
                state.sort_key(),
                state.is_sort_descending(),
            )
        };
        let theme = cx.theme();
        // The list is the keyboard target of this view's focus handle; a
        // tinted border tells the user where j/k/Enter will act.
        let list_focused = self.focus_handle(cx).is_focused(window);

        // Clone data needed inside the closures.
        let state_entity = self.state.clone();
        let state_weak = self.state.downgrade();
        let scroll_handle = self.scroll_handle.clone();
        let focus_handle = self.focus_handle(cx);

        div()
            .flex()
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
            // Column headers
            .child(render_column_headers(
                SortState {
                    key: sort_key,
                    descending: sort_descending,
                },
                &state_entity,
                theme,
            ))
            // Entry list
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .when(is_loading, |this| {
                        this.child(
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap_2()
                                .text_color(theme.muted_foreground)
                                .child(Spinner::new().small())
                                .child("Loading comparison…"),
                        )
                    })
                    .when(!is_loading, |this| {
                        this.child(
                            uniform_list(
                                "entries",
                                entry_count,
                                move |range: Range<usize>,
                                      _: &mut Window,
                                      app: &mut App| {
                                    let mut items =
                                        Vec::with_capacity(range.len());
                                    let theme = app.theme();
                                    if let Some(state_entity) =
                                        state_weak.upgrade()
                                    {
                                        let app_state =
                                            state_entity.read(app);
                                        let selected =
                                            app_state.selected_index();
                                        let left_root =
                                            app_state.left().clone();
                                        let right_root =
                                            app_state.right().clone();
                                        for i in range {
                                            // Rows come through the display
                                            // order, so the sort never
                                            // reorders the comparison data.
                                            if let Some(entry) =
                                                app_state.entry_at(i)
                                            {
                                                items.push(
                                                    EntryRowData::new(
                                                        i,
                                                        entry,
                                                        i == selected,
                                                        &left_root,
                                                        &right_root,
                                                    )
                                                    .render_row(
                                                        theme,
                                                        state_entity
                                                            .clone(),
                                                        focus_handle
                                                            .clone(),
                                                    )
                                                    .into_any_element(),
                                                );
                                            }
                                        }
                                    }
                                    items
                                },
                            )
                            .size_full()
                            .track_scroll(&scroll_handle),
                        )
                    }),
            )
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
                    .child(status_count("Entries", summary.total, theme))
                    .child(status_count("Same", summary.same, theme))
                    .child(status_count("Different", summary.different, theme))
                    .child(status_count("Orphan", summary.orphans, theme)),
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
// Entry table columns
// ---------------------------------------------------------------------------
//
// The lane helpers are shared by `render_column_headers` and
// `EntryRowData::render_row` so headers and values stay aligned; the fixed
// widths are functional data lanes, not decorative sizes.

/// Status lane: colored dot plus the color-independent glyph.
///
/// The width fits the "Status" header plus its sort arrow, so the header
/// label and the row markers share one lane without overlapping the name.
fn status_lane() -> Div {
    div().w_16().flex_shrink_0().flex().items_center().gap_1()
}

/// Name lane: type icon plus the entry name, truncating as needed.
fn name_lane() -> Div {
    div().flex_1().min_w_0().flex().items_center().gap_1p5()
}

/// Mirrored size lane: right-aligned so magnitudes compare at a glance.
fn size_lane() -> Div {
    div()
        .w_20()
        .flex_shrink_0()
        .text_right()
        .text_xs()
        .font_features(tabular_features())
}

/// Mirrored modification-date lane, right-aligned like the sizes.
///
/// `w_32` is the widest lane and holds the full `YYYY-MM-DD HH:MM` stamp;
/// the name lane absorbs the remaining width instead of truncating dates.
fn date_lane() -> Div {
    div()
        .w_32()
        .flex_shrink_0()
        .text_right()
        .text_xs()
        .font_features(tabular_features())
}

/// Tabular figures for the numeric lanes, so columns of digits align.
fn tabular_features() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

/// Which column the table is sorted by, and in which direction.
#[derive(Clone, Copy)]
struct SortState {
    /// Active sort column.
    key: SortKey,
    /// Whether the active column sorts descending.
    descending: bool,
}

/// A column of the entry table: header label, sort key, lane builder, and
/// whether its values align to the trailing edge.
type Column = (&'static str, SortKey, fn() -> Div, bool);

/// The columns in display order: the decision (status), the identity (name),
/// then the mirrored left/right data lanes. Header and rows share the lane
/// builders, so the two always line up.
const COLUMNS: [Column; 6] = [
    ("Status", SortKey::Status, status_lane, false),
    ("Name", SortKey::Name, name_lane, false),
    ("Left size", SortKey::SizeLeft, size_lane, true),
    ("Left date", SortKey::DateLeft, date_lane, true),
    ("Right size", SortKey::SizeRight, size_lane, true),
    ("Right date", SortKey::DateRight, date_lane, true),
];

/// Render the sortable column header row.
fn render_column_headers(
    sort: SortState,
    state: &Entity<AppState>,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .bg(theme.muted)
        .border_b_1()
        .border_color(theme.border)
        .text_xs()
        .font_weight(FontWeight(600.))
        .children(COLUMNS.iter().map(|(label, key, lane, align_end)| {
            sort_header_cell(
                *lane, label, *key, sort, state, theme, *align_end,
            )
        }))
}

/// One clickable column header cell with a visible sort indicator.
///
/// The active column shows its direction as an arrow and the stronger
/// foreground; inactive columns stay muted until hovered, keeping the
/// header quiet while making sorting discoverable and reversible.
fn sort_header_cell(
    lane: fn() -> Div,
    label: &'static str,
    key: SortKey,
    sort: SortState,
    state: &Entity<AppState>,
    theme: &Theme,
    align_end: bool,
) -> impl IntoElement {
    let is_active = sort.key == key;
    let descending = sort.descending;
    let state = state.clone();
    lane()
        .id((ElementId::from("sort_header"), label))
        .when(align_end, |this| this.justify_end())
        .text_color(if is_active {
            theme.foreground
        } else {
            theme.muted_foreground
        })
        .hover(|this| this.bg(theme.accent))
        .role(Role::Button)
        .aria_label(format!("Sort by {label}"))
        .tooltip(move |window, cx| {
            Tooltip::new(format!("Sort by {label}")).build(window, cx)
        })
        .on_click(move |_, _, cx| {
            state.update(cx, |state, cx| state.set_sort(key, cx));
        })
        .child(label)
        .when(is_active, |this| {
            this.child(div().flex_shrink_0().child(if descending {
                IconName::ArrowDown
            } else {
                IconName::ArrowUp
            }))
        })
        // Observed for UI integration tests; inert without `test-support`.
        .test_support()
}

// ---------------------------------------------------------------------------
// Entry Row
// ---------------------------------------------------------------------------

/// Data for a single row in the entry list.
#[derive(Clone)]
struct EntryRowData {
    /// Row index in the display order.
    index: usize,
    /// Entry name.
    name: SharedString,
    /// Entry status.
    status: DirEntryStatus,
    /// Whether this is a directory.
    is_dir: bool,
    /// Whether this row is selected.
    is_selected: bool,
    /// Size from left side, if available.
    left_size: Option<u64>,
    /// Size from right side, if available.
    right_size: Option<u64>,
    /// Modification time from left side, if available.
    left_modified: Option<SharedString>,
    /// Modification time from right side, if available.
    right_modified: Option<SharedString>,
    /// Whether entry exists on right side. Only the right side needs this:
    /// `OpenFileDiff` takes an optional right endpoint, while its left
    /// endpoint is always present.
    has_right: bool,
    /// Absolute left endpoint (comparison root joined with the side path).
    left_endpoint: Endpoint,
    /// Absolute right endpoint (comparison root joined with the side path).
    right_endpoint: Endpoint,
}

impl EntryRowData {
    /// Create entry row data from a comparison entry.
    ///
    /// `left_root` and `right_root` are the current comparison endpoints;
    /// side paths are resolved through [`entry_endpoints`].
    fn new(
        index: usize,
        entry: &DirEntry,
        is_selected: bool,
        left_root: &Endpoint,
        right_root: &Endpoint,
    ) -> Self {
        let has_right = entry.right.is_some();
        let left_size = entry.left.as_ref().map(|l| l.size);
        let right_size = entry.right.as_ref().map(|r| r.size);
        let left_modified = entry
            .left
            .as_ref()
            .map(|l| SharedString::from(l.modified.clone()));
        let right_modified = entry
            .right
            .as_ref()
            .map(|r| SharedString::from(r.modified.clone()));
        let is_dir = entry.left.as_ref().is_some_and(|l| l.is_dir)
            || entry.right.as_ref().is_some_and(|r| r.is_dir);
        let (left_endpoint, right_endpoint) =
            entry_endpoints(entry, left_root, right_root);

        Self {
            index,
            name: SharedString::from(entry.name.clone()),
            status: entry.status,
            is_dir,
            is_selected,
            left_size,
            right_size,
            left_modified,
            right_modified,
            has_right,
            left_endpoint,
            right_endpoint,
        }
    }

    /// Build the action that opens this entry: a nested directory compare
    /// for directories, a text diff for files. A file missing on one side
    /// diffs with `None` for that side, letting the handler fall back to
    /// the existing single-file behavior.
    fn open_action(&self) -> Box<dyn Action> {
        if self.is_dir {
            Box::new(OpenDirDiff {
                left_path: self.left_endpoint.clone(),
                right_path: self.right_endpoint.clone(),
            })
        } else {
            Box::new(OpenFileDiff {
                left_path: self.left_endpoint.clone(),
                right_path: self
                    .has_right
                    .then(|| self.right_endpoint.clone()),
            })
        }
    }
}

impl EntryRowData {
    /// Render a row as a div (for use in uniform_list).
    ///
    /// Selection is shown through the row's own surface (`list_active`
    /// fill, distinct `list_hover` feedback), following the `ListItem`
    /// treatment. A left click focuses the view and selects the row; a
    /// double-click additionally dispatches the open action. A right click
    /// selects the row and opens the context menu (Open, Copy path).
    fn render_row(
        self,
        theme: &Theme,
        state: Entity<AppState>,
        focus_handle: FocusHandle,
    ) -> impl IntoElement {
        let data = self.clone();
        let EntryRowData {
            index,
            name,
            status,
            is_dir,
            is_selected,
            left_size,
            right_size,
            left_modified,
            right_modified,
            has_right: _,
            left_endpoint: _,
            right_endpoint: _,
        } = self;

        let status_char = status_indicator(&status);
        let status_color = status_role(&status).color(theme);
        // Domain identity: entry names are unique within a directory, so
        // the row's stateful id survives reloads that reorder rows.
        let row_id = (ElementId::from("entry_row"), name.clone());
        let open_data = data.clone();
        let click_state = state.clone();
        let click_focus = focus_handle.clone();

        div()
            .id(row_id)
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_0p5()
            .text_color(theme.foreground)
            .when(is_selected, |this| this.bg(theme.list_active))
            // Register `hover` unconditionally; a conditional registration
            // leaves a stale hover style behind when the row turns selected.
            .hover(|this| {
                if is_selected {
                    this
                } else {
                    this.bg(theme.list_hover)
                }
            })
            .role(Role::ListItem)
            .aria_selected(is_selected)
            .on_click(move |event: &ClickEvent, window, cx| {
                // Clicking the list should make it the keyboard target,
                // so j/k/Enter keep working after pointer interaction.
                click_focus.focus(window, cx);
                let double_click = event.click_count() == 2;
                let open = click_state.update(cx, |state, cx| {
                    state.select_row(index, cx);
                    double_click.then(|| data.open_action())
                });
                if let Some(action) = open {
                    window.dispatch_action(action, cx);
                }
            })
            // Right click selects the row under the pointer before the
            // context menu (registered below) opens for it.
            .on_mouse_down(MouseButton::Right, {
                let state = state.clone();
                let focus = focus_handle.clone();
                move |_, window, cx| {
                    focus.focus(window, cx);
                    state.update(cx, |state, cx| {
                        state.select_row(index, cx);
                    });
                }
            })
            .context_menu(move |menu, _, _| {
                let menu = menu
                    .menu("Open", open_data.open_action())
                    .separator()
                    .menu("Copy left path", Box::new(CopyPathLeft))
                    .menu("Copy right path", Box::new(CopyPathRight));
                // Reconcile an orphan by copying it toward the side that is
                // missing it. A differing entry needs a confirmation flow
                // that is not built yet, so it is not offered here.
                match status {
                    DirEntryStatus::RightOnly => menu
                        .separator()
                        .menu("Copy to left", Box::new(CopyToLeft)),
                    DirEntryStatus::LeftOnly => menu
                        .separator()
                        .menu("Copy to right", Box::new(CopyToRight)),
                    _ => menu,
                }
            })
            // Status: dot carries the semantic color, the glyph keeps the
            // meaning independent of color (see the legend popover).
            .child(
                status_lane()
                    .child(
                        div()
                            .size_2()
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(status_color),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight(700.))
                            .text_color(status_color)
                            .child(status_char),
                    ),
            )
            // Name with a type icon, so the double-click result (enter a
            // directory vs. open a text diff) is predictable from the row.
            .child(
                name_lane()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .text_xs()
                            .flex_shrink_0()
                            .text_color(theme.muted_foreground)
                            .child(if is_dir {
                                IconName::Folder
                            } else {
                                IconName::File
                            }),
                    )
                    .child(div().truncate().child(name)),
            )
            // Mirrored left/right size and date lanes; a side missing the
            // entry shows an empty cell, existence being implied by the
            // status marker.
            .child(
                size_lane()
                    .child(left_size.map(format_size).unwrap_or_default()),
            )
            .child(
                date_lane().child(
                    left_modified
                        .as_deref()
                        .map(format_modified)
                        .unwrap_or_default(),
                ),
            )
            .child(
                size_lane()
                    .child(right_size.map(format_size).unwrap_or_default()),
            )
            .child(
                date_lane().child(
                    right_modified
                        .as_deref()
                        .map(format_modified)
                        .unwrap_or_default(),
                ),
            )
    }
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

/// Format a file size in human-readable form.
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Compact display for an RFC 3339 modification timestamp.
///
/// Renders `2026-10-27T21:50:51+01:00` as `2026-10-27 21:50`; seconds and
/// offset carry no comparison signal and the full string would dominate
/// the date lanes. Anything shorter than a timestamp is passed through.
fn format_modified(modified: &str) -> String {
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

#[cfg(test)]
mod tests {
    use cocomo_lib::EntryInfo;
    use gpui::TestAppContext;

    use super::*;

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
    fn row_endpoints_join_side_paths_onto_the_comparison_root() {
        let left_root = Endpoint::local("/left");
        let right_root = Endpoint::local("/right");
        let data = EntryRowData::new(
            0,
            &entry(
                "sub",
                Some(entry_info("sub", true)),
                Some(entry_info("other/sub", true)),
            ),
            false,
            &left_root,
            &right_root,
        );

        assert_eq!(data.left_endpoint.display(), "/left/sub");
        assert_eq!(data.right_endpoint.display(), "/right/other/sub");
    }

    #[test]
    fn row_endpoints_fall_back_to_name_for_missing_side() {
        let left_root = Endpoint::local("/left");
        let right_root = Endpoint::local("/right");
        let data = EntryRowData::new(
            0,
            &entry("orphan.txt", Some(entry_info("orphan.txt", false)), None),
            false,
            &left_root,
            &right_root,
        );

        assert_eq!(data.left_endpoint.display(), "/left/orphan.txt");
        assert_eq!(data.right_endpoint.display(), "/right/orphan.txt");
    }

    #[test]
    fn double_click_on_dir_opens_dir_diff() {
        let data = EntryRowData::new(
            3,
            &entry(
                "sub",
                Some(entry_info("sub", true)),
                Some(entry_info("sub", true)),
            ),
            false,
            &Endpoint::local("/left"),
            &Endpoint::local("/right"),
        );

        let expected = OpenDirDiff {
            left_path: Endpoint::local("/left/sub"),
            right_path: Endpoint::local("/right/sub"),
        };
        assert!(data.open_action().partial_eq(&expected));
    }

    #[test]
    fn double_click_on_file_opens_file_diff_with_both_sides() {
        let data = EntryRowData::new(
            0,
            &entry(
                "a.txt",
                Some(entry_info("a.txt", false)),
                Some(entry_info("a.txt", false)),
            ),
            false,
            &Endpoint::local("/left"),
            &Endpoint::local("/right"),
        );

        let expected = OpenFileDiff {
            left_path: Endpoint::local("/left/a.txt"),
            right_path: Some(Endpoint::local("/right/a.txt")),
        };
        assert!(data.open_action().partial_eq(&expected));
    }

    #[test]
    fn double_click_on_orphan_file_opens_file_diff_without_right() {
        let data = EntryRowData::new(
            0,
            &entry(
                "only-left.txt",
                Some(entry_info("only-left.txt", false)),
                None,
            ),
            false,
            &Endpoint::local("/left"),
            &Endpoint::local("/right"),
        );

        let expected = OpenFileDiff {
            left_path: Endpoint::local("/left/only-left.txt"),
            right_path: None,
        };
        assert!(data.open_action().partial_eq(&expected));
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
    /// production view in a headless window, clicks the "Left size" header,
    /// and verifies the table reorders through `AppState`.
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
        let size_header = (ElementId::from("sort_header"), "Left size");
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
