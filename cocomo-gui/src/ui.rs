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
//! status indicators, navigation, and entry rendering.

use std::ops::Range;

use cocomo_lib::{DirEntry, DirEntryStatus, Endpoint};
use gpui::{
    Action, App, ClickEvent, Context, ElementId, Entity, FocusHandle,
    FontWeight, Hsla, KeyBinding, Render, SharedString,
    UniformListScrollHandle, Window, actions,
    component::{
        ActiveTheme, Sizable as _, Theme, h_flex, spinner::Spinner,
        status_bar::StatusBar,
    },
    div,
    prelude::*,
    uniform_list,
};

use crate::{
    menus::{ReloadCompare, SaveSession},
    session_manager::GuiSessionManager,
    state::{AppState, StatusSummary},
};

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

actions!(
    folder_compare,
    [SelectNext, SelectPrev, EnterDir, LeaveDir, Reload]
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
        }
    }

    /// Return the focus handle for keyboard navigation.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle(cx).clone()
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
            // Menu-dispatched actions that need view context.
            .on_action(cx.listener(Self::handle_reload_compare))
            .on_action(cx.listener(Self::handle_save_session))
            // File/dir diff actions.
            .on_action(cx.listener(Self::handle_open_file_diff))
            .on_action(cx.listener(Self::handle_open_dir_diff))
    }
}

impl FolderCompareView {
    /// Render the header bar showing paths.
    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let left_path = state.left().display();
        let right_path = state.right().display();
        let theme = cx.theme();

        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_1()
            .bg(theme.muted)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(FontWeight(600.))
                            .text_color(theme.muted_foreground)
                            .child("cocomo"),
                    )
                    .child(div().text_color(theme.muted_foreground).child("/"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.foreground)
                            .child(left_path),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.foreground)
                            .child(right_path),
                    )
                    .child(div().text_color(theme.muted_foreground).child("/"))
                    .child(
                        div()
                            .font_weight(FontWeight(600.))
                            .text_color(theme.muted_foreground)
                            .child("compare"),
                    ),
            )
    }

    /// Render the main content area with the entry list.
    fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.read(cx);
        let entry_count = state.entry_count();
        let is_loading = state.is_loading();
        let theme = cx.theme();
        // The list is the keyboard target of this view's focus handle; a
        // tinted border tells the user where j/k/Enter will act.
        let list_focused = self.focus_handle(cx).is_focused(window);

        // Clone data needed inside the closure.
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
            .child(render_column_headers(theme))
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
                                        if let Some(comparison) =
                                            app_state.comparison()
                                        {
                                            let selected =
                                                app_state.selected_index();
                                            let left_root =
                                                app_state.left().clone();
                                            let right_root =
                                                app_state.right().clone();
                                            for i in range {
                                                if let Some(entry) =
                                                    comparison
                                                        .entries
                                                        .get(i)
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
    /// Left region: neutral summary counts for the current comparison.
    /// Right region: what is happening now — a spinner while loading, the
    /// error in danger color, or the selection position within the list.
    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let theme = cx.theme();

        let mut bar = StatusBar::new();

        if let Some(comparison) = state.comparison() {
            let summary = StatusSummary::from_comparison(comparison);
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

        let status = if state.is_loading() {
            h_flex()
                .items_center()
                .gap_1p5()
                .child(Spinner::new().xsmall())
                .child("Loading comparison…")
                .into_any_element()
        } else if let Some(err) = state.error() {
            div()
                .text_color(theme.danger)
                .child(err.to_string())
                .into_any_element()
        } else if let Some(comparison) = state.comparison() {
            if comparison.entries.is_empty() {
                div().child("No entries").into_any_element()
            } else {
                div()
                    .child(format!(
                        "{} of {}",
                        state.selected_index() + 1,
                        comparison.entries.len()
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

/// Render the column header row.
///
/// The fixed lane widths are shared with [`EntryRowData::render_row`] so
/// headers and values stay aligned; they are functional data lanes, not
/// decorative sizes.
fn render_column_headers(theme: &Theme) -> impl IntoElement {
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
        .text_color(theme.muted_foreground)
        .font_weight(FontWeight(600.))
        .child(div().w_4().child("st"))
        .child(div().w_4().child("ty"))
        .child(div().flex_1().child("name"))
        .child(div().w_20().child("size"))
        .child(div().w_16().child("left"))
        .child(div().w_16().child("right"))
}

// ---------------------------------------------------------------------------
// Entry Row
// ---------------------------------------------------------------------------

/// Data for a single row in the entry list.
#[derive(Clone)]
struct EntryRowData {
    /// Row index in the full list.
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
    /// Whether entry exists on left side.
    has_left: bool,
    /// Whether entry exists on right side.
    has_right: bool,
    /// Absolute left endpoint (comparison root joined with the side path).
    left_endpoint: Endpoint,
    /// Absolute right endpoint (comparison root joined with the side path).
    right_endpoint: Endpoint,
}

impl EntryRowData {
    /// Create entry row data from a comparison entry.
    ///
    /// `left_root` and `right_root` are the current comparison endpoints.
    /// An entry's side `path` is relative to the comparison root, so it is
    /// joined onto the root to obtain the absolute endpoint; a side missing
    /// the entry falls back to the entry name, mirroring the navigation in
    /// [`FolderCompareView::enter_dir`].
    fn new(
        index: usize,
        entry: &DirEntry,
        is_selected: bool,
        left_root: &Endpoint,
        right_root: &Endpoint,
    ) -> Self {
        let has_left = entry.left.is_some();
        let has_right = entry.right.is_some();
        let left_size = entry.left.as_ref().map(|l| l.size);
        let right_size = entry.right.as_ref().map(|r| r.size);
        let is_dir = entry.left.as_ref().is_some_and(|l| l.is_dir)
            || entry.right.as_ref().is_some_and(|r| r.is_dir);
        let left_endpoint = left_root.join(
            entry
                .left
                .as_ref()
                .map_or(entry.name.as_str(), |l| l.path.as_str()),
        );
        let right_endpoint = right_root.join(
            entry
                .right
                .as_ref()
                .map_or(entry.name.as_str(), |r| r.path.as_str()),
        );

        Self {
            index,
            name: SharedString::from(entry.name.clone()),
            status: entry.status,
            is_dir,
            is_selected,
            left_size,
            right_size,
            has_left,
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
    /// treatment. A click focuses the view and selects the row; a
    /// double-click additionally dispatches the open action for the entry.
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
            has_left,
            has_right,
            left_endpoint: _,
            right_endpoint: _,
        } = self;

        let status_char = status_indicator(&status);
        let dir_icon = if is_dir { "D" } else { "F" };
        // Domain identity: entry names are unique within a directory, so
        // the row's stateful id survives reloads that reorder rows.
        let row_id = (ElementId::from("entry_row"), name.clone());

        let size_display = if has_left && has_right {
            if left_size == right_size {
                format_size(left_size.unwrap_or(0))
            } else {
                format!(
                    "{}/{}",
                    format_size(left_size.unwrap_or(0)),
                    format_size(right_size.unwrap_or(0))
                )
            }
        } else if has_left || has_right {
            format_size(left_size.or(right_size).unwrap_or(0))
        } else {
            String::new()
        };

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
            .on_click(move |event: &ClickEvent, window, cx| {
                // Clicking the list should make it the keyboard target,
                // so j/k/Enter keep working after pointer interaction.
                focus_handle.focus(window, cx);
                let double_click = event.click_count() == 2;
                let open = state.update(cx, |state, cx| {
                    state.select_row(index, cx);
                    double_click.then(|| data.open_action())
                });
                if let Some(action) = open {
                    window.dispatch_action(action, cx);
                }
            })
            .child(
                div()
                    .w_4()
                    .text_color(status_role(&status).color(theme))
                    .font_weight(FontWeight(700.))
                    .child(status_char),
            )
            .child(
                div()
                    .w_4()
                    .text_color(theme.muted_foreground)
                    .child(dir_icon),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(if has_left || has_right {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .child(name),
            )
            .child(
                div()
                    .w_20()
                    .text_color(theme.muted_foreground)
                    .child(size_display),
            )
            .child(
                div()
                    .w_16()
                    .text_color(if has_left {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .child(if has_left { "yes" } else { "—" }),
            )
            .child(
                div()
                    .w_16()
                    .text_color(if has_right {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .child(if has_right { "yes" } else { "—" }),
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
        DirEntryStatus::Similar
        | DirEntryStatus::IdenticalNameDifferentType => StatusRole::Warning,
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
        DirEntryStatus::IdenticalNameDifferentType => "T",
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

#[cfg(test)]
mod tests {
    use cocomo_lib::EntryInfo;

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
            status_role(&DirEntryStatus::IdenticalNameDifferentType),
            StatusRole::Warning
        );
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
}
